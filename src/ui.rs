use crate::ink::{Ink, WheelZoom};
use crate::{App, DOCUMENT, Document, GAP, Message, PT_TO_PX, SCROLLBAR_WIDTH, SIDEBAR_WIDTH, TOOLBAR_HEIGHT};

use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::{
    button, canvas, column, container, image, mouse_area, row, scrollable, space, stack, svg, text,
    text_input, tooltip,
};
use iced::{Background, Border, Center, Color, Element, Fill, Radians, Rotation, Shadow, Vector};

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

const BACKDROP: Color = Color::from_rgb8(0x2a, 0x2a, 0x2e);
const PANEL: Color = Color::from_rgb8(0x38, 0x38, 0x3d);
const FIELD: Color = Color::from_rgb8(0x47, 0x47, 0x51);
const LINE: Color = Color::from_rgb8(0x0c, 0x0c, 0x0d);
const TEXT: Color = Color::from_rgb8(0xf9, 0xf9, 0xfa);
const MUTED: Color = Color::from_rgb8(0xb1, 0xb1, 0xb3);
const ACCENT: Color = Color::from_rgb8(0x0a, 0x84, 0xff);

pub fn view(app: &App) -> Element<'_, Message> {
    let reveal = app.toolbar.interpolate(0.0, TOOLBAR_HEIGHT, app.now);

    let body: Element<_> = match &app.doc {
        Some(doc) if app.sidebar => row![outline(doc, reveal), document(app, doc)].into(),
        Some(doc) => document(app, doc),
        None => welcome(app),
    };

    let mut layers = stack![container(body).width(Fill).height(Fill).style(|_| backdrop())];
    if reveal > 0.5 {
        layers = layers.push(
            container(toolbar(app))
                .height(reveal)
                .align_y(iced::Bottom)
                .clip(true),
        );
    }
    if let (Some(error), Some(_)) = (&app.error, &app.doc) {
        layers = layers.push(
            container(
                mouse_area(container(text(error).size(13)).padding([8, 14]).style(|_| toast()))
                    .on_press(Message::DismissError),
            )
            .width(Fill)
            .height(Fill)
            .align_x(Center)
            .align_y(iced::Bottom)
            .padding(16),
        );
    }
    layers.into()
}

fn toolbar(app: &App) -> Element<'_, Message> {
    let close = tool(icon::CLOSE, Some(Message::Close), false, "Close");
    let minimize = tool(icon::MINIMIZE, Some(Message::Minimize), false, "Minimize");
    let maximize = tool(icon::MAXIMIZE, Some(Message::ToggleMaximize), false, "Maximize");
    let open = tool(icon::OPEN, Some(Message::PickFile), false, concat!("Open file (", shortcut!("O"), ")"));

    let (start, middle, end) = match &app.doc {
        Some(doc) => document_tools(app, doc),
        None => (row![], row![], row![]),
    };
    // Window buttons go where each platform normally puts them.
    let (start, end) = if cfg!(target_os = "macos") {
        (row![close, minimize, maximize, divider(), start], row![end, open])
    } else {
        (start, row![end, open, divider(), minimize, maximize, close])
    };

    let bar = row![
        container(start.spacing(2).align_y(Center)).width(Fill),
        middle.spacing(2).align_y(Center),
        container(end.spacing(2).align_y(Center)).width(Fill).align_x(iced::Right),
    ]
    .align_y(Center)
    .padding([0, 6])
    .height(TOOLBAR_HEIGHT);

    mouse_area(container(bar).style(|_| panel(true)))
        .on_press(Message::DragWindow)
        .on_double_click(Message::ToggleMaximize)
        .into()
}

type Row<'a> = iced::widget::Row<'a, Message>;

fn document_tools<'a>(app: &'a App, doc: &'a Document) -> (Row<'a>, Row<'a>, Row<'a>) {
    let page = field(
        doc.page_input.clone().unwrap_or_else(|| (doc.current + 1).to_string()),
        44.0,
        Message::PageInput,
        Message::PageSubmit,
    );
    let start = row![
        tool(icon::SIDEBAR, Some(Message::ToggleSidebar), app.sidebar, "Document outline"),
        divider(),
        page,
        text!("of {}", doc.pages.len()).size(13).color(MUTED),
    ]
    .spacing(6)
    .align_y(Center);

    let zoom = field(
        doc.zoom_input.clone().unwrap_or_else(|| format!("{}%", (doc.zoom * 100.0).round())),
        62.0,
        Message::ZoomInput,
        Message::ZoomSubmit,
    );
    let middle = row![
        tool(icon::ZOOM_OUT, Some(Message::ZoomOut), false, concat!("Zoom out (", shortcut!("-"), ")")),
        zoom,
        tool(icon::ZOOM_IN, Some(Message::ZoomIn), false, concat!("Zoom in (", shortcut!("+"), ")")),
    ];

    let mut end = row![tool(
        icon::ROTATE,
        Some(Message::RotateCounterclockwise),
        false,
        "Rotate counterclockwise"
    )];
    if app.drawing {
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
    let end = end
        .push(tool(icon::DRAW, Some(Message::ToggleDrawing), app.drawing, "Draw"))
        .push(tool(icon::PRINT, Some(Message::Print), false, concat!("Print (", shortcut!("P"), ")")));

    (start, middle, end)
}

fn document<'a>(app: &'a App, doc: &'a Document) -> Element<'a, Message> {
    let content = doc.content_size(app.viewport());
    let pages = column(doc.pages.iter().enumerate().map(|(index, _)| page(app, doc, index)))
        .spacing(GAP)
        .padding(GAP)
        .align_x(Center)
        .width(content.width);

    let scroller = scrollable(pages)
        .id(DOCUMENT)
        .direction(Direction::Both { vertical: thin_scrollbar(), horizontal: thin_scrollbar() })
        .style(scrollbars)
        .on_scroll(Message::Scrolled)
        .width(Fill)
        .height(Fill);

    let zoom = app.modifiers.command() || app.modifiers.control();
    stack![scroller, canvas(WheelZoom { active: zoom }).width(Fill).height(Fill)].into()
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
                .style(|_, status| flat(status, false))
                .into()
        } else {
            space().width(20).into()
        };
        let title = button(text(&entry.title).size(13).wrapping(text::Wrapping::WordOrGlyph))
            .on_press_maybe(entry.page.map(Message::GoTo))
            .width(Fill)
            .padding([4, 6])
            .style(|_, status| flat(status, false));

        list = list.push(
            row![toggle, title]
                .align_y(Center)
                .padding(iced::Padding::ZERO.left(entry.depth as f32 * 14.0)),
        );
    }

    let body: Element<_> = if doc.outline.is_empty() {
        container(text("No outline in this document.").size(13).color(MUTED)).padding(12).into()
    } else {
        scrollable(list.padding(6))
            .direction(Direction::Vertical(thin_scrollbar()))
            .style(scrollbars)
            .height(Fill)
            .into()
    };

    container(body)
        .padding(iced::Padding::ZERO.top(top))
        .width(SIDEBAR_WIDTH)
        .height(Fill)
        .style(|_| panel(false))
        .into()
}

fn welcome(app: &App) -> Element<'_, Message> {
    let message = match (&app.pdf, &app.error) {
        (Err(error), _) | (_, Some(error)) => text(error).size(14).color(Color::from_rgb8(0xff, 0x84, 0x8a)),
        _ => text(concat!("Drop a PDF here or press ", shortcut!("O"))).size(14).color(MUTED),
    };
    let open = button(text("Open a PDF").size(14))
        .on_press_maybe(app.pdf.is_ok().then_some(Message::PickFile))
        .padding([8, 16])
        .style(|_, status| {
            let base = button::Style {
                background: Some(ACCENT.into()),
                text_color: Color::WHITE,
                border: Border::default().rounded(6),
                ..button::Style::default()
            };
            match status {
                button::Status::Hovered | button::Status::Pressed => {
                    button::Style { background: Some(ACCENT.scale_alpha(0.85).into()), ..base }
                }
                _ => base,
            }
        });

    container(column![open, message].spacing(14).align_x(Center))
        .center(Fill)
        .into()
}

fn tool<'a>(icon: &'static str, on_press: Option<Message>, active: bool, label: &'a str) -> Element<'a, Message> {
    let button = button(glyph(icon, 18.0))
        .on_press_maybe(on_press)
        .padding(6)
        .style(move |_, status| flat(status, active));
    tooltip(button, container(text(label).size(12)).padding([4, 8]).style(|_| toast()), tooltip::Position::Bottom)
        .into()
}

fn swatch<'a>(color: Color, selected: bool) -> Element<'a, Message> {
    let dot = container(space()).width(14).height(14).style(move |_| container::Style {
        background: Some(color.into()),
        border: Border { color: if selected { TEXT } else { MUTED.scale_alpha(0.5) }, width: if selected { 2.0 } else { 1.0 }, radius: 7.0.into() },
        ..container::Style::default()
    });
    button(dot)
        .on_press(Message::PenColor(color))
        .padding(8)
        .style(|_, status| flat(status, false))
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
        .style(|_, status| text_input::Style {
            background: FIELD.into(),
            border: Border {
                color: if matches!(status, text_input::Status::Focused { .. }) { ACCENT } else { LINE },
                width: 1.0,
                radius: 4.0.into(),
            },
            icon: TEXT,
            placeholder: MUTED,
            value: TEXT,
            selection: ACCENT.scale_alpha(0.5),
        })
        .into()
}

fn glyph<'a>(icon: &'static str, size: f32) -> Element<'a, Message> {
    svg(svg::Handle::from_memory(icon.as_bytes()))
        .width(size)
        .height(size)
        .style(|_, _| svg::Style { color: Some(TEXT) })
        .into()
}

fn divider<'a>() -> Element<'a, Message> {
    container(space())
        .width(1)
        .height(20)
        .style(|_| container::Style { background: Some(Color::WHITE.scale_alpha(0.12).into()), ..container::Style::default() })
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
        rail.scroller.background = Color::WHITE.scale_alpha(0.3).into();
    }
    style
}

fn flat(status: button::Status, active: bool) -> button::Style {
    let background = match status {
        button::Status::Pressed => Some(0.2),
        button::Status::Hovered => Some(0.1),
        _ if active => Some(0.16),
        _ => None,
    };
    button::Style {
        background: background.map(|alpha| Background::Color(Color::WHITE.scale_alpha(alpha))),
        text_color: if status == button::Status::Disabled { MUTED.scale_alpha(0.5) } else { TEXT },
        border: Border::default().rounded(4),
        ..button::Style::default()
    }
}

fn backdrop() -> container::Style {
    container::Style { background: Some(BACKDROP.into()), text_color: Some(TEXT), ..container::Style::default() }
}

fn panel(toolbar: bool) -> container::Style {
    container::Style {
        background: Some(PANEL.into()),
        text_color: Some(TEXT),
        shadow: if toolbar {
            Shadow { color: Color::from_rgba(0.0, 0.0, 0.0, 0.4), offset: Vector::new(0.0, 1.0), blur_radius: 6.0 }
        } else {
            Shadow::default()
        },
        border: Border { color: LINE, width: if toolbar { 0.0 } else { 1.0 }, radius: 0.0.into() },
        ..container::Style::default()
    }
}

fn toast() -> container::Style {
    container::Style {
        background: Some(PANEL.into()),
        text_color: Some(TEXT),
        border: Border { color: LINE, width: 1.0, radius: 6.0.into() },
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
}
