//! Small things asked of the operating system.

/// Opens a web or email link from a PDF in the default app. Other kinds of
/// links (files, scripts) are ignored, as in pdf.js.
pub fn open_link(url: &str) {
    let lower = url.to_ascii_lowercase();
    if !["http://", "https://", "mailto:"].iter().any(|scheme| lower.starts_with(scheme)) {
        return;
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let wide = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (verb, url) = (wide("open"), wide(url));
        unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                url.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// Whether the system wants animations, from Windows' "Animation effects"
/// switch. Other systems answer yes for now.
pub fn animations() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW};
        let mut on: windows_sys::core::BOOL = 1;
        unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut on).cast(), 0) };
        on != 0
    }
    #[cfg(not(windows))]
    true
}

/// Restores native edge resizing to our frameless Windows window and makes
/// it draw exactly into the area it shows. For the
/// shadow and border, winit moves a frameless window's drawing area down
/// 1 px but keeps its height, so its bottom row hangs past the window and
/// Windows squeezes every frame 1 px to fit, which blurs all text a little
/// (measured on a capture: rows drifting by up to a pixel). Here the drawing
/// area ends at the window's bottom instead. Maximized windows are left
/// alone; winit fits those to the screen exactly.
#[cfg(windows)]
pub fn configure_window(window: &dyn iced::window::Window) {
    use raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetWindowLongPtrW, GetWindowRect, HTCLIENT, IsZoomed, NCCALCSIZE_PARAMS,
        SM_CXPADDEDBORDER, SM_CXSIZEFRAME, SM_CYSIZEFRAME, SWP_FRAMECHANGED, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos, WM_NCCALCSIZE, WM_NCHITTEST, WS_THICKFRAME,
    };

    unsafe extern "system" fn fit(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _: usize, _: usize) -> LRESULT {
        let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
        if msg == WM_NCHITTEST
            && result == HTCLIENT as LRESULT
            && unsafe { IsZoomed(hwnd) } == 0
            && unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32 & WS_THICKFRAME != 0
        {
            let mut rect = Default::default();
            if unsafe { GetWindowRect(hwnd, &mut rect) } != 0 {
                let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
                let padding = unsafe { GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) };
                let border_x = (unsafe { GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) } + padding).max(1);
                let border_y = (unsafe { GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi) } + padding).max(1);
                // WM_NCHITTEST packs signed screen coordinates, including
                // negative coordinates on monitors left of or above the primary.
                let x = lparam as i16 as i32;
                let y = (lparam >> 16) as i16 as i32;
                if let Some(hit) = resize_hit_test(rect, x, y, border_x, border_y) {
                    return hit as LRESULT;
                }
            }
        }
        if msg == WM_NCCALCSIZE && wparam != 0 && unsafe { IsZoomed(hwnd) } == 0 {
            // SAFETY: with wparam set, lparam points at NCCALCSIZE_PARAMS.
            let params = unsafe { &mut *(lparam as *mut NCCALCSIZE_PARAMS) };
            params.rgrc[0].bottom -= 1;
        }
        result
    }

    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else { return };
    let hwnd = handle.hwnd.get() as HWND;
    unsafe {
        SetWindowSubclass(hwnd, Some(fit), 1, 0);
        // Has Windows measure the drawing area again, now with the fix.
        let flags = SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE;
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, flags);
    }
}

#[cfg(windows)]
fn resize_hit_test(
    rect: windows_sys::Win32::Foundation::RECT,
    x: i32,
    y: i32,
    border_x: i32,
    border_y: i32,
) -> Option<u32> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT,
    };
    if x < rect.left || x >= rect.right || y < rect.top || y >= rect.bottom {
        return None;
    }
    let left = x < rect.left + border_x;
    let right = x >= rect.right - border_x;
    let top = y < rect.top + border_y;
    let bottom = y >= rect.bottom - border_y;
    match (left, right, top, bottom) {
        (true, _, true, _) => Some(HTTOPLEFT),
        (_, true, true, _) => Some(HTTOPRIGHT),
        (true, _, _, true) => Some(HTBOTTOMLEFT),
        (_, true, _, true) => Some(HTBOTTOMRIGHT),
        (true, _, _, _) => Some(HTLEFT),
        (_, true, _, _) => Some(HTRIGHT),
        (_, _, true, _) => Some(HTTOP),
        (_, _, _, true) => Some(HTBOTTOM),
        _ => None,
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::resize_hit_test;
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT,
    };

    #[test]
    fn all_edges_and_corners_resize_at_different_scales_and_monitor_positions() {
        for (origin_x, origin_y, border) in [(100, 100, 8), (-1200, -900, 12)] {
            let rect = RECT { left: origin_x, top: origin_y, right: origin_x + 800, bottom: origin_y + 600 };
            for (x, y, hit) in [
                (0, 0, HTTOPLEFT), (400, 0, HTTOP), (799, 0, HTTOPRIGHT),
                (0, 300, HTLEFT), (799, 300, HTRIGHT),
                (0, 599, HTBOTTOMLEFT), (400, 599, HTBOTTOM), (799, 599, HTBOTTOMRIGHT),
            ] {
                assert_eq!(resize_hit_test(rect, origin_x + x, origin_y + y, border, border), Some(hit));
            }
            assert_eq!(resize_hit_test(rect, origin_x + border - 1, origin_y + 300, border, border), Some(HTLEFT));
            assert_eq!(resize_hit_test(rect, origin_x + border, origin_y + 300, border, border), None);
            assert_eq!(resize_hit_test(rect, origin_x + 400, origin_y + 300, border, border), None);
            for (x, y) in [(-1, 300), (800, 300), (400, -1), (400, 600)] {
                assert_eq!(resize_hit_test(rect, origin_x + x, origin_y + y, border, border), None);
            }
        }
    }
}
