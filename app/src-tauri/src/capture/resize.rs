use image::{imageops::FilterType, DynamicImage, RgbaImage};

const MAX_LONG_EDGE: u32 = 1568;

/// Caps the image's long edge at MAX_LONG_EDGE before it goes to the vision
/// model (PRD §6.3). Full-monitor screenshots — especially on 4K/high-DPI
/// displays — are otherwise multi-megabyte uncompressed PNGs that turn each
/// request into a multi-minute round trip instead of a few seconds. Operates
/// on the already-decoded frame and returns one too — no PNG round-trip here,
/// see `encode_png` for the single encode at the end of the pipeline.
pub fn cap_long_edge(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let long_edge = w.max(h);

    if long_edge <= MAX_LONG_EDGE {
        return img.clone();
    }

    let scale = MAX_LONG_EDGE as f32 / long_edge as f32;
    let new_w = (w as f32 * scale).round().max(1.0) as u32;
    let new_h = (h as f32 * scale).round().max(1.0) as u32;

    DynamicImage::ImageRgba8(img.clone())
        .resize(new_w, new_h, FilterType::Lanczos3)
        .to_rgba8()
}

/// The one PNG encode in the whole capture -> marker -> resize pipeline.
/// Previously each of those three steps decoded and re-encoded PNG on its
/// own — redundant work that's cheap in a release build but dominates the
/// whole hotkey-to-overlay latency in an unoptimized `tauri dev` build,
/// where `image`/`png`/`miniz_oxide` are disproportionately slow.
pub fn encode_png(img: &RgbaImage) -> anyhow::Result<Vec<u8>> {
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)?;
    Ok(buf.into_inner())
}
