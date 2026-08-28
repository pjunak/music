#![no_main]

use libfuzzer_sys::fuzz_target;
use music_protocol::{ClientAction, ServerMessage};

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<ClientAction>(data);
    let _ = serde_json::from_slice::<ServerMessage>(data);
});
