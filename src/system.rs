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

/// Makes a frameless window draw exactly into the area it shows. For the
/// shadow and border, winit moves a frameless window's drawing area down
/// 1 px but keeps its height, so its bottom row hangs past the window and
/// Windows squeezes every frame 1 px to fit, which blurs all text a little
/// (measured on a capture: rows drifting by up to a pixel). Here the drawing
/// area ends at the window's bottom instead. Maximized windows are left
/// alone; winit fits those to the screen exactly.
#[cfg(windows)]
pub fn fit_drawing_area(window: &dyn iced::window::Window) {
    use raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsZoomed, NCCALCSIZE_PARAMS, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
        SetWindowPos, WM_NCCALCSIZE,
    };

    unsafe extern "system" fn fit(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _: usize, _: usize) -> LRESULT {
        let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
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
