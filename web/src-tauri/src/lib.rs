//! Postern Console (Tauri) — the default delivery shell.
//!
//! The web frontend is transport-agnostic (one Transport seam); under
//! `VITE_TARGET=tauri` it dispatches every `/v1/*` call through
//! `invoke('control_request', …)`. This Rust side is the only place that knows
//! the backend lives on a Unix socket: it does a one-shot HTTP/1.1-over-UDS
//! round-trip to `control.sock` (same shape as postern-cli's hyperlocal client
//! — single connection, no pool, no keep-alive, no retry) and hands the raw
//! {status, ok, text} back to the frontend's error/contract layer.
//!
//! Socket path: `POSTERN_CONTROL_SOCK`, else `$XDG_RUNTIME_DIR/postern/control.sock`,
//! else `/run/postern/control.sock` (mirrors the CLI). Control token: the file at
//! `POSTERN_CONTROL_TOKEN` (read per request, sent as `x-postern-control-token`).

use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::rt::TokioIo;
use serde::Serialize;

/// Connect/read timeout for the one-shot round-trip; timeouts surface as an error.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Raw response handed back to the frontend Transport (matches `RawResponse`:
/// `{status, ok, text}`). The frontend's `client.ts` does the ApiError/409
/// normalization and id-safe JSON parse — this side never interprets the body.
#[derive(Serialize)]
struct ControlResponse {
    status: u16,
    ok: bool,
    text: String,
}

/// Resolve `control.sock` path (mirrors postern-cli `control_socket_path`).
fn control_sock_path() -> PathBuf {
    if let Ok(p) = std::env::var("POSTERN_CONTROL_SOCK") {
        return PathBuf::from(p);
    }
    if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(rt).join("postern").join("control.sock");
    }
    PathBuf::from("/run/postern/control.sock")
}

/// Read the control token from the file at `POSTERN_CONTROL_TOKEN` (if any).
fn control_token() -> Option<String> {
    let path = std::env::var("POSTERN_CONTROL_TOKEN").ok()?;
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// One-shot HTTP-over-UDS round-trip to the control plane. `path` is the full
/// `/v1/...` request target (frontend prepends `API_BASE`). Any connect/IO
/// failure or timeout becomes `Err(String)` (fail-closed — the frontend surfaces
/// it as an error, never a fabricated success).
#[tauri::command]
async fn control_request(
    method: String,
    path: String,
    body: Option<serde_json::Value>,
) -> Result<ControlResponse, String> {
    let sock = control_sock_path();

    // Assemble the request: method + origin-form /v1 target + optional JSON body
    // + the control-token second factor.
    let body_bytes = match &body {
        Some(v) => serde_json::to_vec(v).map_err(|e| e.to_string())?,
        None => Vec::new(),
    };
    let mut builder = Request::builder().method(method.as_str()).uri(&path);
    if body.is_some() {
        builder = builder.header(hyper::header::CONTENT_TYPE, "application/json");
    }
    if let Some(tok) = control_token() {
        builder = builder.header("x-postern-control-token", tok);
    }
    let request = builder
        .body(Full::new(Bytes::from(body_bytes)))
        .map_err(|e| format!("bad request: {e}"))?;

    // New UnixStream per call — single connection, no pool/keep-alive/retry.
    let stream = tokio::time::timeout(TIMEOUT, tokio::net::UnixStream::connect(&sock))
        .await
        .map_err(|_| "daemon unreachable (connect timeout)".to_string())?
        .map_err(|e| format!("daemon unreachable: {e}"))?;

    let (mut sender, conn) =
        tokio::time::timeout(TIMEOUT, hyper::client::conn::http1::handshake(TokioIo::new(stream)))
            .await
            .map_err(|_| "daemon unreachable (handshake timeout)".to_string())?
            .map_err(|e| format!("daemon unreachable: {e}"))?;

    let driver = tokio::spawn(async move {
        let _ = conn.await;
    });

    let response = tokio::time::timeout(TIMEOUT, sender.send_request(request))
        .await
        .map_err(|_| "daemon unreachable (send timeout)".to_string())?
        .map_err(|e| format!("daemon unreachable: {e}"))?;

    let status = response.status().as_u16();
    let ok = response.status().is_success();
    let collected = tokio::time::timeout(TIMEOUT, response.into_body().collect())
        .await
        .map_err(|_| "daemon unreachable (read timeout)".to_string())?
        .map_err(|e| format!("daemon unreachable: {e}"))?;
    let text = String::from_utf8_lossy(&collected.to_bytes()).to_string();

    // Drop the sender and stop the driver — connection closes (no keep-alive).
    drop(sender);
    driver.abort();

    Ok(ControlResponse { status, ok, text })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![control_request])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
