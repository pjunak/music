use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use music_application::modes::{
    ModeBundle, ModeCatalogSource, ModeLoadAttempt, ModeSourceError, ModeSourceFuture,
};

use crate::yaml::{
    YamlDocumentError, parse_cue_document, parse_mode_document, parse_preset_document,
    parse_soundboard_document,
};

const MAX_ROOT_ENTRIES: usize = 1_024;
const MAX_MODES: usize = 256;
const MAX_DOCUMENTS_PER_MODE: usize = 4_096;
const MAX_AUTHORED_DOCUMENT_BYTES: u64 = 1_024 * 1_024;
const MAX_THEME_BYTES: u64 = 512 * 1_024;
const MAX_SLUG_CHARS: usize = 64;

#[derive(Debug, Clone)]
pub struct FilesystemModeCatalogSource {
    root: PathBuf,
}

impl FilesystemModeCatalogSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ModeSourceError> {
        let root = fs::canonicalize(path.as_ref())
            .map_err(|_| ModeSourceError::new("modes directory could not be resolved"))?;
        let metadata = fs::metadata(&root)
            .map_err(|_| ModeSourceError::new("modes directory could not be inspected"))?;
        if !metadata.is_dir() {
            return Err(ModeSourceError::new("modes path is not a directory"));
        }
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl ModeCatalogSource for FilesystemModeCatalogSource {
    fn load<'a>(&'a self) -> ModeSourceFuture<'a> {
        let root = self.root.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || load_catalog(&root))
                .await
                .map_err(|_| ModeSourceError::new("mode loader worker did not complete"))?
        })
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct ModeBundleError(String);

impl Display for ModeBundleError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ModeBundleError {}

impl From<YamlDocumentError> for ModeBundleError {
    fn from(error: YamlDocumentError) -> Self {
        Self(error.to_string())
    }
}

fn load_catalog(root: &Path) -> Result<ModeLoadAttempt, ModeSourceError> {
    let root_metadata = fs::metadata(root)
        .map_err(|_| ModeSourceError::new("modes directory could not be inspected"))?;
    if !root_metadata.is_dir() {
        return Err(ModeSourceError::new("modes path is not a directory"));
    }
    let mut entries = fs::read_dir(root)
        .map_err(|_| ModeSourceError::new("modes directory could not be read"))?
        .take(MAX_ROOT_ENTRIES + 1)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ModeSourceError::new("modes directory entry could not be read"))?;
    if entries.len() > MAX_ROOT_ENTRIES {
        return Err(ModeSourceError::new(
            "modes directory contains too many entries",
        ));
    }
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let mut attempt = ModeLoadAttempt::default();

    let mut mode_directories = 0_usize;
    for (index, entry) in entries.into_iter().enumerate() {
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => {
                attempt.errors.insert(
                    format!("<invalid-{index}>"),
                    "mode directory name is not valid Unicode".to_owned(),
                );
                continue;
            }
        };
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                attempt
                    .errors
                    .insert(name, "mode directory could not be inspected".to_owned());
                continue;
            }
        };
        if metadata.file_type().is_symlink() {
            attempt.errors.insert(
                name,
                "mode directory must not be a symbolic link".to_owned(),
            );
            continue;
        }
        if !metadata.is_dir() {
            continue;
        }
        mode_directories += 1;
        if mode_directories > MAX_MODES {
            return Err(ModeSourceError::new(
                "modes directory contains too many modes",
            ));
        }
        if !valid_slug(&name) {
            attempt.errors.insert(
                name,
                "mode id must be a lowercase filesystem slug".to_owned(),
            );
            continue;
        }
        match load_mode(&path, &name) {
            Ok(mode) => {
                attempt.modes.insert(name, mode);
            }
            Err(error) => {
                attempt.errors.insert(name, error.to_string());
            }
        }
    }
    Ok(attempt)
}

fn load_mode(path: &Path, mode_id: &str) -> Result<ModeBundle, ModeBundleError> {
    let manifest_source = read_document(&path.join("manifest.yaml"), "manifest.yaml")?;
    let manifest = parse_mode_document(&manifest_source, mode_id)?;
    let soundboards = load_documents(
        &path.join("soundboards"),
        "soundboard",
        parse_soundboard_document,
    )?;
    let cues = load_documents(&path.join("cues"), "cue", parse_cue_document)?;
    let presets = load_documents(&path.join("presets"), "preset", parse_preset_document)?;
    if manifest
        .default_soundboard
        .as_ref()
        .is_some_and(|id| !soundboards.contains_key(id))
    {
        return Err(ModeBundleError(
            "default_soundboard does not name a loaded soundboard".to_owned(),
        ));
    }
    let theme_css = manifest
        .theme
        .as_deref()
        .map(|theme| read_theme(path, theme))
        .transpose()?
        .map(Arc::<str>::from);
    Ok(ModeBundle {
        manifest,
        soundboards,
        cues,
        presets,
        theme_css,
    })
}

fn load_documents<T>(
    directory: &Path,
    kind: &'static str,
    parse: fn(&str, &str) -> Result<T, YamlDocumentError>,
) -> Result<BTreeMap<String, T>, ModeBundleError> {
    let metadata = match fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BTreeMap::new());
        }
        Err(_) => {
            return Err(ModeBundleError(format!(
                "{kind} directory could not be inspected"
            )));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ModeBundleError(format!(
            "{kind} path must be a regular directory"
        )));
    }
    let mut entries = fs::read_dir(directory)
        .map_err(|_| ModeBundleError(format!("{kind} directory could not be read")))?
        .take(MAX_DOCUMENTS_PER_MODE + 1)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ModeBundleError(format!("{kind} directory entry could not be read")))?;
    if entries.len() > MAX_DOCUMENTS_PER_MODE {
        return Err(ModeBundleError(format!(
            "{kind} directory contains too many documents"
        )));
    }
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let mut documents = BTreeMap::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("yaml") {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| ModeBundleError(format!("{kind} document could not be inspected")))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(ModeBundleError(format!(
                "{kind} document must be a regular file"
            )));
        }
        let id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .filter(|id| valid_slug(id))
            .ok_or_else(|| ModeBundleError(format!("{kind} filename is not a valid slug")))?;
        let source = read_document(&path, kind)?;
        let document = parse(&source, id)?;
        if documents.insert(id.to_owned(), document).is_some() {
            return Err(ModeBundleError(format!("duplicate {kind} id")));
        }
    }
    Ok(documents)
}

fn read_document(path: &Path, label: &'static str) -> Result<String, ModeBundleError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ModeBundleError(format!("{label} could not be inspected")))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ModeBundleError(format!("{label} must be a regular file")));
    }
    if metadata.len() > MAX_AUTHORED_DOCUMENT_BYTES {
        return Err(ModeBundleError(format!("{label} is too large")));
    }
    fs::read_to_string(path).map_err(|_| ModeBundleError(format!("{label} is not valid UTF-8")))
}

fn read_theme(mode_root: &Path, relative: &str) -> Result<String, ModeBundleError> {
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty()
        || relative.components().any(|component| {
            !matches!(component, Component::Normal(_))
                || component
                    .as_os_str()
                    .to_str()
                    .is_none_or(|value| value.contains('/') || value.contains('\\'))
        })
    {
        return Err(ModeBundleError("theme path is unsafe".to_owned()));
    }
    let canonical_root = fs::canonicalize(mode_root)
        .map_err(|_| ModeBundleError("mode directory could not be resolved".to_owned()))?;
    let mut current = canonical_root.clone();
    let mut components = relative.components().peekable();
    while let Some(Component::Normal(component)) = components.next() {
        let candidate = current.join(component);
        let metadata = fs::symlink_metadata(&candidate)
            .map_err(|_| ModeBundleError("theme file could not be inspected".to_owned()))?;
        if metadata.file_type().is_symlink()
            || (components.peek().is_some() && !metadata.is_dir())
            || (components.peek().is_none() && !metadata.is_file())
        {
            return Err(ModeBundleError("theme path is unsafe".to_owned()));
        }
        current = fs::canonicalize(&candidate)
            .map_err(|_| ModeBundleError("theme path could not be resolved".to_owned()))?;
        if !current.starts_with(&canonical_root) {
            return Err(ModeBundleError("theme path escapes the mode".to_owned()));
        }
    }
    let metadata = fs::metadata(&current)
        .map_err(|_| ModeBundleError("theme file could not be inspected".to_owned()))?;
    if metadata.len() > MAX_THEME_BYTES {
        return Err(ModeBundleError("theme file is too large".to_owned()));
    }
    fs::read_to_string(&current)
        .map_err(|_| ModeBundleError("theme file is not valid UTF-8".to_owned()))
}

fn valid_slug(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    value.chars().count() <= MAX_SLUG_CHARS
        && is_ascii_slug_character(first)
        && characters
            .all(|character| is_ascii_slug_character(character) || matches!(character, '-' | '_'))
}

const fn is_ascii_slug_character(character: char) -> bool {
    character.is_ascii_lowercase() || character.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;

    use music_application::modes::ModeCatalogSource;
    use tempfile::tempdir;

    use super::FilesystemModeCatalogSource;

    #[tokio::test]
    async fn loads_valid_bundles_and_isolates_a_broken_mode() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        fs::create_dir_all(directory.path().join("table/soundboards"))?;
        fs::write(
            directory.path().join("table/manifest.yaml"),
            "id: table\nname: Table\ntheme: theme.css\ndefault_soundboard: main\n",
        )?;
        fs::write(
            directory.path().join("table/theme.css"),
            ":root { color: red; }\n",
        )?;
        fs::write(
            directory.path().join("table/soundboards/main.yaml"),
            "name: Main\ncategories: []\n",
        )?;
        fs::create_dir(directory.path().join("broken"))?;
        fs::write(
            directory.path().join("broken/manifest.yaml"),
            "id: other\nname: Broken\n",
        )?;
        let source = FilesystemModeCatalogSource::open(directory.path())?;

        let attempt = source.load().await?;

        assert_eq!(attempt.modes.len(), 1);
        let table = attempt.modes.get("table").ok_or("table mode missing")?;
        assert_eq!(table.theme_css.as_deref(), Some(":root { color: red; }\n"));
        assert!(table.soundboards.contains_key("main"));
        assert!(attempt.errors.contains_key("broken"));
        Ok(())
    }
}
