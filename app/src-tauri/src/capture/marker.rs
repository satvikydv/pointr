use image::{Rgba, RgbaImage};

const MARKER_COLOR: Rgba<u8> = Rgba([255, 0, 0, 255]);
const MARKER_RADIUS: i64 = 14;
const MARKER_THICKNESS: i64 = 3;

/// Burns a red crosshair + ring into the raw frame at the given pixel
/// position, so the vision model sees exactly where the cursor was at
/// capture time. Mutates in place — no PNG decode/encode here, that happens
/// once at the very end of the pipeline (see `resize::encode_png`).
pub fn draw_cursor_marker(img: &mut RgbaImage, center_x: i64, center_y: i64) {
    for dx in -MARKER_RADIUS..=MARKER_RADIUS {
        for t in 0..MARKER_THICKNESS {
            put_if_in_bounds(img, center_x + dx, center_y - MARKER_THICKNESS / 2 + t, MARKER_COLOR);
        }
    }
    for dy in -MARKER_RADIUS..=MARKER_RADIUS {
        for t in 0..MARKER_THICKNESS {
            put_if_in_bounds(img, center_x - MARKER_THICKNESS / 2 + t, center_y + dy, MARKER_COLOR);
        }
    }

    let steps = 96;
    for i in 0..steps {
        let theta = (i as f32) / (steps as f32) * std::f32::consts::TAU;
        let px = center_x + (MARKER_RADIUS as f32 * theta.cos()) as i64;
        let py = center_y + (MARKER_RADIUS as f32 * theta.sin()) as i64;
        put_if_in_bounds(img, px, py, MARKER_COLOR);
        put_if_in_bounds(img, px + 1, py, MARKER_COLOR);
        put_if_in_bounds(img, px, py + 1, MARKER_COLOR);
    }
}

fn put_if_in_bounds(img: &mut RgbaImage, x: i64, y: i64, color: Rgba<u8>) {
    let (w, h) = img.dimensions();
    if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
        img.put_pixel(x as u32, y as u32, color);
    }
}
