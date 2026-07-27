use tauri::{AppHandle, Manager, Emitter};
use crate::capture::cursor::MonitorInfo;
use base64::{engine::general_purpose, Engine as _};

pub fn show_overlay(app: &AppHandle, monitor: &MonitorInfo, image_bytes: &[u8]) -> anyhow::Result<()> {
    let window = app.get_webview_window("main")
        .ok_or_else(|| anyhow::anyhow!("Main window not found"))?;
    
    // Convert image bytes to base64 data URI
    let b64 = general_purpose::STANDARD.encode(image_bytes);
    let data_uri = format!("data:image/png;base64,{}", b64);

    // Set position and size
    let pos = tauri::PhysicalPosition::new(monitor.origin_x, monitor.origin_y);
    let size = tauri::PhysicalSize::new(monitor.width_px, monitor.height_px);

    window.set_position(pos)?;
    window.set_size(size)?;
    
    // Emit event with the image to the frontend
    window.emit("show-overlay", data_uri)?;

    // Show window and focus
    window.show()?;
    window.set_focus()?;

    Ok(())
}

pub fn hide_overlay(app: &AppHandle) -> anyhow::Result<()> {
    let window = app.get_webview_window("main")
        .ok_or_else(|| anyhow::anyhow!("Main window not found"))?;
    
    window.hide()?;
    Ok(())
}
