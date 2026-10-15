use crate::ink::{Ink, Links, Wheel};
use crate::settings::{SCROLL_SPEEDS, ThemeChoice, Zoom};
use crate::{
    App, DOCUMENT, Document, GAP, Locked, Menu, Message, PASSWORD, PT_TO_PX, SCROLLBAR_WIDTH, SIDEBAR_WIDTH,
    TOOLBAR_HEIGHT,
};

use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::{
    button, canvas, column, container, image, keyed_column, mouse_area, opaque, row, scrollable, space,
    slider, stack, svg, text, text_input, toggler, tooltip,
};
use iced::{Background, Border, Center, Color, Element, Fill, Radians, Rotation, Shadow, Theme, Vector};

/// How a keyboard shortcut is written on this platform.
#[cfg(target_os = "macos")]
macro_rules! shortcut {
    ($key:literal) => {
        concat!("⌘", $key)
    };
}
#[cfg(not(target_os = "macos"))]
macro_rules! shortcut {
    ($key:literal) => {
        concat!("Ctrl+", $key)
    };
}

pub const PEN_COLORS: [Color; 4] = [
    Color::from_rgb8(0x1c, 0x1b, 0x22),
    Color::from_rgb8(0xe0, 0x1b, 0x24),
    Color::from_rgb8(0x1c, 0x71, 0xd8),
    Color::from_rgb8(0x26, 0xa2, 0x69),
];

/// Colors for one theme, copied from pdf.js's light and dark themes.
struct Palette {
    backdrop: Color,
    panel: Color,
    field: Color,
    line: Color,
    text: Color,
    muted: Color,
    error: Color,
    /// Hover and pressed tints, dividers and scrollbars: white on dark, black
    /// on light.
    tint: Color,
}

const DARK: Palette = Palette {
    backdrop: Color::from_rgb8(0x2a, 0x2a, 0x2e),
    panel: Color::from_rgb8(0x38, 0x38, 0x3d),
    field: Color::from_rgb8(0x47, 0x47, 0x51),
    line: Color::from_rgb8(0x0c, 0x0c, 0x0d),
    text: Color::from_rgb8(0xf9, 0xf9, 0xfa),
    muted: Color::from_rgb8(0xb1, 0xb1, 0xb3),
    error: Color::from_rgb8(0xff, 0x84, 0x8a),
    tint: Color::WHITE,
};

const LIGHT: Palette = Palette {
    backdrop: Color::from_rgb8(0xd4, 0xd4, 0xd7),
    panel: Color::from_rgb8(0xf9, 0xf9, 0xfa),
    field: Color::WHITE,
    line: Color::from_rgb8(0xb8, 0xb8, 0xb8),
    text: Color::from_rgb8(0x15, 0x14, 0x1a),
    muted: Color::from_rgb8(0x5b, 0x5b, 0x66),
    error: Color::from_rgb8(0xc5, 0x00, 0x42),
    tint: Color::BLACK,
};

const ACCENT: Color = Color::from_rgb8(0x0a, 0x84, 0xff);

/// Style functions get iced's theme, which `App::theme` sets to light or dark.
fn colors(theme: &Theme) -> &'static Palette {
    if *theme == Theme::Light { &LIGHT } else { &DARK }
}

fn muted(theme: &Theme) -> text::Style {
    text::Style { color: Some(colors(theme).muted) }
}

const PAGE_FIELD_WIDTH: f32 = 44.0;
const ZOOM_FIELD_WIDTH: f32 = 62.0;

pub fn view(app: &App) -> Element<'_, Message> {
    let reveal = app.reveal();

    let body: Element<_> = match &app.doc {
        Some(doc) if app.sidebar => row![outline(doc, reveal), document(app, doc)].into(),
        Some(doc) => document(app, doc),
        None => welcome(app),
    };

    let mut layers = stack![container(body).width(Fill).height(Fill).style(backdrop)];
    // Under the toolbar, so another file can still be opened or the window closed.
    if let Some(locked) = &app.locked {
        layers = layers.push(password(locked));
    }
    if let Some(menu) = app.menu {
        // Clicking anywhere else closes the menu, like a browser menu.
        layers = layers.push(mouse_area(space().width(Fill).height(Fill)).on_press(Message::ToggleMenu(menu)));
        let dropdown = match menu {
            Menu::File => file_menu(app),
            Menu::Settings => settings(app),
        };
        layers = layers.push(
            container(opaque(dropdown))
                .width(Fill)
                .align_x(iced::Right)
                .padding(iced::Padding::new(6.0).top(reveal + 6.0)),
        );
    }
    if reveal > 0.5 {
        layers = layers.push(
            container(toolbar(app))
                .height(reveal)
                .align_y(iced::Bottom)
                .clip(true),
        );
        if let Some(doc) = &app.doc {
            let bar = container(name_bar(app, doc)).height(reveal).clip(true);
            layers = layers.push(container(bar).height(Fill).align_y(iced::Bottom));
        }
    }
    if let (Some(error), Some(_)) = (&app.error, &app.doc) {
        layers = layers.push(
            container(
                mouse_area(container(text(error).size(13)).padding([8, 14]).style(toast))
                    .on_press(Message::DismissError),
            )
            .width(Fill)
            .height(Fill)
            .align_x(Center)
            .align_y(iced::Bottom)
            .padding(iced::Padding::new(16.0).bottom(16.0 + reveal)),
        );
    }
    layers.into()
}

/// Which parts of the toolbar fit. Narrow windows (tiling window managers,
/// split screen) give up parts in the order of `Fit::new`, so nothing gets cut
/// off and the window buttons always stay reachable.
struct Fit {
    /// The zoom controls sit in the middle of the window, not just between
    /// the other two groups.
    centered: bool,
    /// The colors and undo, shown while drawing.
    pen_options: bool,
    pen: bool,
    zoom: bool,
    page_count: bool,
    rotate: bool,
}

impl Fit {
    const ALL: Fit = Fit { centered: true, pen_options: true, pen: true, zoom: true, page_count: true, rotate: true };

    /// Drops parts until rough widths add up to less than the window.
    fn new(app: &App, doc: &Document) -> Self {
        const BUTTON: f32 = 32.0;
        let room = app.window_size.width - 12.0;
        let page_count = 30.0 + 7.0 * doc.pages.len().to_string().len() as f32;
        let needs = |fit: &Fit| {
            let width = |shown: bool, width: f32| if shown { width } else { 0.0 };
            let mut left = BUTTON + 14.0 + PAGE_FIELD_WIDTH + width(fit.page_count, page_count);
            let mut right = width(fit.rotate, BUTTON)
                + width(fit.pen, BUTTON)
                + width(fit.pen_options && app.drawing, 5.0 * BUTTON);
            // The window buttons, the menu and settings always show.
            if cfg!(target_os = "macos") {
                left += 3.0 * BUTTON + 8.0;
                right += 2.0 * BUTTON;
            } else {
                right += 5.0 * BUTTON + 8.0;
            }
            let middle = width(fit.zoom, 2.0 * BUTTON + ZOOM_FIELD_WIDTH + 4.0);
            if fit.centered { 2.0 * left.max(right) + middle } else { left + middle + right }
        };

        // Cloud wants drawing to go first once things would get cut off. While
        // drawing, the pen button stays to the end so it can be turned off.
        let mut fit = Fit::ALL;
        for step in 0.. {
            if needs(&fit) <= room {
                break;
            }
            match step {
                0 => fit.centered = false,
                1 => fit.pen_options = false,
                2 if !app.drawing => fit.pen = false,
                3 => fit.zoom = false,
                4 => fit.page_count = false,
                5 => fit.rotate = false,
                6 => fit.pen = false,
                7.. => break,
                _ => {}
            }
        }
        fit
    }
}

fn toolbar(app: &App) -> Element<'_, Message> {
    let close = tool(icon::CLOSE, Some(Message::Close), false, "Close");
    let minimize = tool(icon::MINIMIZE, Some(Message::Minimize), false, "Minimize");
    let maximize = tool(icon::MAXIMIZE, Some(Message::ToggleMaximize), false, "Maximize");
    let menu = tool(icon::MENU, Some(Message::ToggleMenu(Menu::File)), app.menu == Some(Menu::File), "Menu");
    let settings = tool(icon::SETTINGS, Some(Message::ToggleMenu(Menu::Settings)), app.menu == Some(Menu::Settings), "Settings");

    let fit = app.doc.as_ref().map_or(Fit::ALL, |doc| Fit::new(app, doc));
    let (start, middle, end) = match &app.doc {
        Some(doc) => document_tools(app, doc, &fit),
        None => (row![], row![], row![]),
    };
    // Window buttons go where each platform normally puts them.
    let (start, end) = if cfg!(target_os = "macos") {
        (row![close, minimize, maximize, divider(), start], row![end, menu, settings])
    } else {
        (start, row![end, menu, settings, divider(), minimize, maximize, close])
    };

    let (start, end) = (start.spacing(2).align_y(Center), end.spacing(2).align_y(Center));
    let middle = middle.spacing(2).align_y(Center);
    let bar = if fit.centered {
        row![
            container(start).width(Fill),
            middle,
            container(end).width(Fill).align_x(iced::Right),
        ]
    } else {
        row![start, space().width(Fill), middle, space().width(Fill), end]
    }
    .align_y(Center)
    .padding([0, 6])
    .height(TOOLBAR_HEIGHT);

    mouse_area(container(bar).style(|theme| panel(theme, true)))
        .on_press(Message::DragWindow)
        .on_double_click(Message::ToggleMaximize)
        .into()
}

/// The bar at the bottom with the file's name. It comes and goes with the
/// toolbar.
fn name_bar<'a>(app: &App, doc: &'a Document) -> Element<'a, Message> {
    // iced can't shorten text that doesn't fit, so long names are cut using a
    // rough character width.
    let fits = ((app.window_size.width - 24.0) / 7.0).max(4.0) as usize;
    let name = if doc.name.chars().count() > fits {
        format!("{}…", doc.name.chars().take(fits - 1).collect::<String>())
    } else {
        doc.name.clone()
    };
    let bar = container(text(name).size(13).wrapping(text::Wrapping::None))
        .center_x(Fill)
        .height(TOOLBAR_HEIGHT)
        .align_y(Center)
        .style(|theme| panel(theme, true));
    mouse_area(bar)
        .on_press(Message::DragWindow)
        .on_double_click(Message::ToggleMaximize)
        .into()
}

type Row<'a> = iced::widget::Row<'a, Message>;

fn document_tools<'a>(app: &'a App, doc: &'a Document, fit: &Fit) -> (Row<'a>, Row<'a>, Row<'a>) {
    let page = field(
        doc.page_input.clone().unwrap_or_else(|| (doc.current + 1).to_string()),
        PAGE_FIELD_WIDTH,
        Message::PageInput,
        Message::PageSubmit,
    );
    let mut start = row![
        tool(icon::SIDEBAR, Some(Message::ToggleSidebar), app.sidebar, "Document outline"),
        divider(),
        page,
    ]
    .spacing(6)
    .align_y(Center);
    if fit.page_count {
        let count = text!("of {}", doc.pages.len()).size(13).style(muted).wrapping(text::Wrapping::None);
        // A little more room after the page box than between the other parts.
        start = start.push(container(count).padding(iced::Padding::ZERO.left(6)));
    }

    let mut middle = row![];
    if fit.zoom {
        let zoom = field(
            doc.zoom_input.clone().unwrap_or_else(|| format!("{}%", (doc.zoom * 100.0).round())),
            ZOOM_FIELD_WIDTH,
            Message::ZoomInput,
            Message::ZoomSubmit,
        );
        middle = row![
            tool(icon::ZOOM_OUT, Some(Message::ZoomOut), false, concat!("Zoom out (", shortcut!("-"), ")")),
            zoom,
            tool(icon::ZOOM_IN, Some(Message::ZoomIn), false, concat!("Zoom in (", shortcut!("+"), ")")),
        ];
    }

    let mut end = row![];
    if fit.rotate {
        end = end.push(tool(icon::ROTATE, Some(Message::RotateCounterclockwise), false, "Rotate counterclockwise"));
    }
    if app.drawing && fit.pen_options {
        for color in PEN_COLORS {
            end = end.push(swatch(color, app.pen == color));
        }
        end = end.push(tool(
            icon::UNDO,
            (!doc.history.is_empty()).then_some(Message::Undo),
            false,
            concat!("Undo (", shortcut!("Z"), ")"),
        ));
    }
    if fit.pen {
        end = end.push(tool(icon::DRAW, Some(Message::ToggleDrawing), app.drawing, "Draw"));
    }

    (start, middle, end)
}

fn document<'a>(app: &'a App, doc: &'a Document) -> Element<'a, Message> {
    let view = app.viewport();
    let content = doc.content_size(view);

    // Only pages within a screen of the view get widgets; one spacer stands in
    // for all pages above them and one for all below. Building a widget for
    // every page on each mouse move cost real CPU time on long documents.
    // Pages are keyed by number so a stroke being drawn stays on its page.
    let tops = doc.tops();
    let (start, end) = (doc.offset.y - view.height, doc.offset.y + 2.0 * view.height);
    let bottom = |index: usize| tops[index] + doc.page_size(index).height;
    let first = (0..tops.len()).find(|&index| bottom(index) >= start).unwrap_or(0);
    let last = (first..tops.len()).take_while(|&index| tops[index] <= end).last().unwrap_or(first);

    let mut pages = keyed_column([]).spacing(GAP).padding(GAP).align_items(Center).width(content.width);
    if first > 0 {
        pages = pages.push(usize::MAX - 1, Element::from(space().height(tops[first] - 2.0 * GAP)));
    }
    for index in first..=last {
        pages = pages.push(index, page(app, doc, index));
    }
    if last + 1 < tops.len() {
        pages = pages.push(usize::MAX, Element::from(space().height(content.height - bottom(last) - 2.0 * GAP)));
    }

    let scroller = scrollable(pages)
        .id(DOCUMENT)
        .direction(Direction::Both { vertical: thin_scrollbar(), horizontal: thin_scrollbar() })
        .style(scrollbars)
        .on_scroll(Message::Scrolled)
        .width(Fill)
        .height(Fill);

    let wheel = Wheel {
        zoom: app.modifiers.command() || app.modifiers.control(),
        shift: app.modifiers.shift(),
        notch: app.settings.scroll_speed,
    };
    stack![scroller, canvas(wheel).width(Fill).height(Fill)].into()
}

fn page<'a>(app: &'a App, doc: &'a Document, index: usize) -> Element<'a, Message> {
    let page = &doc.pages[index];
    let size = doc.page_size(index);
    let mut layers = stack![].width(size.width).height(size.height);

    if let Some((_, handle)) = &page.image {
        let turn = Rotation::Solid(Radians(f32::from(doc.rotation).to_radians()));
        layers = layers.push(image(handle.clone()).width(Fill).height(Fill).rotation(turn));
    }
    if app.drawing || !page.strokes.is_empty() {
        let ink = Ink {
            page: index,
            strokes: &page.strokes,
            rotation: doc.rotation,
            scale: doc.zoom * PT_TO_PX,
            pen: app.drawing.then_some(app.pen),
        };
        layers = layers.push(canvas(ink).width(Fill).height(Fill));
    }
    if !app.drawing && !page.links.is_empty() {
        let links = Links { links: &page.links, rotation: doc.rotation };
        layers = layers.push(canvas(links).width(Fill).height(Fill));
    }

    container(layers)
        .style(|_| container::Style {
            background: Some(Color::WHITE.into()),
            shadow: Shadow { color: Color::from_rgba(0.0, 0.0, 0.0, 0.5), offset: Vector::new(0.0, 1.0), blur_radius: 4.0 },
            ..container::Style::default()
        })
        .into()
}

fn outline(doc: &Document, top: f32) -> Element<'_, Message> {
    let mut list = column![].spacing(1);
    let mut hidden_below = None;

    for (index, entry) in doc.outline.iter().enumerate() {
        if hidden_below.is_some_and(|depth| entry.depth > depth) {
            continue;
        }
        hidden_below = None;

        let has_children = doc.outline.get(index + 1).is_some_and(|next| next.depth > entry.depth);
        let collapsed = doc.collapsed.contains(&index);
        if has_children && collapsed {
            hidden_below = Some(entry.depth);
        }

        let toggle: Element<_> = if has_children {
            button(glyph(if collapsed { icon::EXPAND } else { icon::COLLAPSE }, 14.0))
                .on_press(Message::ToggleOutlineEntry(index))
                .padding(3)
                .style(|theme, status| flat(theme, status, false))
                .into()
        } else {
            space().width(20).into()
        };
        let title = button(text(&entry.title).size(13).wrapping(text::Wrapping::WordOrGlyph))
            .on_press_maybe(entry.page.map(|page| Message::Jump(page, entry.top)))
            .width(Fill)
            .padding([4, 6])
            .style(|theme, status| flat(theme, status, false));

        list = list.push(
            row![toggle, title]
                .align_y(Center)
                .padding(iced::Padding::ZERO.left(entry.depth as f32 * 14.0)),
        );
    }

    let scanning = doc.scan.as_ref().map(|scan| {
        text!("Scanning for numbered subsections… {} of {}", scan.next, doc.pages.len()).size(12).style(muted)
    });
    let body: Element<_> = if doc.outline.is_empty() {
        let empty = scanning.unwrap_or_else(|| text("No outline in this document.").size(13).style(muted));
        container(empty).padding(12).into()
    } else {
        let list = scrollable(list.padding(6)).direction(Direction::Vertical(thin_scrollbar())).style(scrollbars);
        let mut body = column![];
        if let Some(scanning) = scanning {
            body = body.push(container(scanning).padding([6, 12]));
        }
        body.push(list.height(Fill)).into()
    };

    container(body)
        .padding(iced::Padding::ZERO.top(top).bottom(top))
        .width(SIDEBAR_WIDTH)
        .height(Fill)
        .style(|theme| panel(theme, false))
        .into()
}

fn welcome(app: &App) -> Element<'_, Message> {
    let message = match (&app.pdf, &app.error) {
        (Err(error), _) | (_, Some(error)) => {
            text(error).size(14).style(|theme| text::Style { color: Some(colors(theme).error) })
        }
        _ => text(concat!("Drop a PDF here or press ", shortcut!("O"))).size(14).style(muted),
    };
    let open = button(text("Open a PDF").size(14))
        .on_press_maybe(app.pdf.is_ok().then_some(Message::PickFile))
        .padding([8, 16])
        .style(|_, status| push(status, ACCENT, Color::WHITE));

    container(column![open, message].spacing(14).align_x(Center))
        .center(Fill)
        .into()
}

/// pdf.js's password dialog, over a dimmed window.
fn password(locked: &Locked) -> Element<'_, Message> {
    let message = if locked.wrong {
        "Invalid password. Please try again."
    } else {
        "Enter the password to open this PDF file."
    };
    let input = text_input("", &locked.input)
        .id(PASSWORD)
        .secure(true)
        .on_input(Message::PasswordInput)
        .on_submit(Message::Unlock)
        .size(13)
        .padding([6, 8])
        .style(field_style);
    let buttons = row![
        space().width(Fill),
        button(text("Cancel").size(13))
            .on_press(Message::CancelPassword)
            .padding([6, 14])
            .style(|theme, status| push(status, colors(theme).field, colors(theme).text)),
        button(text("OK").size(13)).on_press(Message::Unlock).padding([6, 14]).style(|_, status| push(status, ACCENT, Color::WHITE)),
    ]
    .spacing(8);

    let dialog = container(column![text(message).size(13), input, buttons].spacing(12))
        .width(340)
        .padding(16)
        .style(toast);
    opaque(container(dialog).center(Fill).padding(16).style(|_| container::Style {
        background: Some(Color::BLACK.scale_alpha(0.4).into()),
        ..container::Style::default()
    }))
}

/// The settings menu under the gear button.
fn settings(app: &App) -> Element<'_, Message> {
    let themes = row(ThemeChoice::ALL.map(|choice| {
        let selected = app.settings.theme == choice;
        button(text(choice.label()).size(13))
            .on_press(Message::SetTheme(choice))
            .padding([4, 10])
            .style(move |theme, status| flat(theme, status, selected))
            .into()
    }))
    .spacing(2);
    let zooms = Zoom::ALL.map(|zoom| {
        let selected = app.settings.default_zoom == zoom;
        button(text(zoom.label()).size(13))
            .on_press(Message::SetDefaultZoom(zoom))
            .padding([4, 10])
            .width(Fill)
            .style(move |theme, status| flat(theme, status, selected))
            .into()
    });
    let [a, b, c, d]: [Element<_>; 4] = zooms;
    let zooms = column![row![a, b].spacing(2), row![c, d].spacing(2)].spacing(2);
    let speed = row![
        slider(SCROLL_SPEEDS, app.settings.scroll_speed, Message::SetScrollSpeed)
            .step(10.0_f32)
            .on_release(Message::SaveSettings)
            .style(slider_style),
        text!("{} px", app.settings.scroll_speed).size(12).style(muted).width(44).align_x(iced::Right),
    ]
    .spacing(8)
    .align_y(Center);
    let switch = |on: bool, label: &'static str, message: fn(bool) -> Message| {
        toggler(on).label(label).on_toggle(message).size(16).text_size(13).spacing(8).width(Fill).style(switch_style)
    };

    let menu = column![
        text("Theme").size(12).style(muted),
        themes,
        switch(app.settings.smooth_scrolling, "Smooth scrolling", Message::SetSmoothScrolling),
        switch(app.settings.animate_toolbars, "Animate toolbars", Message::SetAnimateToolbars),
        switch(app.settings.always_show_toolbars, "Always show toolbars", Message::SetAlwaysShowToolbars),
        switch(app.settings.scan_subsections, "Scan for numbered subsections", Message::SetScanSubsections),
        text("Zoom for new files").size(12).style(muted),
        zooms,
        text("Scroll speed per wheel notch").size(12).style(muted),
        speed,
    ]
    .spacing(10);
    container(menu).width(260).padding(12).style(toast).into()
}

/// The dropdown under the menu button.
fn file_menu(app: &App) -> Element<'_, Message> {
    let item = |icon: &'static str, label: &'static str, keys: &'static str, message: Option<Message>| {
        let content = row![glyph(icon, 16.0), text(label).size(13), space().width(Fill), text(keys).size(12).style(muted)]
            .spacing(10)
            .align_y(Center);
        button(content)
            .on_press_maybe(message)
            .width(Fill)
            .padding([6, 8])
            .style(|theme, status| flat(theme, status, false))
    };
    let menu = column![
        item(icon::OPEN, "Open file…", shortcut!("O"), app.pdf.is_ok().then_some(Message::PickFile)),
        item(icon::PRINT, "Print…", shortcut!("P"), app.doc.is_some().then_some(Message::Print)),
    ]
    .spacing(2);
    container(menu).width(220).padding(6).style(toast).into()
}

fn tool<'a>(icon: &'static str, on_press: Option<Message>, active: bool, label: &'a str) -> Element<'a, Message> {
    let button = button(glyph(icon, 18.0))
        .on_press_maybe(on_press)
        .padding(6)
        .style(move |theme, status| flat(theme, status, active));
    tooltip(button, container(text(label).size(12)).padding([4, 8]).style(toast), tooltip::Position::Bottom)
        .into()
}

fn swatch<'a>(color: Color, selected: bool) -> Element<'a, Message> {
    let dot = container(space()).width(14).height(14).style(move |theme| container::Style {
        background: Some(color.into()),
        border: Border {
            color: if selected { colors(theme).text } else { colors(theme).muted.scale_alpha(0.5) },
            width: if selected { 2.0 } else { 1.0 },
            radius: 7.0.into(),
        },
        ..container::Style::default()
    });
    button(dot)
        .on_press(Message::PenColor(color))
        .padding(8)
        .style(|theme, status| flat(theme, status, false))
        .into()
}

fn field<'a>(
    value: String,
    width: f32,
    on_input: fn(String) -> Message,
    on_submit: Message,
) -> Element<'a, Message> {
    text_input("", &value)
        .on_input(on_input)
        .on_submit(on_submit)
        .width(width)
        .size(13)
        .padding([4, 6])
        .align_x(Center)
        .style(field_style)
        .into()
}

fn field_style(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let colors = colors(theme);
    text_input::Style {
        background: colors.field.into(),
        border: Border {
            color: if matches!(status, text_input::Status::Focused { .. }) { ACCENT } else { colors.line },
            width: 1.0,
            radius: 4.0.into(),
        },
        icon: colors.text,
        placeholder: colors.muted,
        value: colors.text,
        selection: ACCENT.scale_alpha(0.5),
    }
}

/// iced's slider, in the app's blue.
fn slider_style(theme: &Theme, status: slider::Status) -> slider::Style {
    let mut style = slider::default(theme, status);
    style.rail.backgrounds.0 = ACCENT.into();
    style.handle.background = ACCENT.into();
    style
}

/// iced's switch, in the app's blue instead of iced's purple.
fn switch_style(theme: &Theme, status: toggler::Status) -> toggler::Style {
    let mut style = toggler::default(theme, status);
    if let toggler::Status::Active { is_toggled: true } | toggler::Status::Hovered { is_toggled: true } = status {
        style.background = ACCENT.into();
    }
    style
}

/// A filled button, like the welcome screen's "Open a PDF".
fn push(status: button::Status, color: Color, text: Color) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(if hovered { color.scale_alpha(0.85) } else { color }.into()),
        text_color: text,
        border: Border::default().rounded(6),
        ..button::Style::default()
    }
}

fn glyph<'a>(icon: &'static str, size: f32) -> Element<'a, Message> {
    svg(svg::Handle::from_memory(icon.as_bytes()))
        .width(size)
        .height(size)
        .style(|theme, _| svg::Style { color: Some(colors(theme).text) })
        .into()
}

fn divider<'a>() -> Element<'a, Message> {
    container(space())
        .width(1)
        .height(20)
        .style(|theme| container::Style {
            background: Some(colors(theme).tint.scale_alpha(0.12).into()),
            ..container::Style::default()
        })
        .into()
}

fn thin_scrollbar() -> Scrollbar {
    Scrollbar::new().width(SCROLLBAR_WIDTH).scroller_width(SCROLLBAR_WIDTH).margin(2)
}

fn scrollbars(theme: &iced::Theme, status: scrollable::Status) -> scrollable::Style {
    let mut style = scrollable::default(theme, status);
    for rail in [&mut style.vertical_rail, &mut style.horizontal_rail] {
        rail.background = None;
        rail.border = Border::default();
        rail.scroller.border = Border::default().rounded(SCROLLBAR_WIDTH / 2.0);
        rail.scroller.background = colors(theme).tint.scale_alpha(0.3).into();
    }
    style
}

fn flat(theme: &Theme, status: button::Status, active: bool) -> button::Style {
    let colors = colors(theme);
    let background = match status {
        button::Status::Pressed => Some(0.2),
        button::Status::Hovered => Some(0.1),
        _ if active => Some(0.16),
        _ => None,
    };
    button::Style {
        background: background.map(|alpha| Background::Color(colors.tint.scale_alpha(alpha))),
        text_color: if status == button::Status::Disabled { colors.muted.scale_alpha(0.5) } else { colors.text },
        border: Border::default().rounded(4),
        ..button::Style::default()
    }
}

fn backdrop(theme: &Theme) -> container::Style {
    let colors = colors(theme);
    container::Style { background: Some(colors.backdrop.into()), text_color: Some(colors.text), ..container::Style::default() }
}

fn panel(theme: &Theme, toolbar: bool) -> container::Style {
    let colors = colors(theme);
    container::Style {
        background: Some(colors.panel.into()),
        text_color: Some(colors.text),
        shadow: if toolbar {
            Shadow { color: Color::from_rgba(0.0, 0.0, 0.0, 0.4), offset: Vector::new(0.0, 1.0), blur_radius: 6.0 }
        } else {
            Shadow::default()
        },
        border: Border { color: colors.line, width: if toolbar { 0.0 } else { 1.0 }, radius: 0.0.into() },
        ..container::Style::default()
    }
}

fn toast(theme: &Theme) -> container::Style {
    let colors = colors(theme);
    container::Style {
        background: Some(colors.panel.into()),
        text_color: Some(colors.text),
        border: Border { color: colors.line, width: 1.0, radius: 6.0.into() },
        shadow: Shadow { color: Color::from_rgba(0.0, 0.0, 0.0, 0.4), offset: Vector::new(0.0, 2.0), blur_radius: 8.0 },
        ..container::Style::default()
    }
}

/// Lucide icons (ISC license), drawn in white and recolored by the svg style.
mod icon {
    macro_rules! icon {
        ($name:ident, $body:literal) => {
            pub const $name: &str = concat!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#,
                $body,
                "</svg>"
            );
        };
    }

    icon!(SIDEBAR, r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M9 3v18"/>"#);
    icon!(ZOOM_OUT, r#"<path d="M5 12h14"/>"#);
    icon!(ZOOM_IN, r#"<path d="M5 12h14"/><path d="M12 5v14"/>"#);
    icon!(ROTATE, r#"<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/>"#);
    icon!(DRAW, r#"<path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"/><path d="m15 5 4 4"/>"#);
    icon!(UNDO, r#"<path d="M9 14 4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5a5.5 5.5 0 0 1-5.5 5.5H11"/>"#);
    icon!(PRINT, r#"<path d="M6 18H4a2 2 0 0 1-2-2v-5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2h-2"/><path d="M6 9V3a1 1 0 0 1 1-1h10a1 1 0 0 1 1 1v6"/><rect x="6" y="14" width="12" height="8" rx="1"/>"#);
    icon!(OPEN, r#"<path d="m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2"/>"#);
    icon!(EXPAND, r#"<path d="m9 18 6-6-6-6"/>"#);
    icon!(COLLAPSE, r#"<path d="m6 9 6 6 6-6"/>"#);
    icon!(MINIMIZE, r#"<path d="M5 12h14"/>"#);
    icon!(MAXIMIZE, r#"<rect width="16" height="16" x="4" y="4" rx="2"/>"#);
    icon!(CLOSE, r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#);
    icon!(MENU, r#"<path d="M4 6h16"/><path d="M4 12h16"/><path d="M4 18h16"/>"#);
    icon!(SETTINGS, r#"<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>"#);
}
