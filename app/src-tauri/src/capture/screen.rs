use crate::capture::cursor::MonitorInfo;
use image::{RgbaImage, ImageBuffer};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, RGBQUAD, SRCCOPY,
};

pub enum CaptureMethod {
    Wgc,
    Gdi,
}

/// Returns the raw decoded frame, not PNG bytes — callers (marker burn-in,
/// resize) used to each decode/re-encode PNG themselves, tripling the cost of
/// an already-expensive operation in debug builds. Encode to PNG exactly once,
/// after every pixel-level step is done (see `resize::encode_png`).
pub fn capture_monitor(monitor: &MonitorInfo) -> anyhow::Result<(RgbaImage, CaptureMethod)> {
    match capture_wgc(monitor) {
        Ok(img) => Ok((img, CaptureMethod::Wgc)),
        Err(e) => {
            tracing::warn!(error = %e, "WGC capture failed, falling back to GDI BitBlt");
            let img = capture_gdi(monitor)?;
            Ok((img, CaptureMethod::Gdi))
        }
    }
}

// ---------------------------------------------------------
// WGC (Windows Graphics Capture) Path
// ---------------------------------------------------------

fn capture_wgc(_monitor: &MonitorInfo) -> anyhow::Result<RgbaImage> {
    // WGC implementation deferred. Using GDI fallback for MVP.
    Err(anyhow::anyhow!("WGC API usage needs refinement"))
}

// ---------------------------------------------------------
// GDI (BitBlt) Fallback Path
// ---------------------------------------------------------

fn capture_gdi(monitor: &MonitorInfo) -> anyhow::Result<RgbaImage> {
    unsafe {
        let hwnd = windows::Win32::Foundation::HWND(std::ptr::null_mut()); // Desktop window
        let hdc_screen = GetDC(hwnd);
        if hdc_screen.is_invalid() {
            return Err(anyhow::anyhow!("GetDC failed"));
        }

        let hdc_mem = CreateCompatibleDC(hdc_screen);
        if hdc_mem.is_invalid() {
            ReleaseDC(hwnd, hdc_screen);
            return Err(anyhow::anyhow!("CreateCompatibleDC failed"));
        }

        let hbm = CreateCompatibleBitmap(hdc_screen, monitor.width_px as i32, monitor.height_px as i32);
        if hbm.is_invalid() {
            DeleteDC(hdc_mem);
            ReleaseDC(hwnd, hdc_screen);
            return Err(anyhow::anyhow!("CreateCompatibleBitmap failed"));
        }

        let hbm_old = SelectObject(hdc_mem, hbm);

        let blt_res = BitBlt(
            hdc_mem,
            0,
            0,
            monitor.width_px as i32,
            monitor.height_px as i32,
            hdc_screen,
            monitor.origin_x,
            monitor.origin_y,
            SRCCOPY,
        );

        if let Err(e) = blt_res {
            SelectObject(hdc_mem, hbm_old);
            DeleteObject(hbm);
            DeleteDC(hdc_mem);
            ReleaseDC(hwnd, hdc_screen);
            return Err(anyhow::anyhow!("BitBlt failed: {}", e));
        }

        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: monitor.width_px as i32,
                biHeight: -(monitor.height_px as i32), // Top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            bmiColors: [RGBQUAD::default(); 1],
        };

        let mut pixels: Vec<u8> = vec![0; (monitor.width_px * monitor.height_px * 4) as usize];

        let get_di_res = GetDIBits(
            hdc_mem,
            hbm,
            0,
            monitor.height_px,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );

        SelectObject(hdc_mem, hbm_old);
        DeleteObject(hbm);
        DeleteDC(hdc_mem);
        ReleaseDC(hwnd, hdc_screen);

        if get_di_res == 0 {
            return Err(anyhow::anyhow!("GetDIBits failed"));
        }

        // Convert BGRA to RGBA
        for chunk in pixels.chunks_exact_mut(4) {
            let b = chunk[0];
            let r = chunk[2];
            chunk[0] = r;
            chunk[2] = b;
            chunk[3] = 255; // Force alpha to 255
        }

        let img: RgbaImage = ImageBuffer::from_raw(monitor.width_px, monitor.height_px, pixels)
            .ok_or_else(|| anyhow::anyhow!("Failed to create image buffer"))?;

        Ok(img)
    }
}
