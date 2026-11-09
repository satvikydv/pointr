//! Geometry for capturing an on-screen coding question (meeting/code.rs):
//! which part of the monitor to send, and at what size.
//!
//! Free of app and Windows code so it can be unit-tested directly.

/// Longest edge of the image sent for a coding question. Larger than the
/// 1568 px the everyday screen questions use: problem statements and code
/// are small text, often inside a shared screen that's already scaled down.
pub const CODE_MAX_LONG_EDGE: u32 = 2400;

/// A window's bounds in screen pixels (right/bottom exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// The part of the monitor screenshot to send: the foreground window (the
/// meeting app showing a shared screen, or the coding site), clipped to the
/// monitor, as (x, y, width, height) in image pixels. Cropping keeps the
/// text that matters at a higher effective resolution.
///
/// None means "send the whole monitor": the window isn't on this monitor,
/// covers under a quarter of it (a dialog or a small window is more likely
/// a distraction than the problem), or covers practically all of it anyway.
pub fn crop_for(window: Rect, monitor_origin: (i32, i32), monitor_size: (u32, u32)) -> Option<(u32, u32, u32, u32)> {
    let (mx, my) = monitor_origin;
    let (mw, mh) = (monitor_size.0 as i32, monitor_size.1 as i32);
    let left = window.left.max(mx);
    let top = window.top.max(my);
    let right = window.right.min(mx + mw);
    let bottom = window.bottom.min(my + mh);
    if right <= left || bottom <= top {
        return None;
    }
    let area = (right - left) as i64 * (bottom - top) as i64;
    let monitor = mw as i64 * mh as i64;
    if area * 4 < monitor || area * 100 >= monitor * 95 {
        return None;
    }
    Some(((left - mx) as u32, (top - my) as u32, (right - left) as u32, (bottom - top) as u32))
}

/// Size that fits `max_long_edge`, keeping the aspect ratio. Never upscales.
pub fn fit_long_edge(width: u32, height: u32, max_long_edge: u32) -> (u32, u32) {
    let long = width.max(height);
    if long <= max_long_edge || long == 0 {
        return (width, height);
    }
    let scale = max_long_edge as f64 / long as f64;
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: (i32, i32) = (0, 0);
    const SIZE: (u32, u32) = (1920, 1080);

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect { left, top, right, bottom }
    }

    #[test]
    fn crops_to_a_large_window() {
        assert_eq!(crop_for(rect(100, 50, 1500, 1000), MON, SIZE), Some((100, 50, 1400, 950)));
    }

    #[test]
    fn clips_to_the_monitor() {
        // Window hangs off the left and bottom edges.
        assert_eq!(crop_for(rect(-200, 100, 1300, 1300), MON, SIZE), Some((0, 100, 1300, 980)));
    }

    #[test]
    fn whole_monitor_when_crop_wouldnt_help() {
        assert_eq!(crop_for(rect(0, 0, 1920, 1080), MON, SIZE), None, "maximised");
        assert_eq!(crop_for(rect(500, 300, 900, 600), MON, SIZE), None, "small dialog");
        assert_eq!(crop_for(rect(2000, 0, 3000, 900), MON, SIZE), None, "on another monitor");
    }

    #[test]
    fn works_on_a_second_monitor() {
        // Monitor to the right of the primary one.
        let crop = crop_for(rect(2020, 40, 3700, 1000), (1920, 0), (1920, 1080));
        assert_eq!(crop, Some((100, 40, 1680, 960)));
    }

    #[test]
    fn fit_keeps_aspect_and_never_upscales() {
        assert_eq!(fit_long_edge(3840, 2160, 2400), (2400, 1350));
        assert_eq!(fit_long_edge(1080, 3000, 2400), (864, 2400));
        assert_eq!(fit_long_edge(1400, 900, 2400), (1400, 900));
    }
}
