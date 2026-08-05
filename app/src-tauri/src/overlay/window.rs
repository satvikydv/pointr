use tauri::{AppHandle, Manager, Emitter, WebviewWindow};
use crate::capture::cursor::MonitorInfo;
use base64::{engine::general_purpose, Engine as _};

/// Positions/sizes the (undecorated, borderless) overlay window to exactly
/// cover a monitor. Windows adds an invisible resize-border/shadow inset to
/// borderless windows even with decorations off, so a plain `set_position` +
/// `set_size` leaves the window's actual client/webview area (what
/// `window.innerWidth/innerHeight` and all our `x_norm * innerWidth` marker
/// math measure against) offset from the requested position by that inset —
/// confirmed live: outer_pos=(0,0) but inner_pos=(9,1), a visible gap at the
/// screen's left/top edge and every drawn shape skewed by the same amount.
/// Self-measures the inset instead of hardcoding it (varies by DPI/Windows
/// build) and applies a corrective second `set_position` so the *inner* area
/// lands exactly on the monitor's bounds.
fn position_over_monitor(window: &WebviewWindow, monitor: &MonitorInfo) -> anyhow::Result<()> {
    let pos = tauri::PhysicalPosition::new(monitor.origin_x, monitor.origin_y);
    let size = tauri::PhysicalSize::new(monitor.width_px, monitor.height_px);

    window.set_position(pos)?;
    window.set_size(size)?;

    if let (Ok(outer_pos), Ok(inner_pos)) = (window.outer_position(), window.inner_position()) {
        let border_dx = inner_pos.x - outer_pos.x;
        let border_dy = inner_pos.y - outer_pos.y;
        if border_dx != 0 || border_dy != 0 {
            let corrected = tauri::PhysicalPosition::new(
                monitor.origin_x - border_dx,
                monitor.origin_y - border_dy,
            );
            window.set_position(corrected)?;
        }
    }

    Ok(())
}

pub fn show_overlay(app: &AppHandle, monitor: &MonitorInfo, image_bytes: &[u8]) -> anyhow::Result<()> {
    let window = app.get_webview_window("main")
        .ok_or_else(|| anyhow::anyhow!("Main window not found"))?;

    // Convert image bytes to base64 data URI
    let b64 = general_purpose::STANDARD.encode(image_bytes);
    let data_uri = format!("data:image/png;base64,{}", b64);

    position_over_monitor(&window, monitor)?;

    // Emit event with the image to the frontend
    window.emit("show-overlay", data_uri)?;

    // Show window and focus
    window.show()?;
    window.set_focus()?;

    Ok(())
}

/// Primary hotkey path: no crop UI, just positions/sizes the (currently hidden)
/// overlay over the target monitor and tells the frontend to run the direct
/// capture -> analyze flow. The frontend keeps the window hidden until the
/// backend responds, so no image payload is needed here.
pub fn show_overlay_direct(app: &AppHandle, monitor: &MonitorInfo) -> anyhow::Result<()> {
    let window = app.get_webview_window("main")
        .ok_or_else(|| anyhow::anyhow!("Main window not found"))?;

    position_over_monitor(&window, monitor)?;

    if let (Ok(outer_pos), Ok(outer_size), Ok(inner_pos), Ok(inner_size)) = (
        window.outer_position(),
        window.outer_size(),
        window.inner_position(),
        window.inner_size(),
    ) {
        println!(
            "[overlay] after correction: outer_pos=({}, {}) outer_size={}x{} inner_pos=({}, {}) inner_size={}x{} (target pos=({}, {}) size={}x{})",
            outer_pos.x, outer_pos.y, outer_size.width, outer_size.height,
            inner_pos.x, inner_pos.y, inner_size.width, inner_size.height,
            monitor.origin_x, monitor.origin_y, monitor.width_px, monitor.height_px
        );
    }

    window.emit("show-overlay-direct", ())?;

    Ok(())
}

pub fn hide_overlay(app: &AppHandle) -> anyhow::Result<()> {
    let window = app.get_webview_window("main")
        .ok_or_else(|| anyhow::anyhow!("Main window not found"))?;
    
    window.hide()?;
    Ok(())
}
