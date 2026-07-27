use crate::capture::cursor::MonitorInfo;

pub struct NormalizedCursor { pub x_norm: f32, pub y_norm: f32 }

pub fn normalize_cursor(cursor_x: i32, cursor_y: i32, monitor: &MonitorInfo) -> NormalizedCursor {
    let rel_x = (cursor_x - monitor.origin_x) as f32;
    let rel_y = (cursor_y - monitor.origin_y) as f32;
    NormalizedCursor {
        x_norm: (rel_x / monitor.width_px as f32).clamp(0.0, 1.0),
        y_norm: (rel_y / monitor.height_px as f32).clamp(0.0, 1.0),
    }
}
