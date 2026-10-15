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

/// A clickable area on a page.
#[derive(Debug, Clone)]
pub struct Link {
    /// Left, top, right, bottom as fractions of the page, (0, 0) at top left.
    pub area: [f32; 4],
    pub target: Target,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// A page, and how far down it to show, from 0 to 1, if the link says.
    Page(usize, Option<f32>),
    /// A web or email address.
    Web(String),
}

/// A rendered page and its links.
pub type Rendered = (image::Handle, Vec<Link>);

/// Why a file didn't open.
#[derive(Debug, Clone)]
pub enum OpenError {
    /// The file is locked and needs a password, or the one given was wrong.
    Password,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct OutlineEntry {
    pub title: String,
    pub page: Option<usize>,
    pub depth: usize,
}

enum Request {
    Open(PathBuf, Option<String>, oneshot::Sender<Result<DocInfo, OpenError>>),
    Render(usize, u32, oneshot::Sender<Option<Rendered>>),
    #[cfg(not(windows))]
    Export(Vec<(usize, Stroke)>, oneshot::Sender<Result<Vec<u8>, String>>),
    #[cfg(windows)]
    Print(Vec<(usize, Stroke)>, crate::print::Job, oneshot::Sender<Result<(), String>>),
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

    pub async fn open(self, path: PathBuf, password: Option<String>) -> Result<DocInfo, OpenError> {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Request::Open(path, password, reply));
        rx.await.unwrap_or_else(|_| Err(OpenError::Failed("PDF engine stopped".into())))
    }

    pub async fn render(self, page: usize, width: u32) -> Option<Rendered> {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Request::Render(page, width, reply));
        rx.await.ok().flatten()
    }

    /// The open document's bytes with the drawings burned in.
    #[cfg(not(windows))]
    pub async fn export(self, strokes: Vec<(usize, Stroke)>) -> Result<Vec<u8>, String> {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Request::Export(strokes, reply));
        rx.await.unwrap_or_else(|_| Err("PDF engine stopped".into()))
    }

    /// Prints the open document with the drawings burned in.
    #[cfg(windows)]
    pub async fn print(self, strokes: Vec<(usize, Stroke)>, job: crate::print::Job) -> Result<(), String> {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Request::Print(strokes, job, reply));
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
    // The document on screen, with the password that unlocked it.
    let mut open: Option<(PathBuf, Option<String>, PdfDocument)> = None;

    while let Some(request) = rx.blocking_recv() {
        match request {
            Request::Open(path, password, reply) => {
                // The current document stays open until the new one has
                // opened, so a failed open (or a password prompt) leaves it
                // on screen and working.
                let result = pdfium
                    .load_pdf_from_file(&path, password.as_deref())
                    .map_err(|e| match e {
                        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
                            OpenError::Password
                        }
                        _ => OpenError::Failed(format!("Couldn't open this file ({e}).")),
                    })
                    .map(|doc| {
                        let info = describe(&doc);
                        open = Some((path, password, doc));
                        info
                    });
                let _ = reply.send(result);
            }
            Request::Render(index, width, reply) => {
                let still_wanted = wanted.lock().unwrap().get(&index) == Some(&width);
                let image = open
                    .as_ref()
                    .filter(|_| still_wanted)
                    .and_then(|(_, _, doc)| render(doc, index, width));
                let _ = reply.send(image);
            }
            #[cfg(not(windows))]
            Request::Export(strokes, reply) => {
                let result = match &open {
                    Some((path, password, _)) => export(&pdfium, path, password.as_deref(), &strokes),
                    None => Err("No document is open.".into()),
                };
                let _ = reply.send(result);
            }
            #[cfg(windows)]
            Request::Print(strokes, job, reply) => {
                let result = match &open {
                    Some((path, password, _)) => export(&pdfium, path, password.as_deref(), &strokes).and_then(|pdf| {
                        let title = path.file_name().unwrap_or_default().to_string_lossy();
                        crate::print::print(&job, &pdf, &title)
                    }),
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

fn render(doc: &PdfDocument, index: usize, width: u32) -> Option<Rendered> {
    let page = doc.pages().get(index as PdfPageIndex).ok()?;
    let config = PdfRenderConfig::new()
        .set_target_width(width as Pixels)
        .render_form_data(true);
    let bitmap = page.render_with_config(&config).ok()?;
    let image = image::Handle::from_rgba(bitmap.width() as u32, bitmap.height() as u32, bitmap.as_rgba_bytes());
    Some((image, links(doc, &page)))
}

/// Size of the virtual bitmap used to turn PDF points into fractions of the
/// page and back, so pages with their own rotation or odd origins line up.
const VIRTUAL_WIDTH: f32 = 10_000.0;

/// Where a point of `page` lands, as fractions of the page from the top left.
fn fraction(page: &PdfPage, x: PdfPoints, y: PdfPoints) -> Option<(f32, f32)> {
    let config = PdfRenderConfig::new().set_target_width(VIRTUAL_WIDTH as Pixels);
    let height = VIRTUAL_WIDTH * page.height().value / page.width().value;
    let (px, py) = page.points_to_pixels(x, y, &config).ok()?;
    Some((px as f32 / VIRTUAL_WIDTH, py as f32 / height))
}

fn links(doc: &PdfDocument, page: &PdfPage) -> Vec<Link> {
    page.links()
        .iter()
        .filter_map(|link| {
            let rect = link.rect().ok()?;
            let (left, top) = fraction(page, rect.left(), rect.top())?;
            let (right, bottom) = fraction(page, rect.right(), rect.bottom())?;
            let target = match link.destination() {
                Some(destination) => target(doc, &destination),
                None => {
                    let action = link.action()?;
                    match action.as_uri_action() {
                        Some(uri) => uri.uri().ok().map(Target::Web),
                        None => target(doc, &action.as_local_destination_action()?.destination().ok()?),
                    }
                }
            }?;
            // A page with its own rotation can swap the corners.
            Some(Link { area: [left.min(right), top.min(bottom), left.max(right), top.max(bottom)], target })
        })
        .collect()
}

fn target(doc: &PdfDocument, destination: &PdfDestination) -> Option<Target> {
    let index = destination.page_index().ok()? as usize;
    let top = match destination.view_settings().ok()? {
        PdfDestinationViewSettings::SpecificCoordinatesAndZoom(_, Some(top), _)
        | PdfDestinationViewSettings::FitPageHorizontallyToWindow(Some(top)) => {
            let page = doc.pages().get(index as PdfPageIndex).ok()?;
            fraction(&page, PdfPoints::new(0.0), top).map(|(_, y)| y.clamp(0.0, 1.0))
        }
        _ => None,
    };
    Some(Target::Page(index, top))
}

/// The file as it should go to the printer: drawings burned in, and never
/// locked, so the system's printing doesn't need the password.
fn export(
    pdfium: &Pdfium,
    path: &Path,
    password: Option<&str>,
    strokes: &[(usize, Stroke)],
) -> Result<Vec<u8>, String> {
    if strokes.is_empty() && password.is_none() {
        return std::fs::read(path).map_err(|e| e.to_string());
    }

    let source = pdfium
        .load_pdf_from_file(path, password)
        .map_err(|e| e.to_string())?;
    // PDFium saves a locked file still locked, so a locked file's pages are
    // copied into a new document instead.
    let mut doc = if password.is_some() {
        let mut copy = pdfium.create_new_pdf().map_err(|e| e.to_string())?;
        copy.pages_mut().append(&source).map_err(|e| e.to_string())?;
        copy
    } else {
        source
    };
    for (index, stroke) in strokes {
        burn_in(&mut doc, *index, stroke).map_err(|e| e.to_string())?;
    }
    doc.save_to_bytes().map_err(|e| e.to_string())
}

fn burn_in(doc: &mut PdfDocument, index: usize, stroke: &Stroke) -> Result<(), PdfiumError> {
    // Strokes are stored relative to the page, so map them through the
    // virtual bitmap to get PDF coordinates.
    let mut page = doc.pages().get(index as PdfPageIndex)?;
    let config = PdfRenderConfig::new().set_target_width(VIRTUAL_WIDTH as Pixels);
    let height = VIRTUAL_WIDTH * page.height().value / page.width().value;
    let points = stroke
        .points
        .iter()
        .map(|(u, v)| page.pixels_to_points((u * VIRTUAL_WIDTH) as Pixels, (v * height) as Pixels, &config))
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
