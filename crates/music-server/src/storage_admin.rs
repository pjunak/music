use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::AppConfig;

const MAX_SEED_DEPTH: usize = 32;
const MAX_SEED_ENTRIES: usize = 10_000;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeSeedOutcome {
    NotRequested,
    NotConfigured,
    Seeded,
    TargetNotEmpty,
    SourceUnavailable,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct StorageInitializationOutcome {
    pub music_dir: PathBuf,
    pub sfx_library_dir: PathBuf,
    pub modes_dir: PathBuf,
    pub mode_seed: ModeSeedOutcome,
}

pub fn initialize_storage(
    config: &AppConfig,
    seed_modes: bool,
) -> io::Result<StorageInitializationOutcome> {
    let mut outcome = create_storage_directories(config)?;
    outcome.mode_seed = if seed_modes {
        seed_modes_if_empty(config)?
    } else {
        ModeSeedOutcome::NotRequested
    };
    Ok(outcome)
}

pub(crate) fn create_storage_directories(
    config: &AppConfig,
) -> io::Result<StorageInitializationOutcome> {
    fs::create_dir_all(&config.music_dir)?;
    fs::create_dir_all(&config.sfx_library_dir)?;
    fs::create_dir_all(&config.modes_dir)?;
    Ok(StorageInitializationOutcome {
        music_dir: config.music_dir.clone(),
        sfx_library_dir: config.sfx_library_dir.clone(),
        modes_dir: config.modes_dir.clone(),
        mode_seed: ModeSeedOutcome::NotRequested,
    })
}

pub(crate) fn seed_modes_if_empty(config: &AppConfig) -> io::Result<ModeSeedOutcome> {
    let Some(seed) = config.modes_seed_dir.as_deref() else {
        return Ok(ModeSeedOutcome::NotConfigured);
    };
    if !seed.is_dir() {
        return Ok(ModeSeedOutcome::SourceUnavailable);
    }
    if fs::read_dir(&config.modes_dir)?
        .next()
        .transpose()?
        .is_some()
    {
        return Ok(ModeSeedOutcome::TargetNotEmpty);
    }
    if fs::canonicalize(seed)? == fs::canonicalize(&config.modes_dir)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "mode seed and target are the same directory",
        ));
    }

    let mut remaining = MAX_SEED_ENTRIES;
    copy_seed_directory(seed, &config.modes_dir, 0, &mut remaining)?;
    Ok(ModeSeedOutcome::Seeded)
}

fn copy_seed_directory(
    source: &Path,
    target: &Path,
    depth: usize,
    remaining: &mut usize,
) -> io::Result<()> {
    if depth > MAX_SEED_DEPTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "mode seed exceeds the directory depth limit",
        ));
    }
    for entry in fs::read_dir(source)? {
        if *remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "mode seed exceeds the entry limit",
            ));
        }
        *remaining -= 1;
        let entry = entry?;
        let metadata = entry.file_type()?;
        let destination = target.join(entry.file_name());
        if metadata.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "mode seed contains a symbolic link",
            ));
        }
        if metadata.is_dir() {
            fs::create_dir(&destination)?;
            copy_seed_directory(&entry.path(), &destination, depth + 1, remaining)?;
        } else if metadata.is_file() {
            fs::copy(entry.path(), destination)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "mode seed contains an unsupported file type",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::tempdir;

    use super::*;

    fn config(root: &Path) -> Result<AppConfig, crate::ConfigError> {
        AppConfig::from_values(&BTreeMap::from([
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
                "MODES_SEED_DIR".to_owned(),
                root.join("seed").display().to_string(),
            ),
        ]))
    }

    #[test]
    fn initializes_all_roots_and_only_seeds_an_empty_target()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        fs::create_dir_all(directory.path().join("seed/table"))?;
        fs::write(
            directory.path().join("seed/table/manifest.yaml"),
            "id: table\nname: Table\n",
        )?;
        let config = config(directory.path())?;

        let first = initialize_storage(&config, true)?;
        assert_eq!(first.mode_seed, ModeSeedOutcome::Seeded);
        assert!(config.music_dir.is_dir());
        assert!(config.sfx_library_dir.is_dir());
        assert!(config.modes_dir.join("table/manifest.yaml").is_file());

        let second = initialize_storage(&config, true)?;
        assert_eq!(second.mode_seed, ModeSeedOutcome::TargetNotEmpty);
        Ok(())
    }
}
