use image::imageops::FilterType;

const MAX_LONG_EDGE: u32 = 1568;

/// Caps the image's long edge at MAX_LONG_EDGE before it goes to the vision
/// model (PRD §6.3). Full-monitor screenshots — especially on 4K/high-DPI
/// displays — are otherwise multi-megabyte uncompressed PNGs that turn each
/// request into a multi-minute round trip instead of a few seconds.
pub fn cap_long_edge(png_bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let img = image::load_from_memory(png_bytes)?;
    let (w, h) = (img.width(), img.height());
    let long_edge = w.max(h);

    if long_edge <= MAX_LONG_EDGE {
        return Ok(png_bytes.to_vec());
    }

    let scale = MAX_LONG_EDGE as f32 / long_edge as f32;
    let new_w = (w as f32 * scale).round().max(1.0) as u32;
    let new_h = (h as f32 * scale).round().max(1.0) as u32;

    let resized = img.resize(new_w, new_h, FilterType::Lanczos3);

    let mut buf = std::io::Cursor::new(Vec::new());
    resized.write_to(&mut buf, image::ImageFormat::Png)?;
    Ok(buf.into_inner())
}
