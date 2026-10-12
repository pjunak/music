use std::error::Error;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::header::COOKIE;
use axum::http::{HeaderValue, Request, StatusCode};
use futures_util::{SinkExt, StreamExt};
use music_application::auth::{AuthRepository, UnixSeconds};
use music_application::devices::RememberedDeviceRepository;
use music_application::library::ReconciliationStatus;
use music_protocol::{ErrorCode, ServerMessage};
use music_storage::{SqliteStorage, SqliteStorageOptions};
use tempfile::tempdir;
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tower::ServiceExt;

use crate::{AppConfig, AppRuntime, RuntimeError, TEST_PASSWORD_HASH};

fn runtime_config(root: &Path) -> Result<AppConfig, RuntimeError> {
    crate::test_support::test_config(root, &[]).map_err(Into::into)
}

async fn seed_session(root: &Path, token: &str) -> Result<(), Box<dyn Error>> {
    let storage = SqliteStorage::open(SqliteStorageOptions::new(root.join("app.db"))).await?;
    let user_id = storage
        .create_user(
            "operator",
            TEST_PASSWORD_HASH,
            UnixSeconds::new(1_800_000_000),
        )
        .await?;
    AuthRepository::create_session(
        &storage,
        user_id,
        token,
        UnixSeconds::new(1_800_000_000),
        UnixSeconds::new(4_000_000_000),
    )
    .await
    .map_err(|error| -> Box<dyn Error> { error })?;
    storage.close().await;
    Ok(())
}

async fn seed_remembered_output(
    root: &Path,
    client_id: &str,
    name: &str,
) -> Result<(), Box<dyn Error>> {
    let storage = SqliteStorage::open(SqliteStorageOptions::new(root.join("app.db"))).await?;
    storage
        .upsert_device(client_id, name, true)
        .await
        .map_err(|error| -> Box<dyn Error> { error })?;
    storage.close().await;
    Ok(())
}

async fn next_protocol_message(
    socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
) -> Result<ServerMessage, Box<dyn Error>> {
    loop {
        let incoming = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "WebSocket did not publish the next protocol message within five seconds",
                )
            })?;
        let Some(incoming) = incoming else {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "WebSocket closed before the expected message",
            )
            .into());
        };
        match incoming? {
            TungsteniteMessage::Text(text) => {
                return Ok(serde_json::from_str(text.as_str())?);
            }
            TungsteniteMessage::Binary(bytes) => {
                return Ok(serde_json::from_slice(&bytes)?);
            }
            TungsteniteMessage::Ping(bytes) => {
                socket.send(TungsteniteMessage::Pong(bytes)).await?;
            }
            TungsteniteMessage::Pong(_) | TungsteniteMessage::Frame(_) => {}
            TungsteniteMessage::Close(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "WebSocket closed before the expected message",
                )
                .into());
            }
        }
    }
}

async fn wait_for_protocol_message(
    socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    description: &str,
    mut matches: impl FnMut(&ServerMessage) -> bool,
) -> Result<ServerMessage, Box<dyn Error>> {
    for _ in 0..8 {
        let message = next_protocol_message(socket).await?;
        if matches(&message) {
            return Ok(message);
        }
    }
    Err(io::Error::other(format!(
        "WebSocket did not publish {description} within eight messages"
    ))
    .into())
}

#[tokio::test]
async fn websocket_origin_gate_preserves_native_and_allowed_browsers_and_rejects_cookie_requests()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let token = "synthetic-origin-test-session";
    seed_session(directory.path(), token).await?;
    let mut config = runtime_config(directory.path())?;
    config.allowed_origins = vec!["https://controller.example".to_owned()];
    let runtime = AppRuntime::start(config).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(runtime.run(listener, async move {
        let _ = shutdown_rx.await;
        Ok(())
    }));
    let url = format!("ws://{address}/api/ws");
    let same_origin = format!("http://{address}");
    for origin in [
        None,
        Some(same_origin.as_str()),
        Some("https://controller.example"),
    ] {
        let mut request = url.as_str().into_client_request()?;
        request.headers_mut().insert(
            COOKIE,
            HeaderValue::from_str(&format!("music_session={token}"))?,
        );
        if let Some(origin) = origin {
            request
                .headers_mut()
                .insert("origin", HeaderValue::from_str(origin)?);
        }
        let (mut socket, response) = connect_async(request).await?;
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert!(matches!(
            next_protocol_message(&mut socket).await?,
            ServerMessage::StateSnapshot { .. }
        ));
        socket.close(None).await?;
    }
    for origins in [
        vec!["https://unapproved.example"],
        vec!["null"],
        vec![""],
        vec!["https://controller.example/path"],
        vec!["https://controller.example@unapproved.example"],
        vec!["https://controller.example https://unapproved.example"],
        vec!["https://controller.example", "https://unapproved.example"],
    ] {
        let mut request = url.as_str().into_client_request()?;
        request.headers_mut().insert(
            COOKIE,
            HeaderValue::from_str(&format!("music_session={token}"))?,
        );
        for origin in origins {
            request
                .headers_mut()
                .append("origin", HeaderValue::from_str(origin)?);
        }
        match connect_async(request).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                assert_eq!(response.status(), StatusCode::FORBIDDEN);
            }
            _ => return Err("unapproved Origin was not rejected before upgrade".into()),
        }
    }
    let _ = shutdown_tx.send(());
    server.await??;
    Ok(())
}

#[tokio::test]
async fn guest_socket_registers_but_cannot_control_playback() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(runtime.run(listener, async move {
        let _ = shutdown_rx.await;
        Ok(())
    }));
    let (mut socket, _) = connect_async(format!("ws://{address}/api/ws")).await?;

    let initial = next_protocol_message(&mut socket).await?;
    assert!(matches!(
        initial,
        ServerMessage::StateSnapshot {
            your_device_id,
            state,
        } if your_device_id.is_empty() && state.connected_devices.is_empty()
    ));

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({"type":"pause"}).to_string().into(),
        ))
        .await?;
    assert!(matches!(
        next_protocol_message(&mut socket).await?,
        ServerMessage::Error { detail, .. } if detail.contains("guest sessions")
    ));

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({
                "type":"register",
                "name":"Living room TV",
                "client_id":"living-room-tv",
                "protocol_version":2
            })
            .to_string()
            .into(),
        ))
        .await?;
    let mut saw_self_projection = false;
    for _ in 0..4 {
        if let ServerMessage::StateChanged { state } = next_protocol_message(&mut socket).await?
            && state.device_volumes.contains_key("living-room-tv")
            && state.connected_devices.is_empty()
        {
            saw_self_projection = true;
            break;
        }
    }
    assert!(saw_self_projection);

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({"type":"position_report","position_ms":1234})
                .to_string()
                .into(),
        ))
        .await?;
    assert!(matches!(
        next_protocol_message(&mut socket).await?,
        ServerMessage::Error { detail, .. } if detail.contains("active outputs")
    ));

    socket.close(None).await?;
    let _ = shutdown_tx.send(());
    server.await??;
    Ok(())
}

#[tokio::test]
async fn runtime_shutdown_closes_an_open_websocket_without_client_cooperation()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(runtime.run(listener, async move {
        let _ = shutdown_rx.await;
        Ok(())
    }));
    let (mut socket, _) = connect_async(format!("ws://{address}/api/ws")).await?;
    let _initial = next_protocol_message(&mut socket).await?;

    let _ = shutdown_tx.send(());
    tokio::time::timeout(Duration::from_secs(5), server).await???;

    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(message) = socket.next().await {
            match message {
                Ok(TungsteniteMessage::Close(_)) | Err(_) => return true,
                Ok(_) => {}
            }
        }
        true
    })
    .await?;
    assert!(closed);
    Ok(())
}

#[tokio::test]
async fn authenticated_socket_controls_playback_uses_remembered_defaults_and_downgrades()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let token = "authenticated-test-session-token";
    seed_session(directory.path(), token).await?;
    seed_remembered_output(directory.path(), "living-room", "Living Room TV").await?;
    let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
    let control_router = runtime.router()?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(runtime.run(listener, async move {
        let _ = shutdown_rx.await;
        Ok(())
    }));
    let mut request = format!("ws://{address}/api/ws").into_client_request()?;
    request.headers_mut().insert(
        COOKIE,
        HeaderValue::from_str(&format!("music_session={token}"))?,
    );
    let (mut socket, _) = connect_async(request).await?;
    assert!(matches!(
        next_protocol_message(&mut socket).await?,
        ServerMessage::StateSnapshot { state, .. } if state.connected_devices.is_empty()
    ));

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({
                "type":"register",
                "name":"TV Browser",
                "client_id":"living-room",
                "protocol_version":2
            })
            .to_string()
            .into(),
        ))
        .await?;
    let registered = next_protocol_message(&mut socket).await?;
    assert!(matches!(
        registered,
        ServerMessage::StateChanged { state }
            if state.active_output_device_ids == ["living-room"]
                && state.connected_devices.len() == 1
                && state.connected_devices[0].name == "Living Room TV"
                && state.connected_devices[0].is_output
    ));

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({"type":"resume"}).to_string().into(),
        ))
        .await?;
    let mut saw_playing = false;
    for _ in 0..4 {
        if matches!(
            next_protocol_message(&mut socket).await?,
            ServerMessage::StateChanged { state } if state.is_playing
        ) {
            saw_playing = true;
            break;
        }
    }
    assert!(saw_playing);

    let logout = control_router
        .oneshot(
            Request::post("/api/auth/logout")
                .header(COOKIE, format!("music_session={token}"))
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    let mut saw_revocation = false;
    let mut saw_guest_projection = false;
    for _ in 0..4 {
        match next_protocol_message(&mut socket).await? {
            ServerMessage::Error {
                code: Some(ErrorCode::SessionRevoked),
                ..
            } => saw_revocation = true,
            ServerMessage::StateChanged { state } if state.connected_devices.is_empty() => {
                saw_guest_projection = true;
            }
            _ => {}
        }
        if saw_revocation && saw_guest_projection {
            break;
        }
    }
    assert!(saw_revocation);
    assert!(saw_guest_projection);

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({"type":"pause"}).to_string().into(),
        ))
        .await?;
    let mut saw_guest_rejection = false;
    for _ in 0..3 {
        if matches!(
            next_protocol_message(&mut socket).await?,
            ServerMessage::Error { code: None, detail }
                if detail.contains("guest sessions")
        ) {
            saw_guest_rejection = true;
            break;
        }
    }
    assert!(saw_guest_rejection);

    socket.close(None).await?;
    let _ = shutdown_tx.send(());
    server.await??;
    Ok(())
}

#[tokio::test]
async fn catalog_scoped_actions_resolve_before_mutating_playback() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    fs::create_dir_all(directory.path().join("music/Album/Sub"))?;
    fs::create_dir_all(directory.path().join("music/Else"))?;
    fs::write(
        directory.path().join("music/Album/01 - First.mp3"),
        b"catalog action fixture",
    )?;
    fs::write(
        directory.path().join("music/Album/Sub/02 - Second.mp3"),
        b"catalog action fixture",
    )?;
    fs::write(
        directory.path().join("music/Else/03 - Third.mp3"),
        b"catalog action fixture",
    )?;
    fs::create_dir_all(directory.path().join("modes/table/soundboards"))?;
    fs::create_dir_all(directory.path().join("modes/table/presets"))?;
    fs::write(
        directory.path().join("modes/table/manifest.yaml"),
        "id: table\nname: Table\npanels: [soundboard]\nplaylist_categories: []\ndefault_crossfade_ms: 0\ndefault_soundboard: main\n",
    )?;
    fs::write(
        directory.path().join("modes/table/soundboards/main.yaml"),
        "id: main\nname: Main\ncategories: []\n",
    )?;
    fs::write(
        directory.path().join("modes/table/presets/warm.yaml"),
        "id: warm\nname: Warm\neffects: []\ncrossfade_ms: 750\n",
    )?;

    let token = "catalog-actions-test-session-token";
    seed_session(directory.path(), token).await?;
    let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status = runtime.library_status();
            if status.status == ReconciliationStatus::Current {
                break;
            }
            assert_ne!(status.status, ReconciliationStatus::Failed);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    let router = runtime.router()?;
    let cookie = format!("music_session={token}");
    let search = router
        .oneshot(
            Request::get("/api/library/search?q=&sort=path&order=asc")
                .header(COOKIE, &cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(search.status(), StatusCode::OK);
    let search: serde_json::Value =
        serde_json::from_slice(&to_bytes(search.into_body(), 1024 * 1024).await?)?;
    let track_ids = search["tracks"]
        .as_array()
        .ok_or("library search tracks were missing")?
        .iter()
        .map(|track| track["id"].as_i64().ok_or("track id was missing"))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(track_ids.len(), 3);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(runtime.run(listener, async move {
        let _ = shutdown_rx.await;
        Ok(())
    }));
    let mut request = format!("ws://{address}/api/ws").into_client_request()?;
    request
        .headers_mut()
        .insert(COOKIE, HeaderValue::from_str(&cookie)?);
    let (mut socket, _) = connect_async(request).await?;
    assert!(matches!(
        next_protocol_message(&mut socket).await?,
        ServerMessage::StateSnapshot { .. }
    ));

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({
                "type":"register",
                "name":"Controller",
                "client_id":"controller",
                "protocol_version":2
            })
            .to_string()
            .into(),
        ))
        .await?;
    let _ = wait_for_protocol_message(&mut socket, "the registered controller", |message| {
        matches!(message, ServerMessage::StateChanged { state }
            if state.connected_devices.iter().any(|device| device.client_id == "controller"))
    })
    .await?;

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({"type":"set_active_outputs","device_ids":["ghost"]})
                .to_string()
                .into(),
        ))
        .await?;
    assert!(matches!(
        wait_for_protocol_message(&mut socket, "the disconnected-output error", |message| {
            matches!(message, ServerMessage::Error { detail, .. }
                if detail == "device(s) not connected: ghost")
        })
        .await?,
        ServerMessage::Error { .. }
    ));

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({"type":"set_active_outputs","device_ids":["controller"]})
                .to_string()
                .into(),
        ))
        .await?;
    let _ = wait_for_protocol_message(&mut socket, "the active output", |message| {
        matches!(message, ServerMessage::StateChanged { state }
            if state.active_output_device_ids == ["controller"])
    })
    .await?;

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({"type":"set_active_mode","mode_id":"missing"})
                .to_string()
                .into(),
        ))
        .await?;
    let _ = wait_for_protocol_message(&mut socket, "the unknown-mode error", |message| {
        matches!(message, ServerMessage::Error { detail, .. }
            if detail == "unknown mode: missing")
    })
    .await?;

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({"type":"set_active_soundboard","soundboard_id":"main"})
                .to_string()
                .into(),
        ))
        .await?;
    let _ = wait_for_protocol_message(&mut socket, "the no-active-mode error", |message| {
        matches!(message, ServerMessage::Error { detail, .. }
            if detail == "no active mode; cannot set soundboard without a mode")
    })
    .await?;

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({"type":"set_active_mode","mode_id":"table"})
                .to_string()
                .into(),
        ))
        .await?;
    let _ = wait_for_protocol_message(&mut socket, "the active mode", |message| {
        matches!(message, ServerMessage::StateChanged { state }
            if state.active_mode_id.as_deref() == Some("table"))
    })
    .await?;

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({"type":"set_active_soundboard","soundboard_id":"main"})
                .to_string()
                .into(),
        ))
        .await?;
    let _ = wait_for_protocol_message(&mut socket, "the active soundboard", |message| {
        matches!(message, ServerMessage::StateChanged { state }
            if state.active_soundboard_id.as_deref() == Some("main"))
    })
    .await?;

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({"type":"set_active_presets","preset_ids":["missing"]})
                .to_string()
                .into(),
        ))
        .await?;
    let _ = wait_for_protocol_message(&mut socket, "the unknown-preset error", |message| {
        matches!(message, ServerMessage::Error { detail, .. }
            if detail == "unknown preset(s): missing")
    })
    .await?;

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({"type":"set_active_presets","preset_ids":["warm"]})
                .to_string()
                .into(),
        ))
        .await?;
    let _ = wait_for_protocol_message(&mut socket, "the effective preset", |message| {
        matches!(message, ServerMessage::StateChanged { state }
            if state.active_preset_ids == ["warm"] && state.crossfade_ms == 750)
    })
    .await?;

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({"type":"ambient_play_track","track_id":i64::MAX})
                .to_string()
                .into(),
        ))
        .await?;
    let _ = wait_for_protocol_message(&mut socket, "the missing-track error", |message| {
        matches!(message, ServerMessage::Error { detail, .. }
            if detail == "track not in library")
    })
    .await?;

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({"type":"ambient_play_track","track_id":track_ids[0]})
                .to_string()
                .into(),
        ))
        .await?;
    let _ = wait_for_protocol_message(&mut socket, "the direct ambient track", |message| {
        matches!(message, ServerMessage::StateChanged { state }
            if state.ambient.current_track_id == Some(track_ids[0]))
    })
    .await?;

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({
                "type":"ambient_set_queue",
                "track_ids":[track_ids[2], i64::MAX]
            })
            .to_string()
            .into(),
        ))
        .await?;
    let _ = wait_for_protocol_message(&mut socket, "the invalid-queue error", |message| {
        matches!(message, ServerMessage::Error { detail, .. }
            if detail == "track not in library")
    })
    .await?;

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({"type":"ambient_set_queue","track_ids":[track_ids[2]]})
                .to_string()
                .into(),
        ))
        .await?;
    let _ = wait_for_protocol_message(&mut socket, "the resolved queue", |message| {
        matches!(message, ServerMessage::StateChanged { state }
            if state.ambient.queue == [track_ids[2]])
    })
    .await?;

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({
                "type":"ambient_enqueue",
                "track_id":track_ids[1],
                "position":-1
            })
            .to_string()
            .into(),
        ))
        .await?;
    let _ = wait_for_protocol_message(&mut socket, "the clamped enqueue", |message| {
        matches!(message, ServerMessage::StateChanged { state }
            if state.ambient.queue == [track_ids[1], track_ids[2]])
    })
    .await?;

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({
                "type":"ambient_play_folder",
                "path":"/Album/",
                "start_index":1
            })
            .to_string()
            .into(),
        ))
        .await?;
    let _ = wait_for_protocol_message(&mut socket, "recursive folder playback", |message| {
        matches!(message, ServerMessage::StateChanged { state }
            if state.ambient.current_track_id == Some(track_ids[1])
                && state.ambient.history == [track_ids[0]]
                && state.ambient.queue.is_empty()
                && state.ambient.source_playlist_id.is_none())
    })
    .await?;

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({"type":"ambient_set_loop","loop":"follow"})
                .to_string()
                .into(),
        ))
        .await?;
    let _ = wait_for_protocol_message(&mut socket, "follow mode", |message| {
        matches!(message, ServerMessage::StateChanged { state }
            if state.ambient.loop_mode == music_protocol::LoopMode::Follow)
    })
    .await?;
    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({
                "type":"ambient_skip_next",
                "from_track_id":track_ids[1]
            })
            .to_string()
            .into(),
        ))
        .await?;
    let _ = wait_for_protocol_message(&mut socket, "the follow successor", |message| {
        matches!(message, ServerMessage::StateChanged { state }
            if state.ambient.current_track_id == Some(track_ids[2]))
    })
    .await?;

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({
                "type":"fire_interrupt_track",
                "track_id":track_ids[0],
                "return_to_ambient":true,
                "fade_in_ms":25,
                "fade_out_ms":50,
                "duck_to":0.4
            })
            .to_string()
            .into(),
        ))
        .await?;
    let _ = wait_for_protocol_message(&mut socket, "the resolved interrupt", |message| {
        matches!(message, ServerMessage::StateChanged { state }
            if state.interrupt.as_ref().is_some_and(|interrupt|
                interrupt.current_track_id == track_ids[0]
                    && interrupt.fade_in_ms == 25
                    && interrupt.fade_out_ms == 50
                    && interrupt.duck_to == Some(0.4)))
    })
    .await?;

    socket.close(None).await?;
    let _ = shutdown_tx.send(());
    server.await??;
    Ok(())
}

#[tokio::test]
async fn soundboard_actions_validate_exact_items_and_catalog_updates_prune_loops()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    fs::create_dir_all(directory.path().join("modes/table/soundboards"))?;
    fs::write(
        directory.path().join("modes/table/manifest.yaml"),
        "id: table\nname: Table\npanels: [soundboard]\nplaylist_categories: []\ndefault_crossfade_ms: 0\ndefault_soundboard: main\n",
    )?;
    fs::write(
        directory.path().join("modes/table/soundboards/main.yaml"),
        "id: main\nname: Main\ncategories:\n  - id: doors\n    name: Doors\n    items:\n      - file: dnd/door.ogg\n        name: Door\n",
    )?;
    let token = "soundboard-test-session-token";
    seed_session(directory.path(), token).await?;
    let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
    let control_router = runtime.router()?;
    let cookie = format!("music_session={token}");
    let active = control_router
        .clone()
        .oneshot(
            Request::put("/api/modes/active")
                .header(COOKIE, &cookie)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"mode_id":"table"}"#))?,
        )
        .await?;
    assert_eq!(active.status(), StatusCode::OK);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(runtime.run(listener, async move {
        let _ = shutdown_rx.await;
        Ok(())
    }));
    let mut request = format!("ws://{address}/api/ws").into_client_request()?;
    request
        .headers_mut()
        .insert(COOKIE, HeaderValue::from_str(&cookie)?);
    let (mut socket, _) = connect_async(request).await?;
    assert!(matches!(
        next_protocol_message(&mut socket).await?,
        ServerMessage::StateSnapshot { state, .. }
            if state.active_mode_id.as_deref() == Some("table")
    ));

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({
                "type":"fire_sfx",
                "soundboard_id":"main",
                "item_path":"dnd/missing.ogg",
                "volume":0.5
            })
            .to_string()
            .into(),
        ))
        .await?;
    let mut saw_item_rejection = false;
    for _ in 0..4 {
        if matches!(
            next_protocol_message(&mut socket).await.map_err(|error| format!("Soundboard item rejection: {error}"))?,
            ServerMessage::Error { detail, .. }
                if detail == "item 'dnd/missing.ogg' not in soundboard 'main'"
        ) {
            saw_item_rejection = true;
            break;
        }
    }
    assert!(saw_item_rejection);

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({
                "type":"fire_sfx",
                "soundboard_id":"main",
                "item_path":"dnd/door.ogg",
                "volume":0.5
            })
            .to_string()
            .into(),
        ))
        .await?;
    let mut saw_sfx = false;
    for _ in 0..4 {
        if matches!(
            next_protocol_message(&mut socket).await.map_err(|error| format!("Soundboard SFX event: {error}"))?,
            ServerMessage::SfxFired { soundboard_id, item_path, .. }
                if soundboard_id == "main" && item_path == "dnd/door.ogg"
        ) {
            saw_sfx = true;
            break;
        }
    }
    assert!(saw_sfx);

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({
                "type":"start_loop",
                "id":"rain-loop",
                "name":"Rain",
                "soundboard_id":"main",
                "item_path":"dnd/door.ogg",
                "interval_s":30.0,
                "volume":0.4
            })
            .to_string()
            .into(),
        ))
        .await?;
    let mut saw_loop = false;
    for _ in 0..4 {
        if matches!(
            next_protocol_message(&mut socket).await.map_err(|error| format!("Soundboard loop start: {error}"))?,
            ServerMessage::StateChanged { state }
                if state.looping_sfx.iter().any(|looping| looping.id == "rain-loop")
        ) {
            saw_loop = true;
            break;
        }
    }
    assert!(saw_loop);

    let deleted = control_router
        .oneshot(
            Request::delete("/api/modes/table/soundboards/main/categories/doors/items/0")
                .header(COOKIE, &cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(deleted.status(), StatusCode::OK);
    let mut saw_pruned_loop = false;
    for _ in 0..5 {
        if matches!(
            next_protocol_message(&mut socket).await.map_err(|error| format!("Soundboard loop pruning: {error}"))?,
            ServerMessage::StateChanged { state } if state.looping_sfx.is_empty()
        ) {
            saw_pruned_loop = true;
            break;
        }
    }
    assert!(saw_pruned_loop);

    socket.close(None).await?;
    let _ = shutdown_tx.send(());
    server.await??;
    Ok(())
}

#[tokio::test]
async fn automatic_playlist_http_contract_resolves_into_playback_sequences()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    fs::create_dir_all(directory.path().join("music/Album"))?;
    fs::write(
        directory.path().join("music/Album/01 - First.mp3"),
        b"playlist fixture",
    )?;
    fs::write(
        directory.path().join("music/Album/02 - Second.mp3"),
        b"playlist fixture",
    )?;
    let token = "playlist-test-session-token";
    seed_session(directory.path(), token).await?;
    let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let status = runtime.library_status();
            if status.status == ReconciliationStatus::Current {
                break;
            }
            assert_ne!(status.status, ReconciliationStatus::Failed);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    let router = runtime.router()?;
    let cookie = format!("music_session={token}");

    let search = router
        .clone()
        .oneshot(
            Request::get("/api/library/search?q=&sort=path&order=asc")
                .header(COOKIE, &cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(search.status(), StatusCode::OK);
    let search: serde_json::Value =
        serde_json::from_slice(&to_bytes(search.into_body(), 1024 * 1024).await?)?;
    let track_ids = search["tracks"]
        .as_array()
        .ok_or("library search tracks were missing")?
        .iter()
        .map(|track| track["id"].as_i64().ok_or("track id was missing"))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(track_ids.len(), 2);

    let created = router
        .clone()
        .oneshot(
            Request::post("/api/playlists")
                .header(COOKIE, &cookie)
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Quiet scenes","category":"ambient"}"#,
                ))?,
        )
        .await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: serde_json::Value =
        serde_json::from_slice(&to_bytes(created.into_body(), 1024 * 1024).await?)?;
    let playlist_id = created["id"].as_i64().ok_or("playlist id was missing")?;

    for track_id in &track_ids {
        let added = router
            .clone()
            .oneshot(
                Request::post(format!("/api/playlists/{playlist_id}/tracks"))
                    .header(COOKIE, &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"track_id":{track_id}}}"#)))?,
            )
            .await?;
        assert_eq!(added.status(), StatusCode::CREATED);
    }

    let rule = serde_json::json!({"schema":"automatic-playlist/v1"});
    let preview = router
        .clone()
        .oneshot(
            Request::post(format!("/api/playlists/{playlist_id}/automatic/preview"))
                .header(COOKIE, &cookie)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::json!({"rule":rule}).to_string()))?,
        )
        .await?;
    assert_eq!(preview.status(), StatusCode::OK);
    let preview: serde_json::Value =
        serde_json::from_slice(&to_bytes(preview.into_body(), 1024 * 1024).await?)?;
    assert_eq!(preview["library_tracks"], 2);
    assert_eq!(preview["matched_tracks"], 2);
    let signature = preview["source_signature"]
        .as_str()
        .ok_or("automatic preview signature was missing")?;
    let configured = router
        .clone()
        .oneshot(
            Request::put(format!("/api/playlists/{playlist_id}/automatic"))
                .header(COOKIE, &cookie)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"rule":rule,"source_signature":signature}).to_string(),
                ))?,
        )
        .await?;
    assert_eq!(configured.status(), StatusCode::OK);

    let managed = router
        .clone()
        .oneshot(
            Request::post(format!("/api/playlists/{playlist_id}/tracks"))
                .header(COOKIE, &cookie)
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"track_id":{}}}"#, track_ids[0])))?,
        )
        .await?;
    assert_eq!(managed.status(), StatusCode::CONFLICT);
    let managed: serde_json::Value =
        serde_json::from_slice(&to_bytes(managed.into_body(), 1024 * 1024).await?)?;
    assert_eq!(
        managed["detail"]["code"],
        "automatic_playlist_items_managed"
    );

    let exported = router
        .clone()
        .oneshot(
            Request::get(format!("/api/playlists/{playlist_id}/export"))
                .header(COOKIE, &cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(exported.status(), StatusCode::OK);
    let exported = String::from_utf8(to_bytes(exported.into_body(), 1024 * 1024).await?.to_vec())?;
    assert!(exported.starts_with("#EXTM3U\n#PLAYLIST:Quiet scenes\n"));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(runtime.run(listener, async move {
        let _ = shutdown_rx.await;
        Ok(())
    }));
    let mut request = format!("ws://{address}/api/ws").into_client_request()?;
    request
        .headers_mut()
        .insert(COOKIE, HeaderValue::from_str(&cookie)?);
    let (mut socket, _) = connect_async(request).await?;
    let _ = next_protocol_message(&mut socket).await?;
    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({
                "type":"ambient_play_playlist",
                "playlist_id":playlist_id,
                "start_index":1
            })
            .to_string()
            .into(),
        ))
        .await?;
    let mut saw_playlist = false;
    for _ in 0..4 {
        if matches!(
            next_protocol_message(&mut socket).await?,
            ServerMessage::StateChanged { state }
                if state.ambient.current_track_id == Some(track_ids[1])
                    && state.ambient.source_playlist_id == Some(playlist_id)
        ) {
            saw_playlist = true;
            break;
        }
    }
    assert!(saw_playlist);

    socket.close(None).await?;
    let _ = shutdown_tx.send(());
    server.await??;
    Ok(())
}

#[tokio::test]
async fn cue_dispatch_commits_preset_playlist_position_loops_and_sfx_together()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    fs::create_dir_all(directory.path().join("music/Arrival"))?;
    fs::write(
        directory.path().join("music/Arrival/01 - Approach.mp3"),
        b"cue fixture",
    )?;
    fs::write(
        directory.path().join("music/Arrival/02 - Reveal.mp3"),
        b"cue fixture",
    )?;
    fs::create_dir_all(directory.path().join("modes/table/soundboards"))?;
    fs::create_dir_all(directory.path().join("modes/table/presets"))?;
    fs::create_dir_all(directory.path().join("modes/table/cues"))?;
    fs::write(
        directory.path().join("modes/table/manifest.yaml"),
        "id: table\nname: Table\npanels: [soundboard, cues]\nplaylist_categories: [ambient]\ndefault_crossfade_ms: 0\ndefault_soundboard: main\n",
    )?;
    fs::write(
        directory.path().join("modes/table/soundboards/main.yaml"),
        "id: main\nname: Main\ncategories:\n  - id: scene\n    name: Scene\n    items:\n      - file: dnd/door.ogg\n        name: Door\n      - file: dnd/rain.ogg\n        name: Rain\n",
    )?;
    fs::write(
        directory.path().join("modes/table/presets/cinematic.yaml"),
        "id: cinematic\nname: Cinematic\neffects: []\ncrossfade_ms: 1500\n",
    )?;
    fs::write(
        directory.path().join("modes/table/cues/arrival.yaml"),
        "id: arrival\nname: Arrival\npreset: cinematic\nplaylist: Arrival music\nstart_index: 1\nstart_ms: 2500\nsfx:\n  - soundboard: main\n    item: dnd/door.ogg\n    volume: 0.7\nloops:\n  - soundboard: main\n    item: dnd/rain.ogg\n    interval_s: 30\n    volume: 0.4\n",
    )?;

    let token = "cue-test-session-token";
    seed_session(directory.path(), token).await?;
    let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let status = runtime.library_status();
            if status.status == ReconciliationStatus::Current {
                break;
            }
            assert_ne!(status.status, ReconciliationStatus::Failed);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    let router = runtime.router()?;
    let cookie = format!("music_session={token}");
    let search = router
        .clone()
        .oneshot(
            Request::get("/api/library/search?q=&sort=path&order=asc")
                .header(COOKIE, &cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(search.status(), StatusCode::OK);
    let search: serde_json::Value =
        serde_json::from_slice(&to_bytes(search.into_body(), 1024 * 1024).await?)?;
    let track_ids = search["tracks"]
        .as_array()
        .ok_or("library search tracks were missing")?
        .iter()
        .map(|track| track["id"].as_i64().ok_or("track id was missing"))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(track_ids.len(), 2);

    let created = router
        .clone()
        .oneshot(
            Request::post("/api/playlists")
                .header(COOKIE, &cookie)
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Arrival music","mode_id":"table","category":"ambient"}"#,
                ))?,
        )
        .await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: serde_json::Value =
        serde_json::from_slice(&to_bytes(created.into_body(), 1024 * 1024).await?)?;
    let playlist_id = created["id"].as_i64().ok_or("playlist id was missing")?;
    for track_id in &track_ids {
        let added = router
            .clone()
            .oneshot(
                Request::post(format!("/api/playlists/{playlist_id}/tracks"))
                    .header(COOKIE, &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"track_id":{track_id}}}"#)))?,
            )
            .await?;
        assert_eq!(added.status(), StatusCode::CREATED);
    }
    let active = router
        .oneshot(
            Request::put("/api/modes/active")
                .header(COOKIE, &cookie)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"mode_id":"table"}"#))?,
        )
        .await?;
    assert_eq!(active.status(), StatusCode::OK);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(runtime.run(listener, async move {
        let _ = shutdown_rx.await;
        Ok(())
    }));
    let mut request = format!("ws://{address}/api/ws").into_client_request()?;
    request
        .headers_mut()
        .insert(COOKIE, HeaderValue::from_str(&cookie)?);
    let (mut socket, _) = connect_async(request).await?;
    assert!(matches!(
        next_protocol_message(&mut socket).await?,
        ServerMessage::StateSnapshot { state, .. }
            if state.active_mode_id.as_deref() == Some("table")
    ));

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({"type":"fire_cue","cue_id":"missing"})
                .to_string()
                .into(),
        ))
        .await?;
    let mut saw_missing_cue = false;
    for _ in 0..2 {
        match next_protocol_message(&mut socket).await? {
            ServerMessage::StateChanged { .. } => {}
            ServerMessage::Error { detail, .. } => {
                assert_eq!(detail, "unknown cue 'missing' in mode 'table'");
                saw_missing_cue = true;
                break;
            }
            message => {
                return Err(io::Error::other(format!(
                    "unexpected missing-cue response: {message:?}"
                ))
                .into());
            }
        }
    }
    assert!(saw_missing_cue);

    socket
        .send(TungsteniteMessage::Text(
            serde_json::json!({"type":"fire_cue","cue_id":"arrival"})
                .to_string()
                .into(),
        ))
        .await?;
    let mut saw_atomic_state = false;
    let mut saw_sfx = false;
    for _ in 0..6 {
        match next_protocol_message(&mut socket).await? {
            ServerMessage::StateChanged { state }
                if state.active_preset_ids == ["cinematic"]
                    && state.crossfade_ms == 1_500
                    && state.ambient.current_track_id == Some(track_ids[1])
                    && state.ambient.source_playlist_id == Some(playlist_id)
                    && state.ambient.position_ms >= 2_500
                    && state.looping_sfx.len() == 1
                    && state.looping_sfx[0].id == "cue:arrival:0"
                    && state.looping_sfx[0].name == "Rain" =>
            {
                saw_atomic_state = true;
            }
            ServerMessage::SfxFired {
                soundboard_id,
                item_path,
                volume,
            } if soundboard_id == "main"
                && item_path == "dnd/door.ogg"
                && (volume - 0.7).abs() < f64::EPSILON =>
            {
                saw_sfx = true;
            }
            _ => {}
        }
        if saw_atomic_state && saw_sfx {
            break;
        }
    }
    assert!(saw_atomic_state);
    assert!(saw_sfx);

    socket.close(None).await?;
    let _ = shutdown_tx.send(());
    server.await??;
    Ok(())
}
