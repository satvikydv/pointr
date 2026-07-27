use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor as Win32GetDpiForMonitor, MDT_EFFECTIVE_DPI};

#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub origin_x: i32,
    pub origin_y: i32,
    pub width_px: u32,
    pub height_px: u32,
    pub dpi: u32,
}

pub fn get_cursor_and_monitor() -> anyhow::Result<(POINT, MonitorInfo)> {
    unsafe {
        let mut cursor_pos = POINT::default();
        if GetCursorPos(&mut cursor_pos).is_err() {
            return Err(anyhow::anyhow!("Failed to get cursor pos"));
        }

        let hmonitor = MonitorFromPoint(cursor_pos, MONITOR_DEFAULTTONEAREST);
        if hmonitor.is_invalid() {
            return Err(anyhow::anyhow!("Failed to get monitor from point"));
        }

        let mut monitor_info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };

        if !GetMonitorInfoW(hmonitor, &mut monitor_info).as_bool() {
            return Err(anyhow::anyhow!("Failed to get monitor info"));
        }

        let rect = monitor_info.rcMonitor;
        let width = (rect.right - rect.left).abs() as u32;
        let height = (rect.bottom - rect.top).abs() as u32;

        let mut dpi_x = 0;
        let mut dpi_y = 0;
        // Need to add Win32_UI_HiDpi to features if we want GetDpiForMonitor, but since it's already there or we can just assume 96 if it fails.
        // Let's use Win32GetDpiForMonitor if we added the feature, let's add it in Cargo.toml.
        
        let dpi = match Win32GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) {
            Ok(_) => dpi_x,
            Err(_) => 96,
        };

        Ok((
            cursor_pos,
            MonitorInfo {
                origin_x: rect.left,
                origin_y: rect.top,
                width_px: width,
                height_px: height,
                dpi,
            },
        ))
    }
}
