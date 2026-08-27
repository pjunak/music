#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![forbid(unsafe_code)]

//! Public HTTP and WebSocket data-transfer contracts.
//!
//! Wire compatibility remains independent of internal domain invariants and
//! is translated explicitly at transport boundaries.

mod actions;
mod messages;
mod scalar;

pub use actions::{ClientAction, CrossfadeType, LoopMode, ShuffleMode};
pub use messages::{
    AmbientState, DeviceInfo, ErrorCode, InterruptState, LoopingSfx, PlayerState, PositionReport,
    ServerMessage,
};
pub use scalar::{
    BoundedText, CrossfadeMillis, FadeMillis, LoopIntervalSeconds, NonNegativeI64, ProtocolVersion,
    ScalarError, UnitInterval,
};
