//! Preferences and where each file was left, kept as small text files in the
//! user's settings folder. PDFs are never written to.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeChoice {
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "System",
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
        }
    }
}

/// pdf.js's zoom modes. All but `ActualSize` refit when the window changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zoom {
    Automatic,
    PageFit,
    PageWidth,
    ActualSize,
}

impl Zoom {
    pub const ALL: [Zoom; 4] = [Zoom::Automatic, Zoom::PageFit, Zoom::PageWidth, Zoom::ActualSize];

    pub fn label(self) -> &'static str {
        match self {
            Zoom::Automatic => "Automatic",
            Zoom::PageFit => "Page fit",
            Zoom::PageWidth => "Page width",
            Zoom::ActualSize => "100%",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Zoom::Automatic => "automatic",
            Zoom::PageFit => "page-fit",
            Zoom::PageWidth => "page-width",
            Zoom::ActualSize => "actual-size",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Zoom::ALL.into_iter().find(|zoom| zoom.key() == key)
    }
}

pub const SCROLL_SPEEDS: std::ops::RangeInclusive<f32> = 20.0..=200.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub theme: ThemeChoice,
    /// Mouse wheel notches and arrow/page keys glide instead of jumping.
    pub smooth_scrolling: bool,
    /// The toolbars slide in and out instead of appearing at once.
    pub animate_toolbars: bool,
    /// The toolbars never hide.
    pub always_show_toolbars: bool,
    /// How files open when they weren't open before.
    pub default_zoom: Zoom,
    /// How far one mouse wheel notch scrolls, in pixels.
    pub scroll_speed: f32,
}

impl Settings {
    /// The saved settings. Anything not saved yet uses the default; the two
    /// animations start off when the system has animations turned off.
    pub fn load() -> Self {
        let animations = crate::system::animations();
        let mut settings = Self {
            theme: ThemeChoice::System,
            smooth_scrolling: animations,
            animate_toolbars: animations,
            always_show_toolbars: false,
            default_zoom: Zoom::Automatic,
            scroll_speed: 60.0,
        };
        let Some(text) = file("settings.txt").and_then(|path| std::fs::read_to_string(path).ok()) else {
            return settings;
        };
        for (key, value) in text.lines().filter_map(|line| line.split_once('=')) {
            let (key, value) = (key.trim(), value.trim());
            let on = value == "on";
            match key {
                "theme" => {
                    settings.theme = match value {
                        "light" => ThemeChoice::Light,
                        "dark" => ThemeChoice::Dark,
                        _ => ThemeChoice::System,
                    }
                }
                "smooth_scrolling" => settings.smooth_scrolling = on,
                "animate_toolbars" => settings.animate_toolbars = on,
                "always_show_toolbars" => settings.always_show_toolbars = on,
                "default_zoom" => settings.default_zoom = Zoom::from_key(value).unwrap_or(Zoom::Automatic),
                "scroll_speed" => {
                    if let Ok(speed) = value.parse::<f32>() {
                        settings.scroll_speed = speed.clamp(*SCROLL_SPEEDS.start(), *SCROLL_SPEEDS.end());
                    }
                }
                _ => {}
            }
        }
        settings
    }

    /// Best effort: if the file can't be written, the settings still apply
    /// until the app closes.
    pub fn save(&self) {
        let switch = |on: bool| if on { "on" } else { "off" };
        let text = format!(
            "theme={}\nsmooth_scrolling={}\nanimate_toolbars={}\nalways_show_toolbars={}\ndefault_zoom={}\nscroll_speed={}\n",
            self.theme.label().to_lowercase(),
            switch(self.smooth_scrolling),
            switch(self.animate_toolbars),
            switch(self.always_show_toolbars),
            self.default_zoom.key(),
            self.scroll_speed,
        );
        write("settings.txt", &text);
    }
}

/// Where a file was left, like pdf.js's view history.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    pub page: usize,
    /// How far down the page the top of the view was, from 0 to 1.
    pub fraction: f32,
    /// The zoom mode, or `None` for a zoom the user picked.
    pub fit: Option<Zoom>,
    pub zoom: f32,
    pub rotation: u16,
}

/// How many files' positions are kept, newest first.
const REMEMBERED: usize = 50;

/// Where `pdf` was left last time, if it's one of the last 50 files.
pub fn recall(pdf: &Path) -> Option<Position> {
    let key = key(pdf);
    let text = std::fs::read_to_string(file("positions.txt")?).ok()?;
    text.lines().find_map(|line| {
        let mut parts = line.splitn(6, '\t');
        let (page, fraction, fit, zoom, rotation, path) =
            (parts.next()?, parts.next()?, parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        (path == key).then_some(())?;
        Some(Position {
            page: page.parse().ok()?,
            fraction: fraction.parse().ok()?,
            fit: Zoom::from_key(fit),
            zoom: zoom.parse().ok()?,
            rotation: rotation.parse().ok()?,
        })
    })
}

pub fn remember(pdf: &Path, position: Position) {
    let key = key(pdf);
    let old = file("positions.txt").and_then(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
    let fit = position.fit.map_or("custom", Zoom::key);
    let mut text = format!(
        "{}\t{}\t{fit}\t{}\t{}\t{key}\n",
        position.page, position.fraction, position.zoom, position.rotation
    );
    for line in old.lines().filter(|line| line.rsplit('\t').next() != Some(key.as_str())).take(REMEMBERED - 1) {
        text.push_str(line);
        text.push('\n');
    }
    write("positions.txt", &text);
}

/// The same file opened by different paths should match.
fn key(pdf: &Path) -> String {
    std::fs::canonicalize(pdf).unwrap_or_else(|_| pdf.to_path_buf()).to_string_lossy().into_owned()
}

fn write(name: &str, text: &str) {
    let Some(path) = file(name) else { return };
    if let Some(folder) = path.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let _ = std::fs::write(path, text);
}

/// Where each system keeps app settings.
fn file(name: &str) -> Option<PathBuf> {
    let var = |name| std::env::var_os(name).map(PathBuf::from);
    #[cfg(windows)]
    let folder = var("APPDATA");
    #[cfg(target_os = "macos")]
    let folder = var("HOME").map(|home| home.join("Library/Application Support"));
    #[cfg(not(any(windows, target_os = "macos")))]
    let folder = var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|home| home.join(".config")));
    Some(folder?.join("rs-pdf-render").join(name))
}
