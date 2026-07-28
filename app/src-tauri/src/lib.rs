pub mod capture;
pub mod overlay;
pub mod commands;

use std::sync::Mutex;
use crate::capture::cursor::MonitorInfo;

pub struct CaptureState {
    pub image_bytes: Vec<u8>,
    pub monitor: Option<MonitorInfo>,
}

#[tauri::command]
fn trigger_capture(app: tauri::AppHandle, state: tauri::State<'_, Mutex<CaptureState>>) -> Result<String, String> {
    let (pos, monitor) = capture::cursor::get_cursor_and_monitor()
        .map_err(|e| format!("Failed to get cursor/monitor: {}", e))?;
    
    let (image_bytes, method) = capture::screen::capture_monitor(&monitor)
        .map_err(|e| format!("Failed to capture screen: {}", e))?;

    let method_str = match method {
        capture::screen::CaptureMethod::Wgc => "WGC",
        capture::screen::CaptureMethod::Gdi => "GDI",
    };

    {
        let mut state_lock = state.lock().unwrap();
        state_lock.image_bytes = image_bytes.clone();
        state_lock.monitor = Some(monitor.clone());
    }

    overlay::window::show_overlay(&app, &monitor, &image_bytes)
        .map_err(|e| format!("Failed to show overlay: {}", e))?;

    Ok(format!(
        "Captured {} bytes via {}. Cursor at ({}, {}). Monitor {}x{}@{}, origin ({}, {})",
        image_bytes.len(),
        method_str,
        pos.x,
        pos.y,
        monitor.width_px,
        monitor.height_px,
        monitor.dpi,
        monitor.origin_x,
        monitor.origin_y
    ))
}

use tauri::Manager;
use tauri_plugin_global_shortcut::GlobalShortcutExt;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(Mutex::new(CaptureState {
            image_bytes: Vec::new(),
            monitor: None,
        }))
        .invoke_handler(tauri::generate_handler![
            trigger_capture,
            commands::analyze::process_crop
        ])
        .setup(|app| {
            use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut, ShortcutState};
            let ctrl_alt_space = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space);
            
            app.handle().plugin(
                tauri_plugin_global_shortcut::Builder::new()
                    .with_handler(move |_app, shortcut, event| {
                        if shortcut == &ctrl_alt_space && event.state() == ShortcutState::Pressed {
                            println!("Global shortcut pressed! Triggering capture...");
                            let state = _app.state::<Mutex<CaptureState>>();
                            match trigger_capture(_app.clone(), state) {
                                Ok(res) => println!("{}", res),
                                Err(e) => eprintln!("Capture error: {}", e),
                            }
                        }
                    })
                    .build(),
            )?;
            
            app.global_shortcut().register(ctrl_alt_space)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
