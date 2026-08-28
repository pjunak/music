#![no_main]

use libfuzzer_sys::fuzz_target;
use music_application::assistant::exercise_structured_model_outputs;

fuzz_target!(|data: &[u8]| {
    exercise_structured_model_outputs(data);
});
