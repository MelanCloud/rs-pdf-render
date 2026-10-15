// Release builds on Windows shouldn't open a console window next to the app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ink;
mod pdf;
mod print;
mod settings;
mod system;
mod ui;

use ink::Stroke;
use pdf::{Heading, Link, OpenError, OutlineEntry, Target, Worker};
use settings::{Position, Settings, ThemeChoice, Zoom};

use iced::time::Instant;
use iced::widget::{image, operation, scrollable};
use iced::{Animation, Color, Event, Point, Size, Subscription, Task, Vector};
use iced::{event, keyboard, mouse, window};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

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
/// Pages scanned for headings per request, so page renders can go in between.
const SCAN_BATCH: usize = 8;
/// How long a smooth scroll takes to settle.
const GLIDE: Duration = Duration::from_millis(160);
const DOCUMENT: &str = "document";
const PASSWORD: &str = "password";

fn main() -> iced::Result {
    // On Windows, wgpu picks Vulkan by itself; DirectX 12 used about 70 MB
    // less memory for the same work. Without DirectX 12, iced falls back to
    // its software renderer.
    #[cfg(windows)]
    if std::env::var_os("WGPU_BACKEND").is_none() {
        // SAFETY: no other threads are running yet.
        unsafe { std::env::set_var("WGPU_BACKEND", "dx12") };
    }
    let path = std::env::args_os().nth(1).map(PathBuf::from);

    iced::application(move || App::new(path.clone()), App::update, ui::view)
        .title(App::title)
        .subscription(App::subscription)
        .theme(App::theme)
        .window(window::Settings {
            size: Size::new(1000.0, 800.0),
            min_size: Some(Size::new(520.0, 320.0)),
            // Windows and Linux show it in the taskbar; macOS uses the app bundle's.
            icon: window::icon::from_rgba(include_bytes!("../assets/icon-64.rgba").to_vec(), 64, 64).ok(),
            decorations: false,
            // Closing goes through `Message::Close`, which saves where the
            // open file was left first.
            exit_on_close_request: false,
            // Keeps Windows' shadow, rounded corners and border, which tiling
            // window managers like GlazeWM color to show the focused window.
            #[cfg(windows)]
            platform_specific: window::settings::PlatformSpecific {
                undecorated_shadow: true,
                ..window::settings::PlatformSpecific::default()
            },
            ..window::Settings::default()
        })
        .run()
}

pub struct App {
    pdf: Result<Worker, String>,
    window: Option<window::Id>,
    window_size: Size,
    /// A resize has come in, so the size asked for at startup is out of date.
    resized: bool,
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
    locked: Option<Locked>,
    settings: Settings,
    menu: Option<Menu>,
    /// The pointer is over the open dropdown, so a click there keeps it open.
    over_menu: bool,
    /// The dropdown the last mouse press closed, so that clicking its own
    /// button closes it instead of opening it again on release.
    closed_by_press: Option<Menu>,
    /// Whether the system uses a dark theme, for the "System" choice.
    system_dark: bool,
}

/// The dropdowns under the toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    /// Open and Print, under the menu button.
    File,
    Settings,
}

/// A place in a document: a page, and how far down it the top of the view
/// is, from 0 to 1. It stays right whatever the zoom.
type Spot = (usize, f32);

/// A file waiting for its password.
pub struct Locked {
    path: PathBuf,
    input: String,
    /// The last password tried was wrong.
    wrong: bool,
}

pub struct Document {
    id: u64,
    path: PathBuf,
    name: String,
    pages: Vec<Page>,
    /// What the sidebar shows: the file's outline, plus scanned headings.
    outline: Vec<OutlineEntry>,
    /// The file's own outline, to go back to when scanning is turned off.
    file_outline: Vec<OutlineEntry>,
    /// A scan for numbered headings in progress.
    scan: Option<Scan>,
    /// Outline entries whose children are hidden.
    collapsed: HashSet<usize>,
    zoom: f32,
    /// The zoom mode to refit with when the view changes size, until the user
    /// picks a zoom.
    fit: Option<Zoom>,
    /// Clockwise, in degrees.
    rotation: u16,
    current: usize,
    offset: Vector,
    /// What the user is typing into the page and zoom boxes, if anything.
    page_input: Option<String>,
    zoom_input: Option<String>,
    /// Pages in the order strokes were drawn, for undo.
    history: Vec<usize>,
    /// Strokes taken back by undo, most recent last, for redo.
    undone: Vec<(usize, Stroke)>,
    glide: Option<Glide>,
    /// Where the view was before each jump (outline, link, page box), for Back,
    /// and where Back left from, for Forward.
    back: Vec<Spot>,
    forward: Vec<Spot>,
}

/// How far a scan for numbered headings has got.
#[derive(Default)]
pub struct Scan {
    /// The next page to scan.
    next: usize,
    found: Vec<Heading>,
}

/// A smooth scroll in progress.
#[derive(Clone, Copy)]
pub struct Glide {
    from: Vector,
    to: Vector,
    start: Instant,
}

pub struct Page {
    /// Unrotated size in PDF points.
    size: Size,
    image: Option<(u32, image::Handle)>,
    pending: Option<u32>,
    strokes: Vec<Stroke>,
    /// Kept when the bitmap is dropped; they're small.
    links: Vec<Link>,
}

#[derive(Debug, Clone)]
pub enum Message {
    WindowReady(window::Id, f32),
    FirstSize(Size),
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
    /// The file, whether a password was tried, and how opening went.
    Loaded(PathBuf, bool, Result<pdf::DocInfo, OpenError>),
    PasswordInput(String),
    Unlock,
    CancelPassword,
    /// Escape, even when a text box takes it.
    Escape,
    Rendered { doc: u64, page: usize, width: u32, rendered: Option<pdf::Rendered> },

    Scrolled(scrollable::Viewport),
    ZoomIn,
    ZoomOut,
    ZoomInput(String),
    ZoomSubmit,
    WheelZoom(f32, Point),
    /// A mouse wheel notch to scroll smoothly.
    WheelScroll(Vector),
    PageInput(String),
    PageSubmit,
    /// Go to a page (and a spot on it) from the outline, remembering the way back.
    Jump(usize, Option<f32>),
    /// Headings found on pages up to the given one, for a document id.
    Scanned(u64, usize, Vec<Heading>),
    FollowLink(Target),
    Back,
    Forward,
    RotateCounterclockwise,
    ToggleSidebar,
    ToggleOutlineEntry(usize),

    ToggleDrawing,
    PenColor(Color),
    StrokeDone(usize, Stroke),
    Undo,
    Redo,

    Print,
    /// The PDF to hand to the system's printing, drawings included.
    #[cfg(not(windows))]
    Printable(Result<Vec<u8>, String>),
    Printed(Result<(), String>),
    DismissError,

    ToggleMenu(Menu),
    /// The pointer went onto or off the open dropdown.
    OverMenu(bool),
    /// Any mouse press anywhere, even one a widget handles.
    Pressed,
    SetTheme(ThemeChoice),
    SetSmoothScrolling(bool),
    SetAnimateToolbars(bool),
    SetAlwaysShowToolbars(bool),
    SetDefaultZoom(Zoom),
    SetScrollSpeed(f32),
    SetScanSubsections(bool),
    /// The scroll speed slider was let go, so its value is final.
    SaveSettings,
    SystemTheme(iced::theme::Mode),
}

impl App {
    fn new(path: Option<PathBuf>) -> (Self, Task<Message>) {
        let app = Self {
            pdf: Worker::spawn(),
            window: None,
            window_size: Size::new(1000.0, 800.0),
            resized: false,
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
            locked: None,
            settings: Settings::load(),
            menu: None,
            over_menu: false,
            closed_by_press: None,
            system_dark: true,
        };
        let ready = window::oldest().and_then(|id| {
            window::scale_factor(id).map(move |scale| Message::WindowReady(id, scale))
        });
        let open = path.map_or_else(Task::none, |path| Task::done(Message::Open(path)));
        let theme = iced::system::theme().map(Message::SystemTheme);
        (app, Task::batch([ready, open, theme]))
    }

    fn title(&self) -> String {
        match &self.doc {
            Some(doc) => doc.name.clone(),
            None => "PDF Reader".into(),
        }
    }

    fn theme(&self) -> iced::Theme {
        let dark = match self.settings.theme {
            ThemeChoice::System => self.system_dark,
            ThemeChoice::Light => false,
            ThemeChoice::Dark => true,
        };
        if dark { iced::Theme::Dark } else { iced::Theme::Light }
    }

    /// How much of the toolbars shows, from 0 to `TOOLBAR_HEIGHT`.
    fn reveal(&self) -> f32 {
        if self.settings.animate_toolbars {
            self.toolbar.interpolate(0.0, TOOLBAR_HEIGHT, self.now)
        } else if self.toolbar.value() {
            TOOLBAR_HEIGHT
        } else {
            0.0
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let events = event::listen_with(|event, _status, _window| match event {
            Event::Window(window::Event::Resized(size)) => Some(Message::WindowResized(size)),
            Event::Window(window::Event::Rescaled(scale)) => Some(Message::Rescaled(scale)),
            Event::Window(window::Event::FileDropped(path)) => Some(Message::Open(path)),
            Event::Window(window::Event::CloseRequested) => Some(Message::Close),
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Back)) => Some(Message::Back),
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Forward)) => Some(Message::Forward),
            Event::Mouse(mouse::Event::CursorMoved { position }) => Some(Message::CursorMoved(position)),
            Event::Mouse(mouse::Event::CursorLeft) => Some(Message::CursorLeft),
            Event::Mouse(mouse::Event::ButtonPressed(_)) => Some(Message::Pressed),
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            }) => Some(Message::Escape),
            _ => None,
        });
        let keys = keyboard::listen().map(Message::Key);
        let animating = self.settings.animate_toolbars && self.toolbar.is_animating(self.now);
        let gliding = self.doc.as_ref().is_some_and(|doc| doc.glide.is_some());
        let frames = if animating || gliding {
            window::frames().map(Message::Frame)
        } else {
            Subscription::none()
        };
        let theme = iced::system::theme_changes().map(Message::SystemTheme);
        Subscription::batch([events, keys, frames, theme])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        if matches!(message, Message::PickFile | Message::Print) {
            self.menu = None;
        }
        match message {
            Message::WindowReady(id, scale) => {
                self.window = Some(id);
                self.scale_factor = scale;
                return window::size(id).map(Message::FirstSize);
            }
            // The size asked for at startup can arrive after a newer resize
            // (tiling window managers resize new windows at once), so it only
            // counts if no resize has come in yet.
            Message::FirstSize(_) if self.resized => {}
            Message::FirstSize(size) => {
                self.window_size = size;
                return self.view_resized();
            }
            Message::WindowResized(size) => {
                self.resized = true;
                self.window_size = size;
                return self.view_resized();
            }
            Message::Rescaled(scale) => {
                self.scale_factor = scale;
                return self.sync_renders();
            }
            Message::CursorMoved(position) => {
                // The bottom bar shows and hides with the toolbar, so either
                // edge counts. It only exists while a document is open.
                let bottom = self.window_size.height - position.y;
                let edge = if self.doc.is_some() { position.y.min(bottom) } else { position.y };
                let show = if self.toolbar.value() {
                    edge <= TOOLBAR_HEIGHT + HIDE_DISTANCE
                } else {
                    edge <= REVEAL_DISTANCE
                };
                self.show_toolbar(show);
            }
            Message::CursorLeft => self.show_toolbar(false),
            Message::Key(event) => return self.key(event),
            Message::Frame(now) => {
                self.now = now;
                let Some(doc) = &mut self.doc else { return Task::none() };
                let Some(glide) = doc.glide else { return Task::none() };
                // Ease out: fast at first, settling gently, like Chromium.
                let progress = (now.duration_since(glide.start).as_secs_f32() / GLIDE.as_secs_f32()).min(1.0);
                let eased = 1.0 - (1.0 - progress).powi(3);
                if progress >= 1.0 {
                    doc.glide = None;
                }
                return self.scroll_to(glide.from + (glide.to - glide.from) * eased);
            }
            Message::DragWindow => return self.window.map_or_else(Task::none, window::drag),
            Message::Minimize => {
                return self.window.map_or_else(Task::none, |id| window::minimize(id, true));
            }
            Message::ToggleMaximize => {
                return self.window.map_or_else(Task::none, window::toggle_maximize);
            }
            Message::Close => {
                self.remember_position();
                return iced::exit();
            }

            Message::PickFile => {
                return Task::perform(
                    rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).pick_file(),
                    |file| file.map(|file| Message::Open(file.path().to_path_buf())),
                )
                .and_then(Task::done);
            }
            Message::Open(path) => return self.open(path, None),
            Message::Loaded(path, tried_password, result) => match result {
                Ok(info) => {
                    self.locked = None;
                    return self.load(path, info);
                }
                // Like pdf.js: ask, and ask again after a wrong password.
                Err(OpenError::Password) => {
                    self.locked = Some(Locked { path, input: String::new(), wrong: tried_password });
                    return operation::focus(PASSWORD);
                }
                Err(OpenError::Failed(error)) => {
                    self.locked = None;
                    self.error = Some(error);
                }
            },
            Message::PasswordInput(text) => {
                if let Some(locked) = &mut self.locked {
                    locked.input = text;
                }
            }
            Message::Unlock => {
                if let Some(locked) = &mut self.locked {
                    let (path, password) = (locked.path.clone(), std::mem::take(&mut locked.input));
                    return self.open(path, Some(password));
                }
            }
            Message::CancelPassword => self.locked = None,
            Message::Escape => {
                if self.menu.is_some() {
                    self.menu = None;
                } else if self.locked.take().is_none() {
                    self.drawing = false;
                }
            }
            Message::Rendered { doc, page, width, rendered } => {
                if let Some(page) = self
                    .doc
                    .as_mut()
                    .filter(|d| d.id == doc)
                    .and_then(|d| d.pages.get_mut(page))
                    .filter(|p| p.pending == Some(width))
                {
                    page.pending = None;
                    if let Some((image, links)) = rendered {
                        page.image = Some((width, image));
                        page.links = links;
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
            Message::WheelScroll(delta) => return self.scroll_by(delta),
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
                        return self.jump(number.clamp(1, count) - 1, None);
                    }
                }
            }
            Message::Jump(page, top) => return self.jump(page, top),
            Message::Scanned(id, end, found) => {
                let Some(doc) = self.doc.as_mut().filter(|doc| doc.id == id) else { return Task::none() };
                let Some(scan) = &mut doc.scan else { return Task::none() };
                scan.found.extend(found);
                scan.next = end;
                if end < doc.pages.len() {
                    return self.scan_next();
                }
                let found = doc.scan.take().map(|scan| scan.found).unwrap_or_default();
                let outline = pdf::merge(&doc.file_outline, found);
                doc.collapsed = keep_collapsed(&doc.outline, &doc.collapsed, &outline);
                doc.outline = outline;
            }
            Message::FollowLink(Target::Page(page, top)) => return self.jump(page, top),
            Message::FollowLink(Target::Web(url)) => system::open_link(&url),
            Message::Back => return self.back(false),
            Message::Forward => return self.back(true),
            Message::RotateCounterclockwise => {
                let Some(doc) = &mut self.doc else { return Task::none() };
                doc.rotation = (doc.rotation + 270) % 360;
                let page = doc.current;
                // Like pdf.js: refit in a zoom mode, and show the top of the
                // current page.
                if let Some(mode) = doc.fit {
                    let zoom = self.fit_zoom(mode);
                    if let Some(doc) = &mut self.doc {
                        doc.zoom = zoom;
                    }
                }
                return self.go_to(page);
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
                    doc.undone.clear();
                }
            }
            Message::Undo => {
                if let Some(doc) = &mut self.doc
                    && let Some(page) = doc.history.pop()
                    && let Some(stroke) = doc.pages[page].strokes.pop()
                {
                    doc.undone.push((page, stroke));
                }
            }
            Message::Redo => {
                if let Some(doc) = &mut self.doc
                    && let Some((page, stroke)) = doc.undone.pop()
                {
                    doc.pages[page].strokes.push(stroke);
                    doc.history.push(page);
                }
            }

            Message::Print => {
                let (Some(doc), Ok(worker)) = (&self.doc, &self.pdf) else { return Task::none() };
                let strokes: Vec<_> = doc
                    .pages
                    .iter()
                    .enumerate()
                    .flat_map(|(i, page)| page.strokes.iter().map(move |s| (i, s.clone())))
                    .collect();
                // Windows asks for a printer first, then the PDFium thread prints.
                #[cfg(windows)]
                {
                    let Some(id) = self.window else { return Task::none() };
                    let (worker, pages) = (worker.clone(), doc.pages.len());
                    return window::run(id, print::owner).then(move |owner| {
                        let (worker, strokes) = (worker.clone(), strokes.clone());
                        Task::perform(
                            async move {
                                let Some(job) = print::ask(owner, pages).await? else { return Ok(()) };
                                worker.print(strokes, job).await
                            },
                            Message::Printed,
                        )
                    });
                }
                #[cfg(not(windows))]
                return Task::perform(worker.clone().export(strokes), Message::Printable);
            }
            #[cfg(not(windows))]
            Message::Printable(Ok(pdf)) => {
                if let Some(id) = self.window {
                    return window::run(id, move |window| print::print(window, pdf)).map(Message::Printed);
                }
            }
            #[cfg(not(windows))]
            Message::Printable(Err(error)) => self.error = Some(error),
            Message::Printed(Err(error)) => self.error = Some(error),
            Message::Printed(Ok(())) => {}
            Message::DismissError => self.error = None,

            Message::ToggleMenu(menu) => {
                let closed_now = self.closed_by_press.take() == Some(menu);
                self.menu = (self.menu != Some(menu) && !closed_now).then_some(menu);
                self.over_menu = false;
                self.show_toolbar(true);
            }
            Message::OverMenu(over) => self.over_menu = over,
            // Like a browser menu: a press outside the dropdown closes it, and
            // still does whatever it was pressing (open an outline entry...).
            Message::Pressed => {
                self.closed_by_press = None;
                if !self.over_menu {
                    self.closed_by_press = self.menu.take();
                }
            }
            Message::SetTheme(theme) => {
                self.settings.theme = theme;
                self.settings.save();
            }
            Message::SetSmoothScrolling(on) => {
                self.settings.smooth_scrolling = on;
                self.settings.save();
            }
            Message::SetAnimateToolbars(on) => {
                self.settings.animate_toolbars = on;
                self.settings.save();
            }
            Message::SetAlwaysShowToolbars(on) => {
                self.settings.always_show_toolbars = on;
                self.settings.save();
                self.show_toolbar(on);
            }
            Message::SetDefaultZoom(zoom) => {
                self.settings.default_zoom = zoom;
                self.settings.save();
            }
            Message::SetScrollSpeed(speed) => self.settings.scroll_speed = speed,
            Message::SetScanSubsections(on) => {
                self.settings.scan_subsections = on;
                self.settings.save();
                let Some(doc) = &mut self.doc else { return Task::none() };
                if on {
                    return self.start_scan();
                }
                doc.scan = None;
                doc.collapsed = keep_collapsed(&doc.outline, &doc.collapsed, &doc.file_outline);
                doc.outline = doc.file_outline.clone();
            }
            Message::SaveSettings => self.settings.save(),
            Message::SystemTheme(mode) => self.system_dark = mode != iced::theme::Mode::Light,
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
                let mode = self.settings.default_zoom;
                let task = self.zoom(self.fit_zoom(mode), None);
                if let Some(doc) = &mut self.doc {
                    doc.fit = Some(mode);
                }
                return task;
            }
            Key::Named(Named::ArrowLeft) if modifiers.alt() => return self.back(false),
            Key::Named(Named::ArrowRight) if modifiers.alt() => return self.back(true),
            Key::Character("[") if cfg!(target_os = "macos") && modifiers.command() => return self.back(false),
            Key::Character("]") if cfg!(target_os = "macos") && modifiers.command() => return self.back(true),
            Key::Character("z" | "Z") if modifiers.command() && modifiers.shift() => {
                return self.update(Message::Redo);
            }
            Key::Character("z" | "Z") if modifiers.command() => return self.update(Message::Undo),
            Key::Named(Named::Home) => return self.go_to(0),
            Key::Named(Named::End) => {
                let last = self.doc.as_ref().map_or(0, |doc| doc.pages.len() - 1);
                return self.go_to(last);
            }
            Key::Named(Named::PageDown | Named::Space) => return self.scroll_by(Vector::new(0.0, page_height)),
            Key::Named(Named::PageUp) => return self.scroll_by(Vector::new(0.0, -page_height)),
            Key::Named(Named::ArrowDown) => return self.scroll_by(Vector::new(0.0, 40.0)),
            Key::Named(Named::ArrowUp) => return self.scroll_by(Vector::new(0.0, -40.0)),
            _ => {}
        }
        Task::none()
    }

    fn open(&self, path: PathBuf, password: Option<String>) -> Task<Message> {
        let Ok(worker) = &self.pdf else { return Task::none() };
        let tried_password = password.is_some();
        Task::perform(worker.clone().open(path.clone(), password), move |result| {
            Message::Loaded(path.clone(), tried_password, result)
        })
    }

    fn show_toolbar(&mut self, show: bool) {
        let show = show || self.doc.is_none() || self.menu.is_some() || self.settings.always_show_toolbars;
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
        self.remember_position();
        self.next_id += 1;
        self.error = None;
        self.drawing = false;
        // Like pdf.js, start with only the top level of the outline showing.
        let collapsed = (0..info.outline.len()).filter(|&i| has_children(&info.outline, i)).collect();
        let position = settings::recall(&path).filter(|position| position.page < info.pages.len());
        self.doc = Some(Document {
            id: self.next_id,
            path: path.clone(),
            name: path
                .file_name()
                .map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into()),
            pages: info
                .pages
                .into_iter()
                .map(|(w, h)| Page {
                    size: Size::new(w, h),
                    image: None,
                    pending: None,
                    strokes: Vec::new(),
                    links: Vec::new(),
                })
                .collect(),
            file_outline: info.outline.clone(),
            outline: info.outline,
            scan: None,
            collapsed,
            zoom: position.map_or(1.0, |position| position.zoom),
            fit: position.map_or(Some(self.settings.default_zoom), |position| position.fit),
            rotation: position.map_or(0, |position| position.rotation % 360 / 90 * 90),
            current: position.map_or(0, |position| position.page),
            offset: Vector::ZERO,
            page_input: None,
            zoom_input: None,
            history: Vec::new(),
            undone: Vec::new(),
            glide: None,
            back: Vec::new(),
            forward: Vec::new(),
        });
        // Like pdf.js, reopen where the file was left, or else use the default zoom.
        if let Some(mode) = self.doc.as_ref().and_then(|doc| doc.fit) {
            let zoom = self.fit_zoom(mode);
            if let Some(doc) = &mut self.doc {
                doc.zoom = zoom;
            }
        }
        let show = match position {
            Some(position) => self.go_to_spot((position.page, position.fraction)),
            None => self.scroll_to(Vector::ZERO),
        };
        let scan = if self.settings.scan_subsections { self.start_scan() } else { Task::none() };
        Task::batch([show, scan])
    }

    /// Scans the open document for numbered headings, a few pages at a time.
    fn start_scan(&mut self) -> Task<Message> {
        let Some(doc) = &mut self.doc else { return Task::none() };
        if doc.scan.is_some() || doc.outline.iter().any(|entry| entry.scanned) {
            return Task::none();
        }
        doc.scan = Some(Scan::default());
        self.scan_next()
    }

    fn scan_next(&self) -> Task<Message> {
        let (Some(doc), Ok(worker)) = (&self.doc, &self.pdf) else { return Task::none() };
        let Some(scan) = &doc.scan else { return Task::none() };
        let end = (scan.next + SCAN_BATCH).min(doc.pages.len());
        let id = doc.id;
        Task::perform(worker.clone().scan(scan.next..end), move |found| Message::Scanned(id, end, found))
    }

    /// Saves where the open file was left, for next time.
    fn remember_position(&self) {
        let Some(doc) = &self.doc else { return };
        let (page, fraction) = doc.locate(doc.offset.y);
        let position = Position { page, fraction, fit: doc.fit, zoom: doc.zoom, rotation: doc.rotation };
        settings::remember(&doc.path, position);
    }

    /// Area the document scrolls in.
    fn viewport(&self) -> Size {
        let sidebar = if self.sidebar && self.doc.is_some() { SIDEBAR_WIDTH } else { 0.0 };
        Size::new((self.window_size.width - sidebar).max(1.0), self.window_size.height)
    }

    /// The zoom a pdf.js zoom mode gives for the current page and view.
    fn fit_zoom(&self, mode: Zoom) -> f32 {
        let Some(doc) = &self.doc else { return 1.0 };
        let page = doc.pages[doc.current].size;
        let view = self.viewport();
        let (w, h) = if doc.rotation % 180 == 90 { (page.height, page.width) } else { (page.width, page.height) };
        // Leave room for the scrollbar so it never covers the page.
        let width = (view.width - 2.0 * GAP - 2.0 * SCROLLBAR_WIDTH) / (w * PT_TO_PX);
        let whole = width.min((view.height - 2.0 * GAP) / (h * PT_TO_PX));
        let zoom = match mode {
            // Fit the page width, but no bigger than 125%; landscape pages fit whole.
            Zoom::Automatic => (if w > h { whole } else { width }).min(1.25),
            Zoom::PageFit => whole,
            Zoom::PageWidth => width,
            Zoom::ActualSize => 1.0,
        };
        zoom.clamp(MIN_ZOOM, MAX_ZOOM)
    }

    /// Changes zoom while keeping the document point under `anchor` (or, like
    /// pdf.js, the top left of the view) in place.
    fn zoom(&mut self, zoom: f32, anchor: Option<Point>) -> Task<Message> {
        let view = self.viewport();
        let Some(doc) = &mut self.doc else { return Task::none() };
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        doc.zoom_input = None;
        doc.fit = None;
        doc.glide = None;
        if zoom == doc.zoom {
            return Task::none();
        }
        let anchor = anchor.unwrap_or(Point::ORIGIN);
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
        let Some(mode) = doc.fit else { return self.scroll_to(doc.offset) };
        let task = self.zoom(self.fit_zoom(mode), None);
        if let Some(doc) = &mut self.doc {
            doc.fit = Some(mode);
        }
        Task::batch([task, self.scroll_to(self.doc.as_ref().map_or(Vector::ZERO, |doc| doc.offset))])
    }

    fn go_to(&mut self, page: usize) -> Task<Message> {
        let Some(doc) = &mut self.doc else { return Task::none() };
        doc.page_input = None;
        doc.glide = None;
        let offset = Vector::new(doc.offset.x, doc.tops()[page] - GAP);
        let task = self.scroll_to(offset);
        if let Some(doc) = &mut self.doc {
            doc.current = page;
        }
        task
    }

    /// Goes to a page from the outline, a link or the page box (at `top`, if
    /// the link says where on the page), remembering the way back.
    fn jump(&mut self, page: usize, top: Option<f32>) -> Task<Message> {
        let Some(doc) = &mut self.doc else { return Task::none() };
        let page = page.min(doc.pages.len() - 1);
        let here = doc.locate(doc.offset.y);
        doc.back.push(here);
        doc.forward.clear();
        match top {
            Some(top) => self.go_to_spot((page, top)),
            None => self.go_to(page),
        }
    }

    /// Back (or Forward) to where the view was before the last jump.
    fn back(&mut self, forward: bool) -> Task<Message> {
        let Some(doc) = &mut self.doc else { return Task::none() };
        let Some(spot) = (if forward { doc.forward.pop() } else { doc.back.pop() }) else { return Task::none() };
        let here = doc.locate(doc.offset.y);
        if forward { doc.back.push(here) } else { doc.forward.push(here) }
        self.go_to_spot(spot)
    }

    /// Puts `spot` at the top of the view. The page number then follows what
    /// shows, since a spot can sit in the gap after a page.
    fn go_to_spot(&mut self, spot: Spot) -> Task<Message> {
        let Some(doc) = &mut self.doc else { return Task::none() };
        doc.page_input = None;
        doc.glide = None;
        let offset = Vector::new(doc.offset.x, doc.y_of(spot));
        self.scroll_to(offset)
    }

    /// Scrolls by `delta`, gliding there when smooth scrolling is on. Another
    /// notch or key press mid-glide adds to where it was heading.
    fn scroll_by(&mut self, delta: Vector) -> Task<Message> {
        let view = self.viewport();
        let smooth = self.settings.smooth_scrolling;
        let Some(doc) = &mut self.doc else { return Task::none() };
        if !smooth {
            let offset = doc.offset + delta;
            return self.scroll_to(offset);
        }
        let content = doc.content_size(view);
        let to = doc.glide.map_or(doc.offset, |glide| glide.to) + delta;
        let to = Vector::new(
            to.x.clamp(0.0, (content.width - view.width).max(0.0)),
            to.y.clamp(0.0, (content.height - view.height).max(0.0)),
        );
        self.now = Instant::now();
        doc.glide = Some(Glide { from: doc.offset, to, start: self.now });
        Task::none()
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
        // Like pdf.js: stay on the current page while all of it shows, otherwise
        // pick the page with the biggest share showing (the first one on ties).
        let mut most_visible = (0.0, doc.current);
        let mut keep_current = false;
        let mut wanted = HashMap::new();
        let mut tasks = Vec::new();

        for (index, top) in tops.into_iter().enumerate() {
            let size = doc.page_size(index);
            let bottom = top + size.height;
            let shown = (bottom.min(view_end) - top.max(view_start)) / size.height;
            keep_current |= index == doc.current && shown > 0.99;
            if shown > most_visible.0 {
                most_visible = (shown, index);
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
            tasks.push(Task::perform(worker.clone().render(index, width), move |rendered| {
                Message::Rendered { doc: id, page: index, width, rendered }
            }));
        }

        if !keep_current {
            doc.current = most_visible.1;
        }
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

fn has_children(outline: &[OutlineEntry], index: usize) -> bool {
    outline.get(index + 1).is_some_and(|next| next.depth > outline[index].depth)
}

/// Which entries of `new` start folded when the outline changes from `old`:
/// file entries keep how the user left them, and entries that only now have
/// children (scanned subsections under a section) start folded, like pdf.js.
fn keep_collapsed(old: &[OutlineEntry], collapsed: &HashSet<usize>, new: &[OutlineEntry]) -> HashSet<usize> {
    let old_index: Vec<usize> = (0..old.len()).filter(|&i| !old[i].scanned).collect();
    let mut file_entry = 0;
    let mut result = HashSet::new();
    for index in 0..new.len() {
        let folded = if new[index].scanned {
            has_children(new, index)
        } else {
            let before = old_index.get(file_entry).copied();
            file_entry += 1;
            has_children(new, index)
                && before.is_none_or(|before| collapsed.contains(&before) || !has_children(old, before))
        };
        if folded {
            result.insert(index);
        }
    }
    result
}
