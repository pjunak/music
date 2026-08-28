#![no_main]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use music_domain::{LibraryPath, SfxPath};
use music_media::LibraryRoot;
use tempfile::TempDir;

struct Fixture {
    _directory: TempDir,
    root: LibraryRoot,
}

static FIXTURE: OnceLock<Option<Fixture>> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let Some((&selector, encoded)) = data.split_first() else {
        return;
    };
    // Tracked text corpus files end in LF. Strip only that corpus framing byte;
    // embedded control characters still reach the production parser unchanged.
    let encoded = encoded.strip_suffix(b"\n").unwrap_or(encoded);
    let Ok(candidate) = std::str::from_utf8(encoded) else {
        return;
    };

    let _ = SfxPath::parse(candidate.to_owned());
    let Ok(path) = LibraryPath::parse(candidate.to_owned()) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = parent.join(path.file_name());
    }

    let Some(fixture) = FIXTURE.get_or_init(build_fixture).as_ref() else {
        return;
    };
    match selector % 4 {
        0 => {
            let _ = fixture.root.resolve_existing(&path);
        }
        1 => {
            let _ = fixture.root.resolve_existing_directory(&path);
        }
        2 => {
            let _ = fixture.root.resolve_existing_file_for_mutation(&path);
        }
        _ => {
            let _ = fixture.root.resolve_for_creation(&path);
        }
    }
});

fn build_fixture() -> Option<Fixture> {
    let directory = tempfile::tempdir().ok()?;
    let root_path = directory.path().join("music");
    let inside = root_path.join("inside");
    let outside = directory.path().join("outside");
    std::fs::create_dir_all(&inside).ok()?;
    std::fs::create_dir(&outside).ok()?;
    std::fs::write(inside.join("track.flac"), b"fixture").ok()?;
    std::fs::write(outside.join("secret.flac"), b"fixture").ok()?;

    #[cfg(unix)]
    let _ = std::os::unix::fs::symlink(&outside, root_path.join("escape"));

    Some(Fixture {
        root: LibraryRoot::open(&root_path).ok()?,
        _directory: directory,
    })
}
