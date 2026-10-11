use std::collections::BTreeMap;
use std::path::Path;

use crate::config::{AppConfig, ConfigError};

/// A complete server configuration rooted in a temporary directory, with
/// optional overrides for the setting a test exercises.
pub(crate) fn test_config(
    root: &Path,
    overrides: &[(&str, &str)],
) -> Result<AppConfig, ConfigError> {
    let mut values = BTreeMap::from([
        (
            "DATABASE_URL".to_owned(),
            format!("sqlite:///{}", root.join("app.db").display()),
        ),
        (
            "MUSIC_DIR".to_owned(),
            root.join("music").display().to_string(),
        ),
        (
            "SFX_LIBRARY_DIR".to_owned(),
            root.join("sfx").display().to_string(),
        ),
        (
            "MODES_DIR".to_owned(),
            root.join("modes").display().to_string(),
        ),
        (
            "STATIC_DIR".to_owned(),
            root.join("missing-static").display().to_string(),
        ),
        ("SESSION_COOKIE_SECURE".to_owned(), "false".to_owned()),
    ]);
    for (name, value) in overrides {
        values.insert((*name).to_owned(), (*value).to_owned());
    }
    AppConfig::from_values(&values)
}
