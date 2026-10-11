# Compatibility fixtures

Test inputs that pin behaviour existing data and clients depend on. Rust tests
read them directly; they are not regenerated.

| File | Pins |
|---|---|
| `websocket-actions.examples.json`, `websocket-messages.examples.json` | Valid and rejected WebSocket frames that browsers, outputs and Baton exchange |
| `metadata.examples.json` | Tag read/write results for the supported audio formats |
| `authored-files.examples.json` | Mode, soundboard, cue and preset YAML round trips |
| `crypto-vectors.json` | Stored credential ciphertext and password hashes that must stay readable |

Generated Rust contracts live in `../generated/`; refresh them with
`cargo run --locked -p music-server --bin music-cli -- contracts export --root .`.
