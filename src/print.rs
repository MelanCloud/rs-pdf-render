//! Hands a PDF to the operating system's printing.

use iced::window::Window;

/// Opens the native print sheet with a preview.
#[cfg(target_os = "macos")]
pub fn print(window: &dyn Window, pdf: Vec<u8>) -> Result<(), String> {
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

/// Sends the file to the default PDF app's print command (Windows) or to
/// the default printer (everything else).
#[cfg(not(target_os = "macos"))]
pub fn print(_window: &dyn Window, pdf: Vec<u8>) -> Result<(), String> {
    use std::process::Command;

    let path = std::env::temp_dir().join("rs-pdf-render-print.pdf");
    std::fs::write(&path, pdf).map_err(|e| e.to_string())?;

    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("powershell");
        let script = format!("Start-Process -Verb Print -FilePath '{}'", path.display());
        command.args(["-NoProfile", "-Command", &script]);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = Command::new("lp");
        command.arg(&path);
        command
    };

    command.spawn().map(drop).map_err(|e| format!("Couldn't start printing ({e})."))
}
