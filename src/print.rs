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

/// Windows shows its print dialog, then PDFium draws each page straight onto
/// the printer, so text stays sharp and big documents stay small.
#[cfg(windows)]
mod windows {
    use std::ffi::{CStr, c_char, c_int, c_void};
    use std::ops::RangeInclusive;
    use windows_sys::Win32::Foundation::{GlobalFree, HMODULE, HWND};
    use windows_sys::Win32::Graphics::Gdi::{DeleteDC, GetDeviceCaps, HDC, HORZRES, LOGPIXELSX, LOGPIXELSY, VERTRES};
    use windows_sys::Win32::Storage::Xps::{AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows_sys::Win32::UI::Controls::Dialogs::{
        CommDlgExtendedError, PD_NOSELECTION, PD_PAGENUMS, PD_RETURNDC, PD_USEDEVMODECOPIESANDCOLLATE, PRINTDLGW,
        PrintDlgW,
    };
    use windows_sys::w;

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

    /// Prints `pdf` to the job's printer. Must run on the PDFium thread.
    pub fn print(job: &Job, pdf: &[u8], title: &str) -> Result<(), String> {
        let pdfium = Pdfium::find().ok_or("Couldn't find PDFium's printing functions.")?;
        let doc = unsafe { (pdfium.load_document)(pdf.as_ptr().cast(), pdf.len(), std::ptr::null()) };
        if doc.is_null() {
            return Err("Couldn't prepare the document for printing.".into());
        }
        let dc = job.dc as HDC;
        let title: Vec<u16> = title.encode_utf16().chain([0]).collect();
        let info = DOCINFOW { cbSize: size_of::<DOCINFOW>() as i32, lpszDocName: title.as_ptr(), ..DOCINFOW::default() };

        let result = if unsafe { StartDocW(dc, &info) } <= 0 {
            Err("The printer didn't accept the document.".to_string())
        } else {
            let printed = job.pages.clone().try_for_each(|index| unsafe { pdfium.print_page(doc, index, dc) });
            match printed {
                Ok(()) if unsafe { EndDoc(dc) } > 0 => Ok(()),
                Ok(()) => Err("The printer didn't finish the document.".into()),
                Err(error) => {
                    unsafe { AbortDoc(dc) };
                    Err(error)
                }
            }
        };
        unsafe { (pdfium.close_document)(doc) };
        result
    }

    type Document = *mut c_void;
    type Page = *mut c_void;

    /// The few PDFium functions printing needs. pdfium-render doesn't offer
    /// drawing onto a printer, so they're looked up in the copy of PDFium it
    /// already loaded.
    struct Pdfium {
        load_document: unsafe extern "system" fn(*const c_void, usize, *const c_char) -> Document,
        close_document: unsafe extern "system" fn(Document),
        load_page: unsafe extern "system" fn(Document, c_int) -> Page,
        close_page: unsafe extern "system" fn(Page),
        /// Page sizes in points, after the page's own rotation.
        page_width: unsafe extern "system" fn(Page) -> f32,
        page_height: unsafe extern "system" fn(Page) -> f32,
        render: unsafe extern "system" fn(HDC, Page, c_int, c_int, c_int, c_int, c_int, c_int),
    }

    impl Pdfium {
        fn find() -> Option<Self> {
            /// Looks up `name`, which has to be a function of type `F`.
            unsafe fn function<F>(library: HMODULE, name: &CStr) -> Option<F> {
                let found = unsafe { GetProcAddress(library, name.as_ptr().cast()) }?;
                Some(unsafe { std::mem::transmute_copy(&found) })
            }

            let library = unsafe { GetModuleHandleW(w!("pdfium.dll")) };
            if library.is_null() {
                return None;
            }
            // SAFETY: the types above match PDFium's headers.
            unsafe {
                Some(Self {
                    load_document: function(library, c"FPDF_LoadMemDocument64")?,
                    close_document: function(library, c"FPDF_CloseDocument")?,
                    load_page: function(library, c"FPDF_LoadPage")?,
                    close_page: function(library, c"FPDF_ClosePage")?,
                    page_width: function(library, c"FPDF_GetPageWidthF")?,
                    page_height: function(library, c"FPDF_GetPageHeightF")?,
                    render: function(library, c"FPDF_RenderPage")?,
                })
            }
        }

        /// Prints one page centered on the paper, turned to match the paper's
        /// orientation and shrunk if it doesn't fit, like macOS does.
        unsafe fn print_page(&self, doc: Document, index: usize, dc: HDC) -> Result<(), String> {
            const FPDF_ANNOT: c_int = 0x01;
            const FPDF_PRINTING: c_int = 0x800;

            let page = unsafe { (self.load_page)(doc, index as c_int) };
            if page.is_null() {
                return Err(format!("Couldn't read page {} for printing.", index + 1));
            }
            // The printable area, in printer pixels.
            let caps = |index| unsafe { GetDeviceCaps(dc, index as i32) };
            let (paper_width, paper_height) = (caps(HORZRES), caps(VERTRES));
            let (mut width, mut height) = unsafe { ((self.page_width)(page), (self.page_height)(page)) };
            let turn = (width > height) != (paper_width > paper_height);
            if turn {
                (width, height) = (height, width);
            }
            let (width, height) = (width / 72.0 * caps(LOGPIXELSX) as f32, height / 72.0 * caps(LOGPIXELSY) as f32);
            let scale = (paper_width as f32 / width).min(paper_height as f32 / height).min(1.0);
            let (width, height) = ((width * scale) as c_int, (height * scale) as c_int);

            let started = unsafe { StartPage(dc) } > 0;
            if started {
                let (x, y) = ((paper_width - width) / 2, (paper_height - height) / 2);
                unsafe { (self.render)(dc, page, x, y, width, height, c_int::from(turn), FPDF_ANNOT | FPDF_PRINTING) };
            }
            let ended = started && unsafe { EndPage(dc) } > 0;
            unsafe { (self.close_page)(page) };
            if ended { Ok(()) } else { Err("The printer stopped responding.".into()) }
        }
    }
}
