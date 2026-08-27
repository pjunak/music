"""Synthetic, non-private compatibility cases for the Rust rewrite oracle."""

from typing import Any

VALID_WEBSOCKET_ACTIONS: list[dict[str, Any]] = [
    {
        "id": "register-default-protocol",
        "input": {"type": "register", "name": "Reference output", "client_id": "ref-output"},
    },
    {
        "id": "register-v2",
        "input": {
            "type": "register",
            "name": "Reference controller",
            "client_id": "ref-controller",
            "protocol_version": 2,
        },
    },
    {"id": "set-volume-zero", "input": {"type": "set_volume", "volume": 0.0}},
    {"id": "pause", "input": {"type": "pause"}},
    {"id": "resume", "input": {"type": "resume"}},
    {
        "id": "set-active-mode-null",
        "input": {"type": "set_active_mode", "mode_id": None},
    },
    {
        "id": "set-active-outputs",
        "input": {"type": "set_active_outputs", "device_ids": ["ref-output", "ref-tv"]},
    },
    {
        "id": "set-device-volume-max",
        "input": {"type": "set_device_volume", "device_id": "ref-output", "volume": 1.0},
    },
    {
        "id": "position-report",
        "input": {"type": "position_report", "position_ms": 12345},
    },
    {
        "id": "ambient-play-track",
        "input": {"type": "ambient_play_track", "track_id": 41},
    },
    {
        "id": "ambient-set-queue",
        "input": {"type": "ambient_set_queue", "track_ids": [41, 42]},
    },
    {
        "id": "ambient-jump-queue",
        "input": {"type": "ambient_jump_queue", "position": 1},
    },
    {
        "id": "ambient-enqueue-default-position",
        "input": {"type": "ambient_enqueue", "track_id": 43},
    },
    {"id": "ambient-clear-queue", "input": {"type": "ambient_clear_queue"}},
    {
        "id": "ambient-skip-next-observed",
        "input": {"type": "ambient_skip_next", "from_track_id": 41},
    },
    {"id": "ambient-skip-prev", "input": {"type": "ambient_skip_prev"}},
    {"id": "ambient-seek-zero", "input": {"type": "ambient_seek", "position_ms": 0}},
    {
        "id": "ambient-set-loop",
        "input": {"type": "ambient_set_loop", "loop": "follow"},
    },
    {
        "id": "ambient-set-shuffle",
        "input": {"type": "ambient_set_shuffle", "shuffle": "random"},
    },
    {"id": "ambient-stop", "input": {"type": "ambient_stop"}},
    {
        "id": "ambient-play-playlist-default-index",
        "input": {"type": "ambient_play_playlist", "playlist_id": 7},
    },
    {
        "id": "ambient-play-folder-root",
        "input": {"type": "ambient_play_folder"},
    },
    {
        "id": "set-active-soundboard-null",
        "input": {"type": "set_active_soundboard", "soundboard_id": None},
    },
    {
        "id": "set-active-presets",
        "input": {"type": "set_active_presets", "preset_ids": ["cave", "radio"]},
    },
    {
        "id": "set-crossfade-default-type",
        "input": {"type": "set_crossfade", "crossfade_ms": 1500},
    },
    {
        "id": "fire-interrupt-track-defaults",
        "input": {"type": "fire_interrupt_track", "track_id": 99},
    },
    {
        "id": "fire-interrupt-playlist-full",
        "input": {
            "type": "fire_interrupt_playlist",
            "playlist_id": 9,
            "return_to_ambient": False,
            "fade_in_ms": 250,
            "fade_out_ms": 500,
            "duck_to": 0.25,
        },
    },
    {
        "id": "interrupt-skip-next-default",
        "input": {"type": "interrupt_skip_next"},
    },
    {
        "id": "interrupt-seek",
        "input": {"type": "interrupt_seek", "position_ms": 321},
    },
    {"id": "cancel-interrupt", "input": {"type": "cancel_interrupt"}},
    {
        "id": "fire-sfx-default-volume",
        "input": {"type": "fire_sfx", "soundboard_id": "tavern", "item_path": "doors/0"},
    },
    {
        "id": "start-loop",
        "input": {
            "type": "start_loop",
            "id": "rain-loop",
            "name": "Rain",
            "soundboard_id": "weather",
            "item_path": "rain/0",
            "interval_s": 12.5,
        },
    },
    {"id": "stop-loop", "input": {"type": "stop_loop", "id": "rain-loop"}},
    {"id": "fire-cue", "input": {"type": "fire_cue", "cue_id": "kraken"}},
]


INVALID_WEBSOCKET_ACTIONS: list[dict[str, Any]] = [
    {"id": "missing-type", "input": {}},
    {"id": "unknown-type", "input": {"type": "not_an_action"}},
    {
        "id": "missing-required-nullable-mode",
        "input": {"type": "set_active_mode"},
    },
    {
        "id": "missing-required-nullable-soundboard",
        "input": {"type": "set_active_soundboard"},
    },
    {
        "id": "register-empty-name",
        "input": {"type": "register", "name": "", "client_id": "valid"},
    },
    {
        "id": "register-zero-protocol",
        "input": {"type": "register", "name": "Valid", "client_id": "valid", "protocol_version": 0},
    },
    {"id": "volume-below-zero", "input": {"type": "set_volume", "volume": -0.01}},
    {
        "id": "device-volume-above-one",
        "input": {"type": "set_device_volume", "device_id": "valid", "volume": 1.01},
    },
    {
        "id": "negative-position-report",
        "input": {"type": "position_report", "position_ms": -1},
    },
    {
        "id": "negative-queue-position",
        "input": {"type": "ambient_jump_queue", "position": -1},
    },
    {
        "id": "invalid-loop-mode",
        "input": {"type": "ambient_set_loop", "loop": "weighted"},
    },
    {
        "id": "invalid-shuffle-mode",
        "input": {"type": "ambient_set_shuffle", "shuffle": "weighted"},
    },
    {
        "id": "crossfade-too-long",
        "input": {"type": "set_crossfade", "crossfade_ms": 30001},
    },
    {
        "id": "invalid-crossfade-type",
        "input": {"type": "set_crossfade", "crossfade_ms": 0, "crossfade_type": "logarithmic"},
    },
    {
        "id": "interrupt-duck-above-one",
        "input": {"type": "fire_interrupt_track", "track_id": 1, "duck_to": 1.01},
    },
    {
        "id": "interrupt-fade-too-long",
        "input": {"type": "fire_interrupt_playlist", "playlist_id": 1, "fade_in_ms": 10001},
    },
    {
        "id": "sfx-empty-path",
        "input": {"type": "fire_sfx", "soundboard_id": "valid", "item_path": ""},
    },
    {
        "id": "loop-interval-too-short",
        "input": {
            "type": "start_loop",
            "id": "valid",
            "name": "Valid",
            "soundboard_id": "valid",
            "item_path": "valid",
            "interval_s": 0.5,
        },
    },
    {"id": "stop-loop-empty-id", "input": {"type": "stop_loop", "id": ""}},
    {"id": "fire-cue-empty-id", "input": {"type": "fire_cue", "cue_id": ""}},
]


VALID_WEBSOCKET_MESSAGES: list[dict[str, Any]] = [
    {
        "id": "state-snapshot-defaults",
        "input": {"type": "state_snapshot", "state": {}},
    },
    {
        "id": "state-changed-populated",
        "input": {
            "type": "state_changed",
            "state": {
                "revision": 17,
                "position_epoch": 4,
                "is_playing": True,
                "active_mode_id": "dnd",
                "active_output_device_ids": ["ref-output"],
                "device_volumes": {"ref-output": 0.75},
                "active_soundboard_id": "tavern",
                "active_preset_ids": ["cave"],
                "preset_revision": 3,
                "crossfade_ms": 1500,
                "crossfade_type": "equal_power",
                "ambient": {
                    "current_track_id": 41,
                    "queue": [42],
                    "history": [40],
                    "position_ms": 1234,
                    "position_anchored_at": 1000.5,
                    "loop": "queue",
                    "shuffle": "random",
                    "source_playlist_id": 7,
                },
                "interrupt": {
                    "current_track_id": 99,
                    "queue": [100],
                    "position_ms": 500,
                    "position_anchored_at": 1001.0,
                    "return_to_ambient": False,
                    "fade_in_ms": 250,
                    "fade_out_ms": 500,
                    "duck_to": 0.25,
                },
                "looping_sfx": [
                    {
                        "id": "rain-loop",
                        "name": "Rain",
                        "soundboard_id": "weather",
                        "item_path": "rain/0",
                        "interval_s": 12.5,
                    }
                ],
                "last_position_report": {
                    "device_id": "ref-output",
                    "position_ms": 500,
                    "reported_at": 1001.25,
                },
                "connected_devices": [
                    {
                        "device_id": "ref-output",
                        "client_id": "ref-output",
                        "name": "Reference output",
                    }
                ],
            },
        },
    },
    {
        "id": "sfx-fired-default-volume",
        "input": {"type": "sfx_fired", "soundboard_id": "tavern", "item_path": "doors/0"},
    },
    {
        "id": "error-default-code",
        "input": {"type": "error", "detail": "request rejected"},
    },
    {
        "id": "error-session-expired",
        "input": {"type": "error", "detail": "session expired", "code": "session_expired"},
    },
]


INVALID_WEBSOCKET_MESSAGES: list[dict[str, Any]] = [
    {"id": "message-missing-type", "input": {}},
    {"id": "message-unknown-type", "input": {"type": "unknown"}},
    {"id": "snapshot-missing-state", "input": {"type": "state_snapshot"}},
    {"id": "changed-missing-state", "input": {"type": "state_changed"}},
    {"id": "sfx-missing-item-path", "input": {"type": "sfx_fired", "soundboard_id": "valid"}},
    {"id": "error-missing-detail", "input": {"type": "error"}},
    {
        "id": "error-invalid-code",
        "input": {"type": "error", "detail": "invalid", "code": "other"},
    },
    {
        "id": "state-invalid-crossfade",
        "input": {"type": "state_changed", "state": {"crossfade_type": "logarithmic"}},
    },
]
