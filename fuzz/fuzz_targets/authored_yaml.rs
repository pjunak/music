#![no_main]

use libfuzzer_sys::fuzz_target;
use music_media::{
    parse_cue_document, parse_mode_document, parse_preset_document, parse_soundboard_document,
};

fuzz_target!(|data: &[u8]| {
    let Ok(document) = std::str::from_utf8(data) else {
        return;
    };
    let _ = parse_mode_document(document, "fuzz");
    let _ = parse_soundboard_document(document, "fuzz");
    let _ = parse_cue_document(document, "fuzz");
    let _ = parse_preset_document(document, "fuzz");
});
