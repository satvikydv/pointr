// Client-side browser automation — a local Playwright driver (see
// resources/browser/browser-driver.js) spawned as a persistent subprocess
// on the USER's own machine, so a browser-based agent step opens a real,
// visible window and clicks by accessibility role+name instead of by
// guessed pixel coordinates. Deliberately NOT run on the backend: the
// backend can be a remote server (POINTR_ENV=prod), where a browser it
// opened would be invisible and useless to the user watching their own
// screen.
//
// Node.js is a documented prerequisite (Setup Guide); the `playwright`
// npm package and its browser binary are fetched lazily on first actual
// use here (`npm install` + `npx playwright install chromium` inside the
// bundled resources/browser folder), the same on-demand-fetch spirit the
// backend already uses for its MCP servers (`npx -y <pkg>`), so the
// installer itself stays small.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Manager, State};

pub struct BrowserSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

pub type BrowserState = Mutex<Option<BrowserSession>>;

/// Where the bundled driver script + package.json ship, read-only — for
/// an NSIS install this resolves under Program Files, which a
/// non-admin user cannot write into. `npm install` must NOT run here.
fn bundled_source_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .resolve("resources/browser", BaseDirectory::Resource)
        .map_err(|e| format!("Failed to resolve browser driver location: {}", e))
}

/// Where node_modules and the downloaded browser binary actually live —
/// always writable, since it's under the per-user app-config directory
/// (same one settings.json/agent_history.json already use), regardless of
/// where Pointr itself got installed.
fn writable_driver_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("Failed to resolve config dir: {}", e))?
        .join("browser-driver");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create browser driver dir: {}", e))?;
    Ok(dir)
}

/// Copies the driver script + package.json into the writable dir (only if
/// missing/stale — re-copied whenever the bundled source is newer, so an
/// app update ships driver-script fixes without the user needing to
/// clear anything manually), then runs `npm install` (only if
/// node_modules is missing) and `npx playwright install chromium` (only
/// right after a fresh install, since that's the one-time ~150-300MB
/// download). Blocking — only ever pays this cost on the first browser
/// task after an install or update, everything after is instant.
fn ensure_driver_ready(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let source = bundled_source_dir(app)?;
    let dir = writable_driver_dir(app)?;

    for name in ["browser-driver.js", "package.json"] {
        let src = source.join(name);
        let dst = dir.join(name);
        let needs_copy = match (std::fs::metadata(&src), std::fs::metadata(&dst)) {
            (Ok(s), Ok(d)) => s.modified().ok() > d.modified().ok(),
            _ => true,
        };
        if needs_copy {
            std::fs::copy(&src, &dst)
                .map_err(|e| format!("Failed to set up browser driver ({}): {}", name, e))?;
        }
    }

    if !dir.join("node_modules").exists() {
        let status = Command::new("npm")
            .args(["install", "--no-audit", "--no-fund"])
            .current_dir(&dir)
            .status()
            .map_err(|e| format!(
                "Couldn't run npm — is Node.js installed? ({})", e
            ))?;
        if !status.success() {
            return Err("npm install failed for the browser driver — check your internet connection.".to_string());
        }

        // Only needed right after a fresh install — `npx playwright
        // install chromium` downloads the actual browser binary
        // (~150-300MB), a one-time cost.
        let status = Command::new("npx")
            .args(["--yes", "playwright", "install", "chromium"])
            .current_dir(&dir)
            .status()
            .map_err(|e| format!("Couldn't run npx to install the browser binary: {}", e))?;
        if !status.success() {
            return Err("Downloading the browser binary failed — check your internet connection and try again.".to_string());
        }
    }

    Ok(dir)
}

fn send(session: &mut BrowserSession, cmd: &str, args: Value) -> Result<Value, String> {
    let id = session.next_id;
    session.next_id += 1;

    let request = json!({ "id": id, "cmd": cmd, "args": args });
    writeln!(session.stdin, "{}", request).map_err(|e| format!("Failed to send to browser driver: {}", e))?;
    session.stdin.flush().map_err(|e| format!("Failed to send to browser driver: {}", e))?;

    let mut line = String::new();
    session.stdout.read_line(&mut line).map_err(|e| format!("Failed to read from browser driver: {}", e))?;
    if line.is_empty() {
        return Err("Browser driver process ended unexpectedly.".to_string());
    }

    let response: Value = serde_json::from_str(line.trim())
        .map_err(|e| format!("Malformed response from browser driver: {} ({})", e, line.trim()))?;

    if response.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    } else {
        Err(response.get("error").and_then(Value::as_str).unwrap_or("Unknown browser driver error").to_string())
    }
}

#[tauri::command]
pub fn start_browser_session(app: AppHandle, state: State<'_, BrowserState>) -> Result<(), String> {
    let mut guard = state.lock().unwrap();
    if guard.is_some() {
        return Ok(()); // already running — open_app-equivalent idempotency
    }

    let dir = ensure_driver_ready(&app)?;

    let mut child = Command::new("node")
        .arg("browser-driver.js")
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("Couldn't start the browser driver — is Node.js installed? ({})", e))?;

    let stdin = child.stdin.take().ok_or("Failed to open browser driver stdin")?;
    let stdout = BufReader::new(child.stdout.take().ok_or("Failed to open browser driver stdout")?);

    let mut session = BrowserSession { child, stdin, stdout, next_id: 1 };
    send(&mut session, "open", json!({}))?;
    *guard = Some(session);
    Ok(())
}

#[tauri::command]
pub fn browser_navigate(url: String, state: State<'_, BrowserState>) -> Result<(), String> {
    let mut guard = state.lock().unwrap();
    let session = guard.as_mut().ok_or("No browser session open")?;
    send(session, "navigate", json!({ "url": url }))?;
    Ok(())
}

/// Returns the accessibility snapshot (URL, title, numbered ref list) as a
/// JSON string — the caller (main.js) forwards it to the backend as-is,
/// which folds it into the model's prompt as text context instead of (or
/// alongside) a pixel screenshot.
#[tauri::command]
pub fn browser_snapshot(state: State<'_, BrowserState>) -> Result<String, String> {
    let mut guard = state.lock().unwrap();
    let session = guard.as_mut().ok_or("No browser session open")?;
    let result = send(session, "snapshot", json!({}))?;
    Ok(result.to_string())
}

#[tauri::command]
pub fn browser_click(reference: String, state: State<'_, BrowserState>) -> Result<(), String> {
    let mut guard = state.lock().unwrap();
    let session = guard.as_mut().ok_or("No browser session open")?;
    send(session, "click", json!({ "ref": reference }))?;
    Ok(())
}

#[tauri::command]
pub fn browser_type(reference: String, text: String, state: State<'_, BrowserState>) -> Result<(), String> {
    let mut guard = state.lock().unwrap();
    let session = guard.as_mut().ok_or("No browser session open")?;
    send(session, "type", json!({ "ref": reference, "text": text }))?;
    Ok(())
}

#[tauri::command]
pub fn close_browser_session(state: State<'_, BrowserState>) -> Result<(), String> {
    let mut guard = state.lock().unwrap();
    if let Some(mut session) = guard.take() {
        let _ = send(&mut session, "close", json!({}));
        let _ = session.child.kill();
    }
    Ok(())
}
