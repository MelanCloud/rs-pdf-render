//! PDFium lives on its own thread. The UI talks to it through tokio channels.

use crate::ink::Stroke;
use iced::widget::image;
use pdfium_render::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone)]
pub struct DocInfo {
    /// Unrotated page sizes in PDF points.
    pub pages: Vec<(f32, f32)>,
    pub outline: Vec<OutlineEntry>,
}

#[derive(Debug, Clone)]
pub struct OutlineEntry {
    pub title: String,
    pub page: Option<usize>,
    pub depth: usize,
}

enum Request {
    Open(PathBuf, oneshot::Sender<Result<DocInfo, String>>),
    Render(usize, u32, oneshot::Sender<Option<image::Handle>>),
    Export(Vec<(usize, Stroke)>, oneshot::Sender<Result<Vec<u8>, String>>),
}

/// Handle to the PDFium thread. Cheap to clone.
#[derive(Clone)]
pub struct Worker {
    tx: mpsc::UnboundedSender<Request>,
    /// Page index -> bitmap width the UI still wants. Renders for anything else
    /// (scrolled away, zoom changed since) are skipped.
    wanted: Arc<Mutex<HashMap<usize, u32>>>,
}

impl Worker {
    pub fn spawn() -> Result<Self, String> {
        let bindings = bind()?;
        let (tx, rx) = mpsc::unbounded_channel();
        let wanted = Arc::new(Mutex::new(HashMap::new()));
        let shared = wanted.clone();
        std::thread::Builder::new()
            .name("pdfium".into())
            .spawn(move || run(Pdfium::new(bindings), rx, shared))
            .map_err(|e| e.to_string())?;
        Ok(Self { tx, wanted })
    }

    pub fn set_wanted(&self, wanted: HashMap<usize, u32>) {
        *self.wanted.lock().unwrap() = wanted;
    }

    pub async fn open(self, path: PathBuf) -> Result<DocInfo, String> {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Request::Open(path, reply));
        rx.await.unwrap_or_else(|_| Err("PDF engine stopped".into()))
    }

    pub async fn render(self, page: usize, width: u32) -> Option<image::Handle> {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Request::Render(page, width, reply));
        rx.await.ok().flatten()
    }

    /// The open document's bytes with the drawings burned in.
    pub async fn export(self, strokes: Vec<(usize, Stroke)>) -> Result<Vec<u8>, String> {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Request::Export(strokes, reply));
        rx.await.unwrap_or_else(|_| Err("PDF engine stopped".into()))
    }
}

/// Looks for the PDFium library next to the executable, then in the working
/// directory, then wherever the system keeps libraries.
fn bind() -> Result<Box<dyn PdfiumLibraryBindings>, String> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    exe_dir
        .into_iter()
        .chain([PathBuf::from(".")])
        .find_map(|dir| {
            Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&dir)).ok()
        })
        .map_or_else(Pdfium::bind_to_system_library, Ok)
        .map_err(|_| {
            format!(
                "Couldn't find {}. Put it next to the app.",
                Pdfium::pdfium_platform_library_name().to_string_lossy()
            )
        })
}

fn run(
    pdfium: Pdfium,
    mut rx: mpsc::UnboundedReceiver<Request>,
    wanted: Arc<Mutex<HashMap<usize, u32>>>,
) {
    let mut open: Option<(PathBuf, PdfDocument)> = None;

    while let Some(request) = rx.blocking_recv() {
        match request {
            Request::Open(path, reply) => {
                open = None;
                let result = pdfium
                    .load_pdf_from_file(&path, None)
                    .map_err(|e| match e {
                        PdfiumError::PdfiumLibraryInternalError(
                            PdfiumInternalError::PasswordError,
                        ) => "Password-protected PDFs aren't supported yet.".to_string(),
                        _ => format!("Couldn't open this file ({e})."),
                    })
                    .map(|doc| {
                        let info = describe(&doc);
                        open = Some((path, doc));
                        info
                    });
                let _ = reply.send(result);
            }
            Request::Render(index, width, reply) => {
                let still_wanted = wanted.lock().unwrap().get(&index) == Some(&width);
                let image = open
                    .as_ref()
                    .filter(|_| still_wanted)
                    .and_then(|(_, doc)| render(doc, index, width));
                let _ = reply.send(image);
            }
            Request::Export(strokes, reply) => {
                let result = match &open {
                    Some((path, _)) => export(&pdfium, path, &strokes),
                    None => Err("No document is open.".into()),
                };
                let _ = reply.send(result);
            }
        }
    }
}

fn describe(doc: &PdfDocument) -> DocInfo {
    let pages = doc
        .pages()
        .page_sizes()
        .unwrap_or_default()
        .iter()
        .map(|rect| (rect.width().value, rect.height().value))
        .collect();

    let mut outline = Vec::new();
    if let Some(root) = doc.bookmarks().root() {
        collect_outline(root, 0, &mut outline);
    }

    DocInfo { pages, outline }
}

fn collect_outline(first: PdfBookmark, depth: usize, out: &mut Vec<OutlineEntry>) {
    // Broken files can contain cyclic outlines.
    const LIMIT: usize = 10_000;

    let mut next = Some(first);
    while let Some(bookmark) = next {
        if out.len() >= LIMIT || depth > 32 {
            return;
        }
        let page = bookmark
            .destination()
            .and_then(|dest| dest.page_index().ok())
            .or_else(|| {
                let action = bookmark.action()?;
                action.as_local_destination_action()?.destination().ok()?.page_index().ok()
            })
            .map(|index| index as usize);
        out.push(OutlineEntry {
            title: bookmark.title().unwrap_or_default().trim().to_string(),
            page,
            depth,
        });
        if let Some(child) = bookmark.first_child() {
            collect_outline(child, depth + 1, out);
        }
        next = bookmark.next_sibling();
    }
}

fn render(doc: &PdfDocument, index: usize, width: u32) -> Option<image::Handle> {
    let page = doc.pages().get(index as PdfPageIndex).ok()?;
    let config = PdfRenderConfig::new()
        .set_target_width(width as Pixels)
        .render_form_data(true);
    let bitmap = page.render_with_config(&config).ok()?;
    Some(image::Handle::from_rgba(
        bitmap.width() as u32,
        bitmap.height() as u32,
        bitmap.as_rgba_bytes(),
    ))
}

fn export(pdfium: &Pdfium, path: &Path, strokes: &[(usize, Stroke)]) -> Result<Vec<u8>, String> {
    if strokes.is_empty() {
        return std::fs::read(path).map_err(|e| e.to_string());
    }

    let mut doc = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|e| e.to_string())?;
    for (index, stroke) in strokes {
        burn_in(&mut doc, *index, stroke).map_err(|e| e.to_string())?;
    }
    doc.save_to_bytes().map_err(|e| e.to_string())
}

fn burn_in(doc: &mut PdfDocument, index: usize, stroke: &Stroke) -> Result<(), PdfiumError> {
    // Strokes are stored relative to the page, so map them through a large
    // virtual bitmap to get PDF coordinates that respect the page's own
    // rotation and origin.
    const SIZE: f32 = 10_000.0;

    let mut page = doc.pages().get(index as PdfPageIndex)?;
    let config = PdfRenderConfig::new().set_target_width(SIZE as Pixels);
    let height = SIZE * page.height().value / page.width().value;
    let points = stroke
        .points
        .iter()
        .map(|(u, v)| page.pixels_to_points((u * SIZE) as Pixels, (v * height) as Pixels, &config))
        .collect::<Result<Vec<_>, _>>()?;

    let [r, g, b, a] = stroke.color.into_rgba8();
    let color = PdfColor::new(r, g, b, a);
    let (x, y) = points[0];
    let mut path = PdfPagePathObject::new(doc, x, y, Some(color), Some(PdfPoints::new(stroke.width)), None)?;
    for (x, y) in &points[1..] {
        path.line_to(*x, *y)?;
    }
    path.set_line_cap(PdfPageObjectLineCap::Round)?;
    path.set_line_join(PdfPageObjectLineJoin::Round)?;
    page.objects_mut().add_path_object(path)?;
    Ok(())
}
