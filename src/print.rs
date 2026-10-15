//! Hands a PDF to the operating system's printing.

#[cfg(windows)]
pub use windows::{Job, ask, owner, print};

/// Opens the native print sheet with a preview.
#[cfg(target_os = "macos")]
pub fn print(window: &dyn iced::window::Window, pdf: Vec<u8>) -> Result<(), String> {
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{NSPrintInfo, NSView};
    use objc2_foundation::NSData;
    use objc2_pdf_kit::{PDFDocument, PDFPrintScalingMode};
    use raw_window_handle::RawWindowHandle;

    let mtm = MainThreadMarker::new().ok_or("Printing must start on the main thread.")?;
    let RawWindowHandle::AppKit(handle) = window.window_handle().map_err(|e| e.to_string())?.as_raw()
    else {
        return Err("Unexpected window type.".into());
    };
    // SAFETY: AppKit window handles point at a live NSView on the main thread.
    let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
    let ns_window = view.window().ok_or("The window is gone.")?;

    let data = NSData::with_bytes(&pdf);
    let document = unsafe { PDFDocument::initWithData(PDFDocument::alloc(), &data) }
        .ok_or("macOS couldn't read the document for printing.")?;
    let operation = unsafe {
        document.printOperationForPrintInfo_scalingMode_autoRotate(
            Some(&NSPrintInfo::sharedPrintInfo()),
            PDFPrintScalingMode::PageScaleDownToFit,
            true,
            mtm,
        )
    }
    .ok_or("macOS couldn't prepare the print job.")?;
    unsafe {
        operation.runOperationModalForWindow_delegate_didRunSelector_contextInfo(
            &ns_window,
            None,
            None,
            std::ptr::null_mut(),
        );
    }
    Ok(())
}

/// Sends the file to the default printer.
#[cfg(not(any(target_os = "macos", windows)))]
pub fn print(_window: &dyn iced::window::Window, pdf: Vec<u8>) -> Result<(), String> {
    let path = std::env::temp_dir().join("rs-pdf-render-print.pdf");
    std::fs::write(&path, pdf).map_err(|e| e.to_string())?;
    std::process::Command::new("lp")
        .arg(&path)
        .spawn()
        .map(drop)
        .map_err(|e| format!("Couldn't start printing ({e})."))
}

/// Windows shows its print dialog, then MuPDF draws each page as a picture at
/// the printer's resolution, so text stays sharp.
#[cfg(windows)]
mod windows {
    use std::ops::{Range, RangeInclusive};
    use windows_sys::Win32::Foundation::{GlobalFree, HWND};
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteDC, GetDeviceCaps, HDC, HORZRES, LOGPIXELSX,
        LOGPIXELSY, SRCCOPY, StretchDIBits, VERTRES,
    };
    use windows_sys::Win32::Storage::Xps::{AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};
    use windows_sys::Win32::UI::Controls::Dialogs::{
        CommDlgExtendedError, PD_NOSELECTION, PD_PAGENUMS, PD_RETURNDC, PD_USEDEVMODECOPIESANDCOLLATE, PRINTDLGW,
        PrintDlgW,
    };

    /// The printer and pages the user picked.
    pub struct Job {
        /// The printer's device context, as a number so the job can move
        /// between threads.
        dc: usize,
        pages: RangeInclusive<usize>,
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe { DeleteDC(self.dc as HDC) };
        }
    }

    /// The window the print dialog belongs to.
    pub fn owner(window: &dyn iced::window::Window) -> Option<usize> {
        use raw_window_handle::RawWindowHandle;
        match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as usize),
            _ => None,
        }
    }

    /// Shows the print dialog. It runs on its own thread, like rfd's file
    /// dialogs, so the app keeps drawing behind it. `None` means cancelled.
    pub async fn ask(owner: Option<usize>, pages: usize) -> Result<Option<Job>, String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || reply.send(dialog(owner, pages)));
        rx.await.map_err(|_| "The print dialog closed unexpectedly.".to_string())?
    }

    fn dialog(owner: Option<usize>, pages: usize) -> Result<Option<Job>, String> {
        let last = pages.min(u16::MAX.into()) as u16;
        let mut setup = PRINTDLGW {
            lStructSize: size_of::<PRINTDLGW>() as u32,
            hwndOwner: owner.unwrap_or(0) as HWND,
            Flags: PD_RETURNDC | PD_USEDEVMODECOPIESANDCOLLATE | PD_NOSELECTION,
            nFromPage: 1,
            nToPage: last,
            nMinPage: 1,
            nMaxPage: last,
            nCopies: 1,
            ..PRINTDLGW::default()
        };
        let picked = unsafe { PrintDlgW(&mut setup) } != 0;
        unsafe {
            GlobalFree(setup.hDevMode);
            GlobalFree(setup.hDevNames);
        }
        if !picked {
            return match unsafe { CommDlgExtendedError() } {
                0 => Ok(None),
                _ => Err("Couldn't open the print dialog.".into()),
            };
        }
        if setup.hDC.is_null() {
            return Err("Couldn't connect to the printer.".into());
        }
        let pages = if setup.Flags & PD_PAGENUMS != 0 {
            usize::from(setup.nFromPage) - 1..=usize::from(setup.nToPage) - 1
        } else {
            0..=pages - 1
        };
        Ok(Some(Job { dc: setup.hDC as usize, pages }))
    }

    /// Rows of printer pixels drawn at a time, to keep memory down at
    /// printer resolutions (a whole letter page at 600 DPI is 134 MB).
    const BAND: i32 = 512;

    /// Prints the job's pages. `size` gives a page's size in points; `band`
    /// draws some rows of a page at a size in printer pixels, turned a
    /// quarter clockwise if asked, as top-down BGRA pixels.
    pub fn print(
        job: &Job,
        title: &str,
        size: impl Fn(usize) -> Option<(f32, f32)>,
        mut band: impl FnMut(usize, (i32, i32), bool, Range<i32>) -> Option<Vec<u8>>,
    ) -> Result<(), String> {
        let dc = job.dc as HDC;
        let title: Vec<u16> = title.encode_utf16().chain([0]).collect();
        let info = DOCINFOW { cbSize: size_of::<DOCINFOW>() as i32, lpszDocName: title.as_ptr(), ..DOCINFOW::default() };
        if unsafe { StartDocW(dc, &info) } <= 0 {
            return Err("The printer didn't accept the document.".into());
        }
        let printed = job.pages.clone().try_for_each(|index| print_page(dc, index, &size, &mut band));
        match printed {
            Ok(()) if unsafe { EndDoc(dc) } > 0 => Ok(()),
            Ok(()) => Err("The printer didn't finish the document.".into()),
            Err(error) => {
                unsafe { AbortDoc(dc) };
                Err(error)
            }
        }
    }

    /// Prints one page centered on the paper, turned to match the paper's
    /// orientation and shrunk if it doesn't fit, like macOS does.
    fn print_page(
        dc: HDC,
        index: usize,
        size: impl Fn(usize) -> Option<(f32, f32)>,
        mut band: impl FnMut(usize, (i32, i32), bool, Range<i32>) -> Option<Vec<u8>>,
    ) -> Result<(), String> {
        let unreadable = || format!("Couldn't read page {} for printing.", index + 1);
        let (mut width, mut height) = size(index).ok_or_else(unreadable)?;
        // The printable area, in printer pixels.
        let caps = |index| unsafe { GetDeviceCaps(dc, index as i32) };
        let (paper_width, paper_height) = (caps(HORZRES), caps(VERTRES));
        let turn = (width > height) != (paper_width > paper_height);
        if turn {
            (width, height) = (height, width);
        }
        let (width, height) = (width / 72.0 * caps(LOGPIXELSX) as f32, height / 72.0 * caps(LOGPIXELSY) as f32);
        let scale = (paper_width as f32 / width).min(paper_height as f32 / height).min(1.0);
        let (width, height) = (((width * scale) as i32).max(1), ((height * scale) as i32).max(1));
        let (x, y) = ((paper_width - width) / 2, (paper_height - height) / 2);

        if unsafe { StartPage(dc) } <= 0 {
            return Err("The printer stopped responding.".into());
        }
        let mut rows = 0..BAND.min(height);
        while !rows.is_empty() {
            let pixels = band(index, (width, height), turn, rows.clone()).ok_or_else(unreadable)?;
            let count = rows.len() as i32;
            let header = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    // Negative: rows go top to bottom.
                    biHeight: -count,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..BITMAPINFOHEADER::default()
                },
                ..BITMAPINFO::default()
            };
            let (top, bits) = (y + rows.start, pixels.as_ptr().cast());
            let drawn = unsafe { StretchDIBits(dc, x, top, width, count, 0, 0, width, count, bits, &header, DIB_RGB_COLORS, SRCCOPY) };
            if drawn <= 0 {
                unsafe { EndPage(dc) };
                return Err("The printer couldn't take the page.".into());
            }
            rows = rows.end..(rows.end + BAND).min(height);
        }
        if unsafe { EndPage(dc) } > 0 { Ok(()) } else { Err("The printer stopped responding.".into()) }
    }
}
