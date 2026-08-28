#![no_main]

use libfuzzer_sys::fuzz_target;
use music_server::exercise_authoring_import_parser;

fuzz_target!(|data: &[u8]| {
    exercise_authoring_import_parser(data);
});
