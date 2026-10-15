mod ink;
mod pdf;
mod print;
mod ui;

use ink::Stroke;
use pdf::{OutlineEntry, Worker};

use iced::time::Instant;
use iced::widget::{image, operation, scrollable};
use iced::{Animation, Color, Event, Point, Size, Subscription, Task, Vector};
use iced::{event, keyboard, mouse, window};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Space around and between pages.
const GAP: f32 = 10.0;
/// 100% zoom shows a page at its printed size on a 96 DPI screen, like pdf.js.
const PT_TO_PX: f32 = 96.0 / 72.0;
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 10.0;
/// Rendered bitmaps are capped at this many pixels and scaled up past it.
const MAX_BITMAP_PIXELS: f32 = 16_777_216.0;
const SIDEBAR_WIDTH: f32 = 240.0;
const SCROLLBAR_WIDTH: f32 = 8.0;
const TOOLBAR_HEIGHT: f32 = 40.0;
/// The toolbar appears when the cursor comes this close to the top edge...
const REVEAL_DISTANCE: f32 = 12.0;
/// ...and goes away once the cursor is this far below the toolbar.
const HIDE_DISTANCE: f32 = 40.0;
const DOCUMENT: &str = "document";

fn main() -> iced::Result {
    let path = std::env::args_os().nth(1).map(PathBuf::from);

    iced::application(move || App::new(path.clone()), App::update, ui::view)
        .title(App::title)
        .subscription(App::subscription)
        .theme(iced::Theme::Dark)
        .window(window::Settings {
            size: Size::new(1000.0, 800.0),
            min_size: Some(Size::new(520.0, 320.0)),
            decorations: false,
            ..window::Settings::default()
        })
        .run()
}

pub struct App {
    pdf: Result<Worker, String>,
    window: Option<window::Id>,
    window_size: Size,
    scale_factor: f32,
    modifiers: keyboard::Modifiers,
    toolbar: Animation<bool>,
    now: Instant,
    sidebar: bool,
    drawing: bool,
    pen: Color,
    doc: Option<Document>,
    next_id: u64,
    error: Option<String>,
}

pub struct Document {
    id: u64,
    name: String,
    pages: Vec<Page>,
    outline: Vec<OutlineEntry>,
    /// Outline entries whose children are hidden.
    collapsed: HashSet<usize>,
    zoom: f32,
    /// pdf.js "Automatic" zoom: refit when the view changes size.
    auto_zoom: bool,
    /// Clockwise, in degrees.
    rotation: u16,
    current: usize,
    offset: Vector,
    /// What the user is typing into the page and zoom boxes, if anything.
    page_input: Option<String>,
    zoom_input: Option<String>,
    /// Pages in the order strokes were drawn, for undo.
    history: Vec<usize>,
}

pub struct Page {
    /// Unrotated size in PDF points.
    size: Size,
    image: Option<(u32, image::Handle)>,
    pending: Option<u32>,
    strokes: Vec<Stroke>,
}

#[derive(Debug, Clone)]
pub enum Message {
    WindowReady(window::Id, f32),
    WindowResized(Size),
    Rescaled(f32),
    CursorMoved(Point),
    CursorLeft,
    Key(keyboard::Event),
    Frame(Instant),
    DragWindow,
    Minimize,
    ToggleMaximize,
    Close,

    PickFile,
    Open(PathBuf),
    Loaded(PathBuf, Result<pdf::DocInfo, String>),
    Rendered { doc: u64, page: usize, width: u32, image: Option<image::Handle> },

    Scrolled(scrollable::Viewport),
    ZoomIn,
    ZoomOut,
    ZoomInput(String),
    ZoomSubmit,
    WheelZoom(f32, Point),
    PageInput(String),
    PageSubmit,
    GoTo(usize),
    RotateCounterclockwise,
    ToggleSidebar,
    ToggleOutlineEntry(usize),

    ToggleDrawing,
    PenColor(Color),
    StrokeDone(usize, Stroke),
    Undo,

    Print,
    Printable(Result<Vec<u8>, String>),
    Printed(Result<(), String>),
    DismissError,
}

impl App {
    fn new(path: Option<PathBuf>) -> (Self, Task<Message>) {
        let app = Self {
            pdf: Worker::spawn(),
            window: None,
            window_size: Size::new(1000.0, 800.0),
            scale_factor: 1.0,
            modifiers: keyboard::Modifiers::default(),
            toolbar: Animation::new(true).very_quick(),
            now: Instant::now(),
            sidebar: false,
            drawing: false,
            pen: ui::PEN_COLORS[0],
            doc: None,
            next_id: 0,
            error: None,
        };
        let ready = window::oldest().and_then(|id| {
            window::scale_factor(id).map(move |scale| Message::WindowReady(id, scale))
        });
        let open = path.map_or_else(Task::none, |path| Task::done(Message::Open(path)));
        (app, Task::batch([ready, open]))
    }

    fn title(&self) -> String {
        match &self.doc {
            Some(doc) => doc.name.clone(),
            None => "PDF Reader".into(),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let events = event::listen_with(|event, _status, _window| match event {
            Event::Window(window::Event::Resized(size)) => Some(Message::WindowResized(size)),
            Event::Window(window::Event::Rescaled(scale)) => Some(Message::Rescaled(scale)),
            Event::Window(window::Event::FileDropped(path)) => Some(Message::Open(path)),
            Event::Mouse(mouse::Event::CursorMoved { position }) => Some(Message::CursorMoved(position)),
            Event::Mouse(mouse::Event::CursorLeft) => Some(Message::CursorLeft),
            _ => None,
        });
        let keys = keyboard::listen().map(Message::Key);
        let frames = if self.toolbar.is_animating(self.now) {
            window::frames().map(Message::Frame)
        } else {
            Subscription::none()
        };
        Subscription::batch([events, keys, frames])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::WindowReady(id, scale) => {
                self.window = Some(id);
                self.scale_factor = scale;
                return window::size(id).map(Message::WindowResized);
            }
            Message::WindowResized(size) => {
                self.window_size = size;
                return self.view_resized();
            }
            Message::Rescaled(scale) => {
                self.scale_factor = scale;
                return self.sync_renders();
            }
            Message::CursorMoved(position) => {
                let show = if self.toolbar.value() {
                    position.y <= TOOLBAR_HEIGHT + HIDE_DISTANCE
                } else {
                    position.y <= REVEAL_DISTANCE
                };
                self.show_toolbar(show);
            }
            Message::CursorLeft => self.show_toolbar(false),
            Message::Key(event) => return self.key(event),
            Message::Frame(now) => self.now = now,
            Message::DragWindow => return self.window.map_or_else(Task::none, window::drag),
            Message::Minimize => {
                return self.window.map_or_else(Task::none, |id| window::minimize(id, true));
            }
            Message::ToggleMaximize => {
                return self.window.map_or_else(Task::none, window::toggle_maximize);
            }
            Message::Close => return iced::exit(),

            Message::PickFile => {
                return Task::perform(
                    rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).pick_file(),
                    |file| file.map(|file| Message::Open(file.path().to_path_buf())),
                )
                .and_then(Task::done);
            }
            Message::Open(path) => {
                let Ok(worker) = &self.pdf else { return Task::none() };
                return Task::perform(worker.clone().open(path.clone()), move |result| {
                    Message::Loaded(path.clone(), result)
                });
            }
            Message::Loaded(path, result) => match result {
                Ok(info) => return self.load(path, info),
                Err(error) => self.error = Some(error),
            },
            Message::Rendered { doc, page, width, image } => {
                if let Some(page) = self
                    .doc
                    .as_mut()
                    .filter(|d| d.id == doc)
                    .and_then(|d| d.pages.get_mut(page))
                    .filter(|p| p.pending == Some(width))
                {
                    page.pending = None;
                    if let Some(image) = image {
                        page.image = Some((width, image));
                    }
                }
            }

            Message::Scrolled(viewport) => {
                if let Some(doc) = &mut self.doc {
                    let offset = viewport.absolute_offset();
                    doc.offset = Vector::new(offset.x, offset.y);
                    return self.sync_renders();
                }
            }
            Message::ZoomIn => {
                if let Some(doc) = &self.doc {
                    // Same steps as pdf.js: 10% at a time, rounded up to a tenth.
                    let zoom = ((doc.zoom * 1.1 * 100.0).round() / 10.0).ceil() / 10.0;
                    return self.zoom(zoom, None);
                }
            }
            Message::ZoomOut => {
                if let Some(doc) = &self.doc {
                    let zoom = ((doc.zoom / 1.1 * 100.0).round() / 10.0).floor() / 10.0;
                    return self.zoom(zoom, None);
                }
            }
            Message::ZoomInput(text) => {
                if let Some(doc) = &mut self.doc {
                    doc.zoom_input = Some(text);
                }
            }
            Message::ZoomSubmit => {
                if let Some(doc) = &mut self.doc {
                    let typed = doc.zoom_input.take().unwrap_or_default();
                    if let Ok(percent) = typed.trim().trim_end_matches('%').trim().parse::<f32>() {
                        return self.zoom(percent / 100.0, None);
                    }
                }
            }
            Message::WheelZoom(steps, at) => {
                if let Some(doc) = &self.doc {
                    return self.zoom(doc.zoom * 1.1f32.powf(steps), Some(at));
                }
            }
            Message::PageInput(text) => {
                if let Some(doc) = &mut self.doc {
                    doc.page_input = Some(text.chars().filter(char::is_ascii_digit).collect());
                }
            }
            Message::PageSubmit => {
                if let Some(doc) = &mut self.doc {
                    let typed = doc.page_input.take().unwrap_or_default();
                    let count = doc.pages.len();
                    if let Ok(number) = typed.parse::<usize>() {
                        return self.go_to(number.clamp(1, count) - 1);
                    }
                }
            }
            Message::GoTo(page) => return self.go_to(page),
            Message::RotateCounterclockwise => {
                if let Some(doc) = &mut self.doc {
                    // Keep the top of the current page in view.
                    doc.rotation = (doc.rotation + 270) % 360;
                    let page = doc.current;
                    return self.go_to(page);
                }
            }
            Message::ToggleSidebar => {
                self.sidebar = !self.sidebar;
                return self.view_resized();
            }
            Message::ToggleOutlineEntry(entry) => {
                if let Some(doc) = &mut self.doc
                    && !doc.collapsed.remove(&entry)
                {
                    doc.collapsed.insert(entry);
                }
            }

            Message::ToggleDrawing => self.drawing = !self.drawing && self.doc.is_some(),
            Message::PenColor(color) => self.pen = color,
            Message::StrokeDone(page, stroke) => {
                if let Some(doc) = &mut self.doc {
                    doc.pages[page].strokes.push(stroke);
                    doc.history.push(page);
                }
            }
            Message::Undo => {
                if let Some(doc) = &mut self.doc
                    && let Some(page) = doc.history.pop()
                {
                    doc.pages[page].strokes.pop();
                }
            }

            Message::Print => {
                if let (Some(doc), Ok(worker)) = (&self.doc, &self.pdf) {
                    let strokes = doc
                        .pages
                        .iter()
                        .enumerate()
                        .flat_map(|(i, page)| page.strokes.iter().map(move |s| (i, s.clone())))
                        .collect();
                    return Task::perform(worker.clone().export(strokes), Message::Printable);
                }
            }
            Message::Printable(Ok(bytes)) => {
                if let Some(id) = self.window {
                    return window::run(id, move |window| print::print(window, bytes))
                        .map(Message::Printed);
                }
            }
            Message::Printable(Err(error)) | Message::Printed(Err(error)) => {
                self.error = Some(error);
            }
            Message::Printed(Ok(())) => {}
            Message::DismissError => self.error = None,
        }
        Task::none()
    }

    fn key(&mut self, event: keyboard::Event) -> Task<Message> {
        use keyboard::Key;
        use keyboard::key::Named;

        let keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
            if let keyboard::Event::ModifiersChanged(modifiers) = event {
                self.modifiers = modifiers;
            }
            return Task::none();
        };
        let page_height = self.viewport().height - 2.0 * GAP;

        match key.as_ref() {
            Key::Character("o") if modifiers.command() => return self.update(Message::PickFile),
            _ if self.doc.is_none() => {}
            Key::Character("p") if modifiers.command() => return self.update(Message::Print),
            Key::Character("=" | "+") if modifiers.command() => return self.update(Message::ZoomIn),
            Key::Character("-") if modifiers.command() => return self.update(Message::ZoomOut),
            Key::Character("0") if modifiers.command() => {
                let zoom = self.auto_zoom();
                let task = self.zoom(zoom, None);
                if let Some(doc) = &mut self.doc {
                    doc.auto_zoom = true;
                }
                return task;
            }
            Key::Character("z") if modifiers.command() => return self.update(Message::Undo),
            Key::Named(Named::Escape) => self.drawing = false,
            Key::Named(Named::Home) => return self.go_to(0),
            Key::Named(Named::End) => {
                let last = self.doc.as_ref().map_or(0, |doc| doc.pages.len() - 1);
                return self.go_to(last);
            }
            Key::Named(Named::PageDown | Named::Space) => return self.scroll_by(page_height),
            Key::Named(Named::PageUp) => return self.scroll_by(-page_height),
            Key::Named(Named::ArrowDown) => return self.scroll_by(40.0),
            Key::Named(Named::ArrowUp) => return self.scroll_by(-40.0),
            _ => {}
        }
        Task::none()
    }

    fn show_toolbar(&mut self, show: bool) {
        let show = show || self.doc.is_none();
        if show == self.toolbar.value() {
            return;
        }
        self.now = Instant::now();
        self.toolbar.go_mut(show, self.now);
        if let Some(doc) = &mut self.doc
            && !show
        {
            doc.page_input = None;
            doc.zoom_input = None;
        }
    }

    fn load(&mut self, path: PathBuf, info: pdf::DocInfo) -> Task<Message> {
        if info.pages.is_empty() {
            self.error = Some("This PDF has no pages.".into());
            return Task::none();
        }
        self.next_id += 1;
        self.error = None;
        self.drawing = false;
        // Like pdf.js, start with only the top level of the outline showing.
        let collapsed = (0..info.outline.len())
            .filter(|&i| info.outline.get(i + 1).is_some_and(|next| next.depth > info.outline[i].depth))
            .collect();
        self.doc = Some(Document {
            id: self.next_id,
            name: path
                .file_name()
                .map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into()),
            pages: info
                .pages
                .into_iter()
                .map(|(w, h)| Page { size: Size::new(w, h), image: None, pending: None, strokes: Vec::new() })
                .collect(),
            outline: info.outline,
            collapsed,
            zoom: 1.0,
            auto_zoom: true,
            rotation: 0,
            current: 0,
            offset: Vector::ZERO,
            page_input: None,
            zoom_input: None,
            history: Vec::new(),
        });
        let zoom = self.auto_zoom();
        if let Some(doc) = &mut self.doc {
            doc.zoom = zoom;
            if let Ok(v) = std::env::var("DBG") {
                let first = self.scroll_to(Vector::ZERO);
                return match v.as_str() {
                    "goto" => Task::batch([first, Task::done(Message::GoTo(9))]),
                    _ => Task::batch([first, Task::done(Message::GoTo(9)), Task::done(Message::RotateCounterclockwise)]),
                };
            }
        }
        self.scroll_to(Vector::ZERO)
    }

    /// Area the document scrolls in.
    fn viewport(&self) -> Size {
        let sidebar = if self.sidebar && self.doc.is_some() { SIDEBAR_WIDTH } else { 0.0 };
        Size::new((self.window_size.width - sidebar).max(1.0), self.window_size.height)
    }

    /// pdf.js "Automatic": fit the page width, but no bigger than 125%.
    fn auto_zoom(&self) -> f32 {
        let Some(doc) = &self.doc else { return 1.0 };
        let page = doc.pages[doc.current].size;
        let view = self.viewport();
        let (w, h) = if doc.rotation % 180 == 90 { (page.height, page.width) } else { (page.width, page.height) };
        // Leave room for the scrollbar so it never covers the page.
        let fit_width = (view.width - 2.0 * GAP - 2.0 * SCROLLBAR_WIDTH) / (w * PT_TO_PX);
        let fit_height = (view.height - 2.0 * GAP) / (h * PT_TO_PX);
        let fit = if w > h { fit_width.min(fit_height) } else { fit_width };
        fit.clamp(MIN_ZOOM, 1.25)
    }

    /// Changes zoom while keeping the document point under `anchor` (or the
    /// middle of the view) in place.
    fn zoom(&mut self, zoom: f32, anchor: Option<Point>) -> Task<Message> {
        let view = self.viewport();
        let Some(doc) = &mut self.doc else { return Task::none() };
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        doc.zoom_input = None;
        doc.auto_zoom = false;
        if zoom == doc.zoom {
            return Task::none();
        }
        let anchor = anchor.unwrap_or(Point::new(view.width / 2.0, view.height / 2.0));
        let spot = doc.locate(doc.offset.y + anchor.y);
        let fraction_x = (doc.offset.x + anchor.x) / doc.content_size(view).width;

        doc.zoom = zoom;
        let offset = Vector::new(
            fraction_x * doc.content_size(view).width - anchor.x,
            doc.y_of(spot) - anchor.y,
        );
        self.scroll_to(offset)
    }

    fn view_resized(&mut self) -> Task<Message> {
        let Some(doc) = &self.doc else { return Task::none() };
        if !doc.auto_zoom {
            return self.scroll_to(doc.offset);
        }
        let task = self.zoom(self.auto_zoom(), None);
        if let Some(doc) = &mut self.doc {
            doc.auto_zoom = true;
        }
        Task::batch([task, self.scroll_to(self.doc.as_ref().map_or(Vector::ZERO, |doc| doc.offset))])
    }

    fn go_to(&mut self, page: usize) -> Task<Message> {
        let Some(doc) = &mut self.doc else { return Task::none() };
        doc.page_input = None;
        let offset = Vector::new(doc.offset.x, doc.tops()[page] - GAP);
        let task = self.scroll_to(offset);
        if let Some(doc) = &mut self.doc {
            doc.current = page;
        }
        task
    }

    fn scroll_by(&mut self, dy: f32) -> Task<Message> {
        let Some(doc) = &self.doc else { return Task::none() };
        self.scroll_to(doc.offset + Vector::new(0.0, dy))
    }

    fn scroll_to(&mut self, offset: Vector) -> Task<Message> {
        let view = self.viewport();
        let Some(doc) = &mut self.doc else { return Task::none() };
        let content = doc.content_size(view);
        doc.offset = Vector::new(
            offset.x.clamp(0.0, (content.width - view.width).max(0.0)),
            offset.y.clamp(0.0, (content.height - view.height).max(0.0)),
        );
        let scroll = operation::scroll_to(
            DOCUMENT,
            scrollable::AbsoluteOffset { x: doc.offset.x, y: doc.offset.y },
        );
        Task::batch([scroll, self.sync_renders()])
    }

    /// Requests bitmaps for pages near the view, drops the rest, and updates
    /// the current page number.
    fn sync_renders(&mut self) -> Task<Message> {
        let view = self.viewport();
        let scale_factor = self.scale_factor;
        let (Some(doc), Ok(worker)) = (&mut self.doc, &self.pdf) else { return Task::none() };

        let tops = doc.tops();
        // Render half a screen past each edge so scrolling rarely shows a blank page.
        let (near_start, near_end) = (doc.offset.y - view.height / 2.0, doc.offset.y + 1.5 * view.height);
        let (view_start, view_end) = (doc.offset.y, doc.offset.y + view.height);
        let mut most_visible = (0.0, doc.current);
        let mut wanted = HashMap::new();
        let mut tasks = Vec::new();

        for (index, top) in tops.into_iter().enumerate() {
            let size = doc.page_size(index);
            let bottom = top + size.height;
            let visible = bottom.min(view_end) - top.max(view_start);
            if visible > most_visible.0 {
                most_visible = (visible, index);
            }
            let page = &mut doc.pages[index];
            if bottom < near_start || top > near_end {
                page.image = None;
                page.pending = None;
                continue;
            }

            let mut width = page.size.width * doc.zoom * PT_TO_PX * scale_factor;
            let pixels = width * width * page.size.height / page.size.width;
            if pixels > MAX_BITMAP_PIXELS {
                width *= (MAX_BITMAP_PIXELS / pixels).sqrt();
            }
            let width = width.round().max(1.0) as u32;
            wanted.insert(index, width);
            if page.image.as_ref().is_some_and(|(w, _)| *w == width) || page.pending == Some(width) {
                continue;
            }
            page.pending = Some(width);
            let id = doc.id;
            tasks.push(Task::perform(worker.clone().render(index, width), move |image| {
                Message::Rendered { doc: id, page: index, width, image }
            }));
        }

        doc.current = most_visible.1;
        worker.set_wanted(wanted);
        Task::batch(tasks)
    }
}

impl Document {
    /// On-screen size of a page at the current zoom and rotation.
    fn page_size(&self, index: usize) -> Size {
        let Size { width, height } = self.pages[index].size;
        let scale = self.zoom * PT_TO_PX;
        if self.rotation % 180 == 90 {
            Size::new(height * scale, width * scale)
        } else {
            Size::new(width * scale, height * scale)
        }
    }

    fn tops(&self) -> Vec<f32> {
        let mut y = GAP;
        (0..self.pages.len())
            .map(|i| {
                let top = y;
                y += self.page_size(i).height + GAP;
                top
            })
            .collect()
    }

    fn content_size(&self, view: Size) -> Size {
        let (width, height) = (0..self.pages.len())
            .map(|i| self.page_size(i))
            .fold((0.0f32, GAP), |(w, h), size| (w.max(size.width), h + size.height + GAP));
        Size::new((width + 2.0 * GAP).ceil().max(view.width), height)
    }

    /// The page at height `y` and how far down that page `y` is (0 to 1).
    fn locate(&self, y: f32) -> (usize, f32) {
        let tops = self.tops();
        let index = tops.partition_point(|&top| top <= y).saturating_sub(1);
        let fraction = (y - tops[index]) / self.page_size(index).height;
        (index, fraction.clamp(0.0, 1.0))
    }

    fn y_of(&self, (index, fraction): (usize, f32)) -> f32 {
        self.tops()[index] + fraction * self.page_size(index).height
    }
}
