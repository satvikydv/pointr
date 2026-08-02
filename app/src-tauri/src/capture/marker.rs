use image::{Rgba, RgbaImage};

const MARKER_COLOR: Rgba<u8> = Rgba([255, 0, 0, 255]);
const MARKER_RADIUS: i64 = 14;
const MARKER_THICKNESS: i64 = 3;

/// Burns a red crosshair + ring into the PNG at the given pixel position, so the
/// vision model sees exactly where the cursor was at capture time.
pub fn draw_cursor_marker(png_bytes: &[u8], center_x: i64, center_y: i64) -> anyhow::Result<Vec<u8>> {
    let mut img: RgbaImage = image::load_from_memory(png_bytes)?.to_rgba8();

    for dx in -MARKER_RADIUS..=MARKER_RADIUS {
        for t in 0..MARKER_THICKNESS {
            put_if_in_bounds(&mut img, center_x + dx, center_y - MARKER_THICKNESS / 2 + t, MARKER_COLOR);
        }
    }
    for dy in -MARKER_RADIUS..=MARKER_RADIUS {
        for t in 0..MARKER_THICKNESS {
            put_if_in_bounds(&mut img, center_x - MARKER_THICKNESS / 2 + t, center_y + dy, MARKER_COLOR);
        }
    }

    let steps = 96;
    for i in 0..steps {
        let theta = (i as f32) / (steps as f32) * std::f32::consts::TAU;
        let px = center_x + (MARKER_RADIUS as f32 * theta.cos()) as i64;
        let py = center_y + (MARKER_RADIUS as f32 * theta.sin()) as i64;
        put_if_in_bounds(&mut img, px, py, MARKER_COLOR);
        put_if_in_bounds(&mut img, px + 1, py, MARKER_COLOR);
        put_if_in_bounds(&mut img, px, py + 1, MARKER_COLOR);
    }

    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)?;
    Ok(buf.into_inner())
}

fn put_if_in_bounds(img: &mut RgbaImage, x: i64, y: i64, color: Rgba<u8>) {
    let (w, h) = img.dimensions();
    if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
        img.put_pixel(x as u32, y as u32, color);
    }
}
