use std::collections::BTreeMap;
use std::error::Error;
use std::ffi::OsString;
use std::fmt::{self, Display, Formatter};
use std::io;
use std::path::{Path, PathBuf};

use axum::http::Uri;
use music_storage::SecretString;

const DEFAULT_DATABASE_URL: &str = "sqlite:///./app.db";
const DEFAULT_ALLOWED_ORIGINS: &str = "http://localhost:5173";
const DEFAULT_STATIC_DIR: &str = "/app/static";

const ENVIRONMENT_NAMES: &[&str] = &[
    "DATABASE_URL",
    "MUSIC_DIR",
    "SFX_LIBRARY_DIR",
    "MODES_DIR",
    "DEVICES_FILE",
    "MODES_SEED_DIR",
    "ADVANCER_ENABLED",
    "ALLOWED_ORIGINS",
    "SESSION_COOKIE_SECURE",
    "SESSION_COOKIE_DOMAIN",
    "SESSION_COOKIE_NAME",
    "SESSION_TTL_DAYS",
    "ASSISTANT_CREDENTIAL_KEY",
    "ASSISTANT_CREDENTIAL_KEY_FILE",
    "ASSISTANT_CREDENTIAL_HOST_DIRECTORY_HINT",
    "ASSISTANT_VOICE_MODEL_PATH",
    "ASSISTANT_LIBRARY_CONTEXT_WORKERS",
    "MAX_UPLOAD_FILES",
    "MAX_UPLOAD_FILE_BYTES",
    "LOG_LEVEL",
    "STATIC_DIR",
];

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Display for LogLevel {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        })
    }
}

#[derive(Debug)]
pub struct AppConfig {
    pub database_url: String,
    pub database_path: PathBuf,
    pub music_dir: PathBuf,
    pub sfx_library_dir: PathBuf,
    pub modes_dir: PathBuf,
    pub devices_file: PathBuf,
    pub modes_seed_dir: Option<PathBuf>,
    pub advancer_enabled: bool,
    pub allowed_origins: Vec<String>,
    pub session_cookie_secure: bool,
    pub session_cookie_domain: Option<String>,
    pub session_cookie_name: String,
    pub session_ttl_days: u32,
    pub assistant_credential_key: Option<SecretString>,
    pub assistant_credential_key_file: Option<PathBuf>,
    pub assistant_credential_host_directory_hint: Option<String>,
    pub assistant_voice_model_path: Option<PathBuf>,
    pub assistant_library_context_workers: u8,
    pub max_upload_files: usize,
    pub max_upload_file_bytes: u64,
    pub request_body_limit_bytes: usize,
    pub log_level: LogLevel,
    pub static_dir: Option<PathBuf>,
}

impl AppConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let current_directory = std::env::current_dir().map_err(ConfigError::CurrentDirectory)?;
        let mut process_values = BTreeMap::new();
        for &name in ENVIRONMENT_NAMES {
            if let Some(value) = std::env::var_os(name) {
                process_values.insert(name.to_owned(), unicode_environment_value(name, value)?);
            }
        }
        Self::load_from(&current_directory, &process_values)
    }

    pub fn from_values(values: &BTreeMap<String, String>) -> Result<Self, ConfigError> {
        let database_url = value(values, "DATABASE_URL", DEFAULT_DATABASE_URL)
            .trim()
            .to_owned();
        let database_path = sqlite_path(&database_url)?;
        let max_upload_files =
            parse_positive::<usize>(values, "MAX_UPLOAD_FILES", 500, "a positive integer")?;
        let max_upload_file_bytes = parse_positive::<u64>(
            values,
            "MAX_UPLOAD_FILE_BYTES",
            1024_u64.pow(3),
            "a positive integer number of bytes",
        )?;
        let aggregate_upload_bytes = max_upload_file_bytes
            .checked_mul(u64::try_from(max_upload_files).map_err(|_| {
                ConfigError::invalid("MAX_UPLOAD_FILES", "a platform-sized positive integer")
            })?)
            .ok_or_else(|| {
                ConfigError::invalid(
                    "MAX_UPLOAD_FILES/MAX_UPLOAD_FILE_BYTES",
                    "a bounded aggregate request size",
                )
            })?;
        let request_body_limit_bytes = usize::try_from(aggregate_upload_bytes).map_err(|_| {
            ConfigError::invalid(
                "MAX_UPLOAD_FILES/MAX_UPLOAD_FILE_BYTES",
                "an aggregate size supported by this platform",
            )
        })?;

        let workers = parse_number::<u8>(
            values,
            "ASSISTANT_LIBRARY_CONTEXT_WORKERS",
            1,
            "an integer from 1 through 4",
        )?;
        if !(1..=4).contains(&workers) {
            return Err(ConfigError::invalid(
                "ASSISTANT_LIBRARY_CONTEXT_WORKERS",
                "an integer from 1 through 4",
            ));
        }

        let session_ttl_days =
            parse_positive::<u32>(values, "SESSION_TTL_DAYS", 30, "a positive integer")?;
        let session_cookie_name = value(values, "SESSION_COOKIE_NAME", "music_session")
            .trim()
            .to_owned();
        if !is_cookie_name(&session_cookie_name) {
            return Err(ConfigError::invalid(
                "SESSION_COOKIE_NAME",
                "a non-empty HTTP cookie token",
            ));
        }

        Ok(Self {
            database_url,
            database_path,
            music_dir: required_path(values, "MUSIC_DIR", "./music")?,
            sfx_library_dir: required_path(values, "SFX_LIBRARY_DIR", "./sfx")?,
            modes_dir: required_path(values, "MODES_DIR", "../modes")?,
            devices_file: required_path(values, "DEVICES_FILE", "./devices.json")?,
            modes_seed_dir: optional_path(values, "MODES_SEED_DIR"),
            advancer_enabled: parse_bool(values, "ADVANCER_ENABLED", true)?,
            allowed_origins: parse_origins(value(
                values,
                "ALLOWED_ORIGINS",
                DEFAULT_ALLOWED_ORIGINS,
            ))?,
            session_cookie_secure: parse_bool(values, "SESSION_COOKIE_SECURE", true)?,
            session_cookie_domain: optional_text(values, "SESSION_COOKIE_DOMAIN"),
            session_cookie_name,
            session_ttl_days,
            assistant_credential_key: optional_text(values, "ASSISTANT_CREDENTIAL_KEY")
                .map(SecretString::new),
            assistant_credential_key_file: optional_path(values, "ASSISTANT_CREDENTIAL_KEY_FILE"),
            assistant_credential_host_directory_hint: optional_text(
                values,
                "ASSISTANT_CREDENTIAL_HOST_DIRECTORY_HINT",
            ),
            assistant_voice_model_path: optional_path(values, "ASSISTANT_VOICE_MODEL_PATH"),
            assistant_library_context_workers: workers,
            max_upload_files,
            max_upload_file_bytes,
            request_body_limit_bytes,
            log_level: parse_log_level(value(values, "LOG_LEVEL", "info"))?,
            static_dir: optional_path_with_default(values, "STATIC_DIR", DEFAULT_STATIC_DIR),
        })
    }

    fn load_from(
        current_directory: &Path,
        process_values: &BTreeMap<String, String>,
    ) -> Result<Self, ConfigError> {
        let mut values = read_dotenv(&current_directory.join(".env"))?;
        for (name, value) in process_values {
            if ENVIRONMENT_NAMES.contains(&name.as_str()) {
                values.insert(name.clone(), value.clone());
            }
        }
        Self::from_values(&values)
    }
}

#[derive(Debug)]
pub enum ConfigError {
    CurrentDirectory(io::Error),
    Dotenv {
        path: PathBuf,
    },
    NonUnicodeEnvironment {
        variable: &'static str,
    },
    Invalid {
        variable: &'static str,
        expected: &'static str,
    },
}

impl ConfigError {
    const fn invalid(variable: &'static str, expected: &'static str) -> Self {
        Self::Invalid { variable, expected }
    }
}

impl Display for ConfigError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentDirectory(_) => {
                formatter.write_str("could not resolve the configuration directory")
            }
            Self::Dotenv { path } => {
                write!(
                    formatter,
                    "could not read configuration from {}",
                    path.display()
                )
            }
            Self::NonUnicodeEnvironment { variable } => {
                write!(formatter, "{variable} is not valid Unicode")
            }
            Self::Invalid { variable, expected } => {
                write!(formatter, "invalid {variable}; expected {expected}")
            }
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CurrentDirectory(source) => Some(source),
            Self::Dotenv { .. } | Self::NonUnicodeEnvironment { .. } | Self::Invalid { .. } => None,
        }
    }
}

fn unicode_environment_value(
    variable: &'static str,
    value: OsString,
) -> Result<String, ConfigError> {
    value
        .into_string()
        .map_err(|_| ConfigError::NonUnicodeEnvironment { variable })
}

fn read_dotenv(path: &Path) -> Result<BTreeMap<String, String>, ConfigError> {
    match path.try_exists() {
        Ok(false) => return Ok(BTreeMap::new()),
        Ok(true) => {}
        Err(_) => {
            return Err(ConfigError::Dotenv {
                path: path.to_path_buf(),
            });
        }
    }
    let iterator = dotenvy::from_path_iter(path).map_err(|_| ConfigError::Dotenv {
        path: path.to_path_buf(),
    })?;
    let mut values = BTreeMap::new();
    for entry in iterator {
        let (name, value) = entry.map_err(|_| ConfigError::Dotenv {
            path: path.to_path_buf(),
        })?;
        if ENVIRONMENT_NAMES.contains(&name.as_str()) {
            values.insert(name, value);
        }
    }
    Ok(values)
}

fn value<'a>(values: &'a BTreeMap<String, String>, name: &str, default: &'a str) -> &'a str {
    values.get(name).map_or(default, String::as_str)
}

fn path_value(values: &BTreeMap<String, String>, name: &str, default: &str) -> PathBuf {
    PathBuf::from(value(values, name, default))
}

fn required_path(
    values: &BTreeMap<String, String>,
    name: &'static str,
    default: &str,
) -> Result<PathBuf, ConfigError> {
    let path = path_value(values, name, default);
    if path.as_os_str().is_empty() {
        Err(ConfigError::invalid(name, "a non-empty filesystem path"))
    } else {
        Ok(path)
    }
}

fn optional_path_with_default(
    values: &BTreeMap<String, String>,
    name: &str,
    default: &str,
) -> Option<PathBuf> {
    match values.get(name) {
        Some(configured) if configured.trim().is_empty() => None,
        Some(configured) => Some(PathBuf::from(configured)),
        None => Some(PathBuf::from(default)),
    }
}

fn optional_path(values: &BTreeMap<String, String>, name: &str) -> Option<PathBuf> {
    optional_text(values, name).map(PathBuf::from)
}

fn optional_text(values: &BTreeMap<String, String>, name: &str) -> Option<String> {
    values
        .get(name)
        .map(|configured| configured.trim())
        .filter(|configured| !configured.is_empty())
        .map(str::to_owned)
}

fn parse_number<T>(
    values: &BTreeMap<String, String>,
    name: &'static str,
    default: T,
    expected: &'static str,
) -> Result<T, ConfigError>
where
    T: std::str::FromStr,
{
    match values.get(name) {
        Some(configured) => configured
            .trim()
            .parse()
            .map_err(|_| ConfigError::invalid(name, expected)),
        None => Ok(default),
    }
}

fn parse_positive<T>(
    values: &BTreeMap<String, String>,
    name: &'static str,
    default: T,
    expected: &'static str,
) -> Result<T, ConfigError>
where
    T: std::str::FromStr + PartialEq + Default,
{
    let parsed = parse_number(values, name, default, expected)?;
    if parsed == T::default() {
        Err(ConfigError::invalid(name, expected))
    } else {
        Ok(parsed)
    }
}

fn parse_bool(
    values: &BTreeMap<String, String>,
    name: &'static str,
    default: bool,
) -> Result<bool, ConfigError> {
    let Some(configured) = values.get(name) else {
        return Ok(default);
    };
    match configured.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::invalid(name, "a boolean")),
    }
}

fn parse_log_level(configured: &str) -> Result<LogLevel, ConfigError> {
    match configured.trim().to_ascii_lowercase().as_str() {
        "trace" => Ok(LogLevel::Trace),
        "debug" => Ok(LogLevel::Debug),
        "info" => Ok(LogLevel::Info),
        "warn" | "warning" => Ok(LogLevel::Warn),
        "error" => Ok(LogLevel::Error),
        _ => Err(ConfigError::invalid(
            "LOG_LEVEL",
            "trace, debug, info, warn, or error",
        )),
    }
}

fn parse_origins(configured: &str) -> Result<Vec<String>, ConfigError> {
    configured
        .split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .map(|origin| {
            let normalized = origin.strip_suffix('/').unwrap_or(origin);
            let uri: Uri = normalized.parse().map_err(|_| {
                ConfigError::invalid("ALLOWED_ORIGINS", "comma-separated HTTP(S) origins")
            })?;
            let valid_scheme = matches!(uri.scheme_str(), Some("http" | "https"));
            let valid_authority = uri
                .authority()
                .is_some_and(|authority| !authority.as_str().contains('@'));
            if !valid_scheme || !valid_authority || uri.path() != "/" || uri.query().is_some() {
                return Err(ConfigError::invalid(
                    "ALLOWED_ORIGINS",
                    "comma-separated HTTP(S) origins without paths",
                ));
            }
            Ok(normalized.to_owned())
        })
        .collect()
}

fn sqlite_path(database_url: &str) -> Result<PathBuf, ConfigError> {
    let path = database_url
        .strip_prefix("sqlite:///")
        .ok_or_else(|| ConfigError::invalid("DATABASE_URL", "a file-backed sqlite:/// URL"))?;
    if path.is_empty()
        || path == ":memory:"
        || path.contains('?')
        || path.contains('#')
        || path.contains('\0')
    {
        return Err(ConfigError::invalid(
            "DATABASE_URL",
            "a file-backed sqlite:/// URL without query parameters",
        ));
    }
    Ok(PathBuf::from(path))
}

fn is_cookie_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::error::Error;
    use std::fs;

    use tempfile::tempdir;

    use super::{AppConfig, ConfigError, LogLevel};

    #[test]
    fn defaults_match_the_python_deployment_contract() -> Result<(), Box<dyn Error>> {
        let config = AppConfig::from_values(&BTreeMap::new())?;

        assert_eq!(config.database_url, "sqlite:///./app.db");
        assert_eq!(config.database_path, std::path::Path::new("./app.db"));
        assert_eq!(config.music_dir, std::path::Path::new("./music"));
        assert_eq!(config.sfx_library_dir, std::path::Path::new("./sfx"));
        assert_eq!(config.modes_dir, std::path::Path::new("../modes"));
        assert_eq!(config.devices_file, std::path::Path::new("./devices.json"));
        assert!(config.modes_seed_dir.is_none());
        assert!(config.advancer_enabled);
        assert_eq!(config.allowed_origins, ["http://localhost:5173"]);
        assert!(config.session_cookie_secure);
        assert_eq!(config.session_cookie_name, "music_session");
        assert_eq!(config.session_ttl_days, 30);
        assert_eq!(config.assistant_library_context_workers, 1);
        assert_eq!(config.max_upload_files, 500);
        assert_eq!(config.max_upload_file_bytes, 1024_u64.pow(3));
        assert_eq!(config.log_level, LogLevel::Info);
        assert_eq!(
            config.static_dir.as_deref(),
            Some(std::path::Path::new("/app/static"))
        );
        Ok(())
    }

    #[test]
    fn validates_and_redacts_operator_overrides() -> Result<(), Box<dyn Error>> {
        let values = BTreeMap::from([
            (
                "DATABASE_URL".to_owned(),
                "sqlite:////data/app.db".to_owned(),
            ),
            ("ADVANCER_ENABLED".to_owned(), "off".to_owned()),
            (
                "ALLOWED_ORIGINS".to_owned(),
                "https://music.example, http://localhost:5173/".to_owned(),
            ),
            ("SESSION_COOKIE_SECURE".to_owned(), "false".to_owned()),
            ("SESSION_TTL_DAYS".to_owned(), "90".to_owned()),
            (
                "ASSISTANT_CREDENTIAL_KEY".to_owned(),
                "definitely-secret".to_owned(),
            ),
            (
                "ASSISTANT_LIBRARY_CONTEXT_WORKERS".to_owned(),
                "4".to_owned(),
            ),
            ("MAX_UPLOAD_FILES".to_owned(), "2".to_owned()),
            ("MAX_UPLOAD_FILE_BYTES".to_owned(), "4096".to_owned()),
            ("LOG_LEVEL".to_owned(), "warning".to_owned()),
        ]);
        let config = AppConfig::from_values(&values)?;

        assert_eq!(config.database_path, std::path::Path::new("/data/app.db"));
        assert!(!config.advancer_enabled);
        assert_eq!(
            config.allowed_origins,
            ["https://music.example", "http://localhost:5173"]
        );
        assert!(!config.session_cookie_secure);
        assert_eq!(config.request_body_limit_bytes, 8192);
        assert_eq!(config.log_level, LogLevel::Warn);
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("definitely-secret"));
        Ok(())
    }

    #[test]
    fn process_values_override_dotenv_without_mutating_the_process() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        fs::write(
            directory.path().join(".env"),
            "LOG_LEVEL=debug\nMAX_UPLOAD_FILES=3\n",
        )?;
        let process = BTreeMap::from([("LOG_LEVEL".to_owned(), "error".to_owned())]);

        let config = AppConfig::load_from(directory.path(), &process)?;

        assert_eq!(config.log_level, LogLevel::Error);
        assert_eq!(config.max_upload_files, 3);
        Ok(())
    }

    #[test]
    fn rejects_ambiguous_or_unbounded_configuration() {
        for (name, value) in [
            ("DATABASE_URL", "postgresql://example/music"),
            ("DATABASE_URL", "sqlite:///:memory:"),
            ("ALLOWED_ORIGINS", "https://example.test/path"),
            ("ASSISTANT_LIBRARY_CONTEXT_WORKERS", "5"),
            ("MAX_UPLOAD_FILES", "0"),
            ("SESSION_COOKIE_NAME", "bad name"),
            ("MUSIC_DIR", ""),
        ] {
            let values = BTreeMap::from([(name.to_owned(), value.to_owned())]);
            assert!(matches!(
                AppConfig::from_values(&values),
                Err(ConfigError::Invalid { .. })
            ));
        }
    }
}
