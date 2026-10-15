//! MuPDF lives on its own thread. The UI talks to it through tokio channels.

use crate::ink::Stroke;
use iced::widget::image;
use mupdf::link::LinkDestination;
use mupdf::text_page::TextPageFlags;
use mupdf::{ColorParams, Colorspace, DestinationKind, Device, Document, IRect, Matrix, Page, Pixmap, Rect, StrokeState};
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

/// How a page bitmap is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
}

/// A part of a page rendered at full sharpness, for zooms where the whole
/// page would be too big a bitmap. In pixels of the full-size page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detail {
    /// The full page's width at this zoom, which the rest is measured in.
    pub page_width: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Detail {
    /// Whether this already shows all of `other`.
    pub fn covers(&self, other: &Detail) -> bool {
        self.page_width == other.page_width
            && self.x <= other.x
            && self.y <= other.y
            && self.x + self.width >= other.x + other.width
            && self.y + self.height >= other.y + other.height
    }
}

/// A rendered page and its links.
pub type Rendered = (image::Handle, Vec<Link>);

/// A page's text, for selecting and copying: its characters in reading order
/// and its lines. Boxes are left, top, right, bottom as fractions of the page.
#[derive(Debug, Clone, Default)]
pub struct PageText {
    pub chars: Vec<char>,
    pub boxes: Vec<[f32; 4]>,
    /// Where each line's characters start and end in `chars`, and its box.
    pub lines: Vec<(usize, usize, [f32; 4])>,
}

/// One match of a search: its page and boxes (a match can go on over a line
/// break).
#[derive(Debug, Clone)]
pub struct Hit {
    pub page: usize,
    pub areas: Vec<[f32; 4]>,
}

impl PageText {
    /// Where a point falls between the characters, like a text cursor: on the
    /// line under it (or else the nearest line), before the first character
    /// whose middle is right of it. Past the last character is `chars.len()`
    /// for the last line, or the next line's start.
    pub fn caret(&self, (x, y): (f32, f32)) -> usize {
        let away = |[left, top, right, bottom]: [f32; 4]| {
            ((top - y).max(y - bottom).max(0.0), (left - x).max(x - right).max(0.0))
        };
        let nearest = self.lines.iter().min_by(|a, b| {
            let ((ay, ax), (by, bx)) = (away(a.2), away(b.2));
            ay.total_cmp(&by).then(ax.total_cmp(&bx))
        });
        let Some(&(start, end, _)) = nearest else { return 0 };
        (start..end)
            .find(|&i| {
                let [left, _, right, _] = self.boxes[i];
                (left + right) / 2.0 > x
            })
            .unwrap_or(end)
    }

    /// The word around a caret, for a double click.
    pub fn word(&self, caret: usize) -> (usize, usize) {
        let (line_start, line_end) = self.line(caret);
        let in_word = |i: usize| self.chars[i].is_alphanumeric();
        let mut start = caret.min(line_end.saturating_sub(1)).max(line_start);
        if start >= line_end || !in_word(start) {
            return (start, (start + 1).min(line_end));
        }
        let mut end = start;
        while start > line_start && in_word(start - 1) {
            start -= 1;
        }
        while end < line_end && in_word(end) {
            end += 1;
        }
        (start, end)
    }

    /// The line a caret is on, for a triple click.
    pub fn line(&self, caret: usize) -> (usize, usize) {
        self.lines
            .iter()
            .find(|(start, end, _)| (*start..*end).contains(&caret))
            .or(self.lines.last())
            .map_or((0, 0), |&(start, end, _)| (start, end))
    }

    /// Boxes covering the characters from `from` to `to`, one per line.
    pub fn areas(&self, from: usize, to: usize) -> Vec<[f32; 4]> {
        self.lines
            .iter()
            .filter_map(|&(start, end, _)| {
                let (a, b) = (start.max(from), end.min(to));
                (a < b).then(|| union(&self.boxes[a..b]))
            })
            .collect()
    }

    /// The characters from `from` to `to`, with a line break between lines.
    fn text(&self, from: usize, to: usize) -> String {
        let mut text = String::new();
        for (index, &(start, end, _)) in self.lines.iter().enumerate() {
            let (a, b) = (start.max(from), end.min(to));
            if a < b {
                if index > 0 && a == start && !text.is_empty() {
                    text.push('\n');
                }
                text.extend(&self.chars[a..b]);
            }
        }
        text
    }
}

/// The box around some boxes.
fn union(boxes: &[[f32; 4]]) -> [f32; 4] {
    boxes.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |[l, t, r, b], [bl, bt, br, bb]| {
        [l.min(*bl), t.min(*bt), r.max(*br), b.max(*bb)]
    })
}

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
    /// How far down the page to show, from 0 to 1, for scanned headings.
    pub top: Option<f32>,
    pub depth: usize,
    /// Found by scanning the text rather than read from the file's outline.
    pub scanned: bool,
}

/// A numbered heading found in a page's text, like "2.6.4 Zero–One Matrices".
#[derive(Debug, Clone)]
pub struct Heading {
    pub number: Vec<u16>,
    pub title: String,
    pub page: usize,
    /// How far down the page the heading starts, from 0 to 1.
    pub top: f32,
}

enum Request {
    Open(PathBuf, Option<String>, oneshot::Sender<Result<DocInfo, OpenError>>),
    Render(usize, Bitmap, oneshot::Sender<Option<Rendered>>),
    RenderDetail(usize, Detail, oneshot::Sender<Option<image::Handle>>),
    #[cfg(not(windows))]
    Export(Vec<(usize, Stroke)>, oneshot::Sender<Result<Vec<u8>, String>>),
    Scan(std::ops::Range<usize>, oneshot::Sender<Vec<Heading>>),
    Text(usize, oneshot::Sender<Option<PageText>>),
    Find(String, std::ops::Range<usize>, oneshot::Sender<Vec<Hit>>),
    Copy((usize, usize), (usize, usize), oneshot::Sender<String>),
    #[cfg(windows)]
    Print(Vec<(usize, Stroke)>, crate::print::Job, oneshot::Sender<Result<(), String>>),
}

/// Handle to the MuPDF thread. Cheap to clone.
#[derive(Clone)]
pub struct Worker {
    tx: mpsc::UnboundedSender<Request>,
    /// Page index -> bitmap the UI still wants. Renders for anything else
    /// (scrolled away, zoom changed since) are skipped.
    wanted: Wanted<Bitmap>,
    /// The same for sharp details.
    wanted_details: Wanted<Detail>,
}

/// Page index -> what the UI still wants rendered for it, shared with the
/// MuPDF thread.
type Wanted<T> = Arc<Mutex<HashMap<usize, T>>>;

impl Worker {
    pub fn spawn() -> Result<Self, String> {
        let (tx, rx) = mpsc::unbounded_channel();
        let wanted = Arc::new(Mutex::new(HashMap::new()));
        let wanted_details = Arc::new(Mutex::new(HashMap::new()));
        let shared = (wanted.clone(), wanted_details.clone());
        std::thread::Builder::new()
            .name("mupdf".into())
            .spawn(move || run(rx, shared))
            .map_err(|e| e.to_string())?;
        Ok(Self { tx, wanted, wanted_details })
    }

    pub fn set_wanted(&self, wanted: HashMap<usize, Bitmap>, details: HashMap<usize, Detail>) {
        *self.wanted.lock().unwrap() = wanted;
        *self.wanted_details.lock().unwrap() = details;
    }

    pub async fn open(self, path: PathBuf, password: Option<String>) -> Result<DocInfo, OpenError> {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Request::Open(path, password, reply));
        rx.await.unwrap_or_else(|_| Err(OpenError::Failed("PDF engine stopped".into())))
    }

    pub async fn render_detail(self, page: usize, detail: Detail) -> Option<image::Handle> {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Request::RenderDetail(page, detail, reply));
        rx.await.ok().flatten()
    }

    pub async fn render(self, page: usize, bitmap: Bitmap) -> Option<Rendered> {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Request::Render(page, bitmap, reply));
        rx.await.ok().flatten()
    }

    /// A page's text, for selecting, if the page is still near the view.
    pub async fn text(self, page: usize) -> Option<PageText> {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Request::Text(page, reply));
        rx.await.ok().flatten()
    }

    /// Where `needle` is on some pages, ignoring case.
    pub async fn find(self, needle: String, pages: std::ops::Range<usize>) -> Vec<Hit> {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Request::Find(needle, pages, reply));
        rx.await.unwrap_or_default()
    }

    /// The text between two places, each a page and a caret on it.
    pub async fn copy(self, from: (usize, usize), to: (usize, usize)) -> String {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Request::Copy(from, to, reply));
        rx.await.unwrap_or_default()
    }

    /// Numbered headings on some pages of the open document.
    pub async fn scan(self, pages: std::ops::Range<usize>) -> Vec<Heading> {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Request::Scan(pages, reply));
        rx.await.unwrap_or_default()
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

fn run(mut rx: mpsc::UnboundedReceiver<Request>, (wanted, wanted_details): (Wanted<Bitmap>, Wanted<Detail>)) {
    // The document on screen, with the password that unlocked it.
    let mut open: Option<(PathBuf, Option<String>, Document)> = None;
    // Scans read their own copy of the document, dropped when the scan ends,
    // so what they parse doesn't stay in memory with the open document.
    let mut scanning: Option<Document> = None;

    while let Some(request) = rx.blocking_recv() {
        match request {
            Request::Open(path, password, reply) => {
                // The current document stays open until the new one has
                // opened, so a failed open (or a password prompt) leaves it
                // on screen and working.
                let result = load(&path, password.as_deref()).map(|doc| {
                    let info = describe(&doc);
                    scanning = None;
                    open = Some((path, password, doc));
                    info
                });
                let _ = reply.send(result);
            }
            Request::Render(index, bitmap, reply) => {
                let still_wanted = wanted.lock().unwrap().get(&index) == Some(&bitmap);
                let image = open
                    .as_ref()
                    .filter(|_| still_wanted)
                    .and_then(|(_, _, doc)| render(doc, index, bitmap));
                let _ = reply.send(image);
            }
            Request::RenderDetail(index, detail, reply) => {
                let still_wanted = wanted_details.lock().unwrap().get(&index) == Some(&detail);
                let image = open
                    .as_ref()
                    .filter(|_| still_wanted)
                    .and_then(|(_, _, doc)| render_detail(doc, index, detail));
                let _ = reply.send(image);
            }
            Request::Text(index, reply) => {
                let still_wanted = wanted.lock().unwrap().contains_key(&index);
                let text = open
                    .as_ref()
                    .filter(|_| still_wanted)
                    .and_then(|(_, _, doc)| page_text(&doc.load_page(index as i32).ok()?));
                let _ = reply.send(text);
            }
            Request::Find(needle, pages, reply) => {
                let hits = match &open {
                    Some((_, _, doc)) => pages.filter_map(|index| find(doc, index, &needle)).flatten().collect(),
                    None => Vec::new(),
                };
                let _ = reply.send(hits);
            }
            Request::Copy(from, to, reply) => {
                let text = match &open {
                    Some((_, _, doc)) => copy(doc, from, to),
                    None => String::new(),
                };
                let _ = reply.send(text);
            }
            Request::Scan(pages, reply) => {
                if pages.start == 0 {
                    scanning = open.as_ref().and_then(|(path, password, _)| load(path, password.as_deref()).ok());
                }
                let Some(doc) = &scanning else {
                    let _ = reply.send(Vec::new());
                    continue;
                };
                let last = pages.end >= doc.page_count().unwrap_or(0).max(0) as usize;
                let found = pages
                    .filter_map(|index| Some(headings(index, &doc.load_page(index as i32).ok()?)))
                    .flatten()
                    .collect();
                if last {
                    scanning = None;
                }
                let _ = reply.send(found);
            }
            #[cfg(not(windows))]
            Request::Export(strokes, reply) => {
                let result = match &open {
                    Some((path, password, doc)) => export(path, password.is_some(), doc, &strokes),
                    None => Err("No document is open.".into()),
                };
                let _ = reply.send(result);
            }
            #[cfg(windows)]
            Request::Print(strokes, job, reply) => {
                let result = match &open {
                    Some((path, _, doc)) => {
                        let title = path.file_name().unwrap_or_default().to_string_lossy();
                        print(doc, &strokes, &job, &title)
                    }
                    None => Err("No document is open.".into()),
                };
                let _ = reply.send(result);
            }
        }
    }
}

/// Opens a file, unlocking it with `password` if it needs one. Files with
/// only a permissions password open without one, as in pdf.js.
fn load(path: &Path, password: Option<&str>) -> Result<Document, OpenError> {
    // MuPDF takes paths as text on Windows.
    #[cfg(windows)]
    let path = path.to_str().ok_or_else(|| OpenError::Failed("Couldn't read this file's name.".into()))?;
    let mut doc = Document::open(path).map_err(|e| OpenError::Failed(format!("Couldn't open this file ({e}).")))?;
    if doc.needs_password().unwrap_or(false) {
        let unlocked = password.is_some_and(|password| doc.authenticate(password).unwrap_or(false));
        if !unlocked {
            return Err(OpenError::Password);
        }
    }
    Ok(doc)
}

fn describe(doc: &Document) -> DocInfo {
    let count = doc.page_count().unwrap_or(0).max(0);
    let pages = (0..count)
        .map(|index| {
            let bounds = doc.load_page(index).and_then(|page| page.bounds());
            bounds.map_or((612.0, 792.0), |b| (b.x1 - b.x0, b.y1 - b.y0))
        })
        .collect();

    let mut outline = Vec::new();
    collect_outline(&doc.outlines().unwrap_or_default(), 0, &mut outline);

    DocInfo { pages, outline }
}

fn collect_outline(items: &[mupdf::Outline], depth: usize, out: &mut Vec<OutlineEntry>) {
    // Broken files can contain huge or deep outlines.
    const LIMIT: usize = 10_000;

    for item in items {
        if out.len() >= LIMIT || depth > 32 {
            return;
        }
        out.push(OutlineEntry {
            title: item.title.trim().to_string(),
            page: item.dest.map(|dest| dest.loc.page_number as usize),
            top: None,
            depth,
            scanned: false,
        });
        collect_outline(&item.down, depth + 1, out);
    }
}

/// A page's text in reading order, with boxes as fractions of the page.
fn page_text(page: &Page) -> Option<PageText> {
    let bounds = page.bounds().ok()?;
    let text = page.to_text_page(TextPageFlags::empty()).ok()?;
    let fraction = |rect: Rect| fractions(bounds, rect);
    let mut out = PageText::default();
    for block in text.blocks() {
        for line in block.lines() {
            let start = out.chars.len();
            for ch in line.chars() {
                let Some(c) = ch.char() else { continue };
                out.chars.push(c);
                out.boxes.push(fraction(quad_bounds(ch.quad())));
            }
            if out.chars.len() > start {
                let area = union(&out.boxes[start..]);
                out.lines.push((start, out.chars.len(), area));
            }
        }
    }
    Some(out)
}

/// The matches of `needle` on a page.
fn find(doc: &Document, index: usize, needle: &str) -> Option<Vec<Hit>> {
    let page = doc.load_page(index as i32).ok()?;
    let bounds = page.bounds().ok()?;
    let text = page.to_text_page(TextPageFlags::empty()).ok()?;
    let mut hits: Vec<Hit> = Vec::new();
    text.search_cb(needle, &mut hits, |hits, quads| {
        let areas = quads.iter().map(|quad| fractions(bounds, quad_bounds(quad.clone()))).collect();
        hits.push(Hit { page: index, areas });
        mupdf::text_page::SearchHitResponse::ContinueSearch
    })
    .ok()?;
    Some(hits)
}

/// The text from one place to another, each a page and a caret on it, with a
/// line break between lines and between pages.
fn copy(doc: &Document, (from_page, from): (usize, usize), (to_page, to): (usize, usize)) -> String {
    let mut text = String::new();
    for index in from_page..=to_page {
        let Some(page) = doc.load_page(index as i32).ok().and_then(|page| page_text(&page)) else { continue };
        let start = if index == from_page { from } else { 0 };
        let end = if index == to_page { to } else { page.chars.len() };
        let part = page.text(start, end.min(page.chars.len()));
        if !part.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&part);
        }
    }
    text
}

/// The box around a quad.
fn quad_bounds(quad: mupdf::Quad) -> Rect {
    let xs = [quad.ul.x, quad.ur.x, quad.ll.x, quad.lr.x];
    let ys = [quad.ul.y, quad.ur.y, quad.ll.y, quad.lr.y];
    let (min, max) = (|v: [f32; 4]| v.into_iter().fold(f32::MAX, f32::min), |v: [f32; 4]| v.into_iter().fold(f32::MIN, f32::max));
    Rect::new(min(xs), min(ys), max(xs), max(ys))
}

/// A rectangle on a page as fractions of it: left, top, right, bottom.
fn fractions(bounds: Rect, rect: Rect) -> [f32; 4] {
    let (width, height) = (bounds.x1 - bounds.x0, bounds.y1 - bounds.y0);
    [
        (rect.x0 - bounds.x0) / width,
        (rect.y0 - bounds.y0) / height,
        (rect.x1 - bounds.x0) / width,
        (rect.y1 - bounds.y0) / height,
    ]
}

/// A character of a line, for finding headings.
struct Char {
    c: char,
    bold: bool,
    size: f32,
    /// Its top edge, in page units.
    top: f32,
}

/// A piece of a line of text: its baseline, left and right edges, and
/// characters.
type Piece = (f32, f32, f32, Vec<Char>);

/// Finds numbered headings in a page's text: lines that start with a number
/// of two or more parts and a title, set bolder or larger than the page's body
/// text (which leaves out running headers and plain mentions).
fn headings(index: usize, page: &Page) -> Vec<Heading> {
    let (Ok(bounds), Ok(text)) = (page.bounds(), page.to_text_page(TextPageFlags::empty())) else {
        return Vec::new();
    };
    let mut pieces: Vec<Piece> = Vec::new();
    let mut sizes = Vec::new();
    for block in text.blocks() {
        for line in block.lines() {
            let mut chars = Vec::new();
            let mut baseline = None;
            for ch in line.chars() {
                let Some(c) = ch.char() else { continue };
                let bold = ch.font().is_some_and(|font| {
                    let name = font.name().to_lowercase();
                    font.is_bold() || ["bold", "black", "heavy", "semibold", "demi"].iter().any(|weight| name.contains(weight))
                });
                if !c.is_whitespace() {
                    sizes.push(ch.size());
                }
                baseline.get_or_insert(ch.origin().y);
                chars.push(Char { c, bold, size: ch.size(), top: ch.quad().ul.y });
            }
            if let Some(baseline) = baseline {
                let edges = line.bounds();
                pieces.push((baseline, edges.x0, edges.x1, chars));
            }
        }
    }
    if sizes.is_empty() {
        return Vec::new();
    }
    sizes.sort_by(f32::total_cmp);
    let body = sizes[sizes.len() / 2];

    // MuPDF splits a line where there's a wide gap ("1.1.1   Introduction"),
    // so pieces on the same baseline are joined again, unless they're so far
    // apart that they're in different columns.
    pieces.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut rows: Vec<Vec<Piece>> = Vec::new();
    for piece in pieces {
        match rows.last_mut() {
            Some(row) if (piece.0 - row[0].0).abs() < body * 0.3 => row.push(piece),
            _ => rows.push(vec![piece]),
        }
    }
    let mut lines: Vec<Vec<Char>> = Vec::new();
    for mut row in rows {
        row.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut right: Option<f32> = None;
        for (_, left, end, chars) in row {
            match lines.last_mut() {
                Some(line) if right.is_some_and(|right| left - right < body * 4.0) => {
                    let last = &line[line.len() - 1];
                    let space = Char { c: ' ', bold: last.bold, size: last.size, top: last.top };
                    line.push(space);
                    line.extend(chars);
                }
                _ => lines.push(chars),
            }
            right = Some(end);
        }
    }

    lines
        .iter()
        .filter_map(|chars| {
            let text: String = chars.iter().map(|ch| ch.c).collect();
            let (number, title) = numbered(&text)?;
            let first = chars.get(title)?;
            let size = chars.iter().filter(|ch| !ch.c.is_whitespace()).map(|ch| ch.size).fold(0.0, f32::max);
            if !first.bold && size < body * 1.1 {
                return None;
            }
            // A little room above, so the heading isn't flush with the edge.
            let top = ((first.top - bounds.y0) / (bounds.y1 - bounds.y0) - 0.015).clamp(0.0, 1.0);
            let title = text.split_whitespace().collect::<Vec<_>>().join(" ");
            Some(Heading { number, title, page: index, top })
        })
        .collect()
}

/// Splits "2.6.4 Zero–One Matrices" into [2, 6, 4] and where the title starts
/// (in characters). Needs two or more parts of up to three digits, then a
/// capitalized title of a sensible length that isn't a contents line (dot
/// leaders or a page number at the end).
fn numbered(line: &str) -> Option<(Vec<u16>, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let start = chars.iter().take_while(|c| c.is_whitespace()).count();
    let end = start + chars[start..].iter().take_while(|c| c.is_ascii_digit() || **c == '.').count();
    let number: String = chars[start..end].iter().collect();
    let parts = number
        .trim_end_matches('.')
        .split('.')
        .map(|part| if (1..=3).contains(&part.len()) { part.parse().ok() } else { None })
        .collect::<Option<Vec<u16>>>()?;
    let gap = chars[end..].iter().take_while(|c| c.is_whitespace()).count();
    let title = end + gap;
    let rest: String = chars[title..].iter().collect();
    let rest = rest.trim_end();
    let fits = parts.len() >= 2
        && gap > 0
        && rest.chars().next().is_some_and(char::is_uppercase)
        && (2..=80).contains(&rest.chars().count())
        && !rest.contains("..")
        && !rest.ends_with(|c: char| c.is_ascii_digit());
    fits.then_some((parts, title))
}

/// The number an outline title starts with, like [2, 6] for "2.6 Matrices".
fn leading_number(title: &str) -> Option<Vec<u16>> {
    let (number, _) = title.split_once(' ')?;
    number.trim_end_matches('.').split('.').map(|part| part.parse().ok()).collect()
}

/// Adds scanned headings to an outline. Each goes under the entry whose
/// number is its closest prefix ("2.6.4" under "2.6", else "2"), after that
/// entry's other children, or else among the top-level entries by page.
/// Numbers already in the outline are skipped, so running headers and
/// repeats only count once.
pub fn merge(outline: &[OutlineEntry], headings: Vec<Heading>) -> Vec<OutlineEntry> {
    let mut merged = outline.to_vec();
    let mut numbers: Vec<Option<Vec<u16>>> = merged.iter().map(|entry| leading_number(&entry.title)).collect();
    for heading in headings {
        if numbers.iter().any(|number| number.as_ref() == Some(&heading.number)) {
            continue;
        }
        let parent = (1..heading.number.len())
            .rev()
            .find_map(|len| numbers.iter().rposition(|number| number.as_deref() == Some(&heading.number[..len])));
        let (at, depth) = match parent {
            Some(parent) => {
                let depth = merged[parent].depth;
                let end = (parent + 1..merged.len()).find(|&i| merged[i].depth <= depth).unwrap_or(merged.len());
                (end, depth + 1)
            }
            // No numbered parent: a top-level entry, in page order.
            None => {
                let after = |entry: &OutlineEntry| entry.depth == 0 && entry.page.is_some_and(|page| page > heading.page);
                (merged.iter().position(after).unwrap_or(merged.len()), 0)
            }
        };
        let entry = OutlineEntry {
            title: heading.title,
            page: Some(heading.page),
            top: Some(heading.top),
            depth,
            scanned: true,
        };
        merged.insert(at, entry);
        numbers.insert(at, Some(heading.number));
    }
    merged
}

fn render(doc: &Document, index: usize, bitmap: Bitmap) -> Option<Rendered> {
    let page = doc.load_page(index as i32).ok()?;
    let bounds = page.bounds().ok()?;
    let width = bitmap.width as f32;
    let height = (width * (bounds.y1 - bounds.y0) / (bounds.x1 - bounds.x0)).round().max(1.0);
    let area = IRect::new(0, 0, width as i32, height as i32);
    let content = |device: &Device, ctm: &Matrix| page.run(device, ctm);
    let pixmap = draw(bounds, content, &[], &Colorspace::device_rgb(), (width, height), false, area).ok()?;
    let image = image::Handle::from_rgba(pixmap.width(), pixmap.height(), pixmap.samples().to_vec());
    Some((image, links(doc, &page, bounds)))
}

/// Renders part of a page into a bitmap of just that size.
fn render_detail(doc: &Document, index: usize, detail: Detail) -> Option<image::Handle> {
    let page = doc.load_page(index as i32).ok()?;
    let bounds = page.bounds().ok()?;
    let width = detail.page_width as f32;
    let height = width * (bounds.y1 - bounds.y0) / (bounds.x1 - bounds.x0);
    let (x, y) = (detail.x as i32, detail.y as i32);
    let area = IRect::new(x, y, x + detail.width as i32, y + detail.height as i32);
    let content = |device: &Device, ctm: &Matrix| page.run(device, ctm);
    let pixmap = draw(bounds, content, &[], &Colorspace::device_rgb(), (width, height), false, area).ok()?;
    Some(image::Handle::from_rgba(detail.width, detail.height, pixmap.samples().to_vec()))
}

/// Draws a page's `content` and drawings on it, scaled to `size` in pixels
/// and turned a quarter clockwise if asked, into opaque pixels (four bytes
/// each, in the order of `colors`) covering just `area` of it.
fn draw(
    bounds: Rect,
    content: impl FnOnce(&Device, &Matrix) -> Result<(), mupdf::Error>,
    strokes: &[&Stroke],
    colors: &Colorspace,
    (width, height): (f32, f32),
    turn: bool,
    area: IRect,
) -> Result<Pixmap, mupdf::Error> {
    let (page_width, page_height) = (bounds.x1 - bounds.x0, bounds.y1 - bounds.y0);
    let (dx, dy) = (area.x0 as f32, area.y0 as f32);
    // MuPDF maps x' = a·x + c·y + e and y' = b·x + d·y + f.
    let ctm = if turn {
        // The page's left edge goes to the top.
        let (sx, sy) = (height / page_width, width / page_height);
        Matrix::new(0.0, sx, -sy, 0.0, width + bounds.y0 * sy - dx, -bounds.x0 * sx - dy)
    } else {
        let (sx, sy) = (width / page_width, height / page_height);
        Matrix::new(sx, 0.0, 0.0, sy, -bounds.x0 * sx - dx, -bounds.y0 * sy - dy)
    };
    let mut pixmap = Pixmap::new(colors, 0, 0, area.x1 - area.x0, area.y1 - area.y0, true)?;
    pixmap.clear_with(255)?;
    let device = Device::from_pixmap(&pixmap)?;
    content(&device, &ctm)?;
    ink(&device, bounds, strokes, &ctm)?;
    drop(device);
    Ok(pixmap)
}

/// Draws strokes onto a page being drawn with `ctm`.
fn ink(device: &Device, bounds: Rect, strokes: &[&Stroke], ctm: &Matrix) -> Result<(), mupdf::Error> {
    let (page_width, page_height) = (bounds.x1 - bounds.x0, bounds.y1 - bounds.y0);
    let rgb = Colorspace::device_rgb();
    for stroke in strokes {
        let Some(&(u, v)) = stroke.points.first() else { continue };
        // Strokes are stored as fractions of the page.
        let at = |(u, v): (f32, f32)| (bounds.x0 + u * page_width, bounds.y0 + v * page_height);
        let mut path = mupdf::Path::new()?;
        let (x, y) = at((u, v));
        path.move_to(x, y)?;
        // A single point still shows, as a dot.
        for &point in stroke.points.iter().skip(1).chain((stroke.points.len() == 1).then_some(&(u, v))) {
            let (x, y) = at(point);
            path.line_to(x, y)?;
        }
        use mupdf::stroke_state::{LineCap, LineJoin};
        let style = StrokeState::new(LineCap::Round, LineCap::Round, LineCap::Round, LineJoin::Round, stroke.width, 10.0, 0.0, &[])?;
        let color = stroke.color;
        let rgb_color = [color.r, color.g, color.b];
        device.stroke_path(&path, &style, ctm, &rgb, &rgb_color, color.a, ColorParams::default())?;
    }
    Ok(())
}

fn links(doc: &Document, page: &Page, bounds: Rect) -> Vec<Link> {
    let Ok(links) = page.links() else { return Vec::new() };
    links
        .filter_map(|link| {
            let area = fractions(bounds, link.bounds);
            let target = match link.dest {
                Some(dest) => target(doc, dest),
                None if link.uri.contains("://") || link.uri.starts_with("mailto:") => Target::Web(link.uri),
                None => return None,
            };
            Some(Link { area, target })
        })
        .collect()
}

fn target(doc: &Document, dest: LinkDestination) -> Target {
    let index = dest.loc.page_number as usize;
    let top = match dest.kind {
        DestinationKind::XYZ { top: Some(top), .. }
        | DestinationKind::FitH { top: Some(top) }
        | DestinationKind::FitBH { top: Some(top) } => {
            let bounds = doc.load_page(index as i32).and_then(|page| page.bounds()).ok();
            bounds.map(|b| ((top - b.y0) / (b.y1 - b.y0)).clamp(0.0, 1.0))
        }
        _ => None,
    };
    Target::Page(index, top)
}

/// The file as it should go to the printer: drawings burned in, and never
/// locked, so the system's printing doesn't need the password. A copy is
/// written page by page, which also drops the lock.
#[cfg(not(windows))]
fn export(path: &Path, locked: bool, doc: &Document, strokes: &[(usize, Stroke)]) -> Result<Vec<u8>, String> {
    if strokes.is_empty() && !locked {
        return std::fs::read(path).map_err(|e| e.to_string());
    }
    let copy = std::env::temp_dir().join(format!("rs-pdf-render-print-{}.pdf", std::process::id()));
    let written = (|| -> Result<(), mupdf::Error> {
        let mut writer = mupdf::DocumentWriter::new(&*copy, "pdf", "")?;
        for index in 0..doc.page_count()? {
            let page = doc.load_page(index)?;
            let bounds = page.bounds()?;
            let device = writer.begin_page(Rect::new(0.0, 0.0, bounds.x1 - bounds.x0, bounds.y1 - bounds.y0))?;
            let ctm = Matrix::new_translate(-bounds.x0, -bounds.y0);
            page.run(&device, &ctm)?;
            let on_page: Vec<&Stroke> = strokes.iter().filter(|(at, _)| *at == index as usize).map(|(_, s)| s).collect();
            ink(&device, bounds, &on_page, &ctm)?;
            writer.end_page(device)?;
        }
        Ok(())
    })();
    let result = written.map_err(|e| e.to_string()).and_then(|()| std::fs::read(&copy).map_err(|e| e.to_string()));
    let _ = std::fs::remove_file(&copy);
    result
}

/// Prints the pages the job asks for, drawings included, as pictures at the
/// printer's resolution.
#[cfg(windows)]
fn print(doc: &Document, strokes: &[(usize, Stroke)], job: &crate::print::Job, title: &str) -> Result<(), String> {
    // The page being printed, drawn in bands from a recording of it so
    // it's only read once.
    let mut current: Option<(usize, mupdf::DisplayList, Rect)> = None;
    let size = |index: usize| {
        let bounds = doc.load_page(index as i32).and_then(|page| page.bounds()).ok()?;
        Some((bounds.x1 - bounds.x0, bounds.y1 - bounds.y0))
    };
    let band = |index: usize, (width, height): (i32, i32), turn: bool, rows: std::ops::Range<i32>| {
        if current.as_ref().is_none_or(|(at, ..)| *at != index) {
            let page = doc.load_page(index as i32).ok()?;
            current = Some((index, page.to_display_list(true).ok()?, page.bounds().ok()?));
        }
        let (_, list, bounds) = current.as_ref()?;
        let on_page: Vec<&Stroke> = strokes.iter().filter(|(at, _)| *at == index).map(|(_, s)| s).collect();
        let area = IRect::new(0, rows.start, width, rows.end);
        let clip = Rect::new(0.0, 0.0, width as f32, rows.len() as f32);
        let content = |device: &Device, ctm: &Matrix| list.run(device, ctm, clip);
        let pixmap = draw(*bounds, content, &on_page, &Colorspace::device_bgr(), (width as f32, height as f32), turn, area);
        pixmap.ok().map(|pixmap| pixmap.samples().to_vec())
    };
    crate::print::print(job, title, size, band)
}
