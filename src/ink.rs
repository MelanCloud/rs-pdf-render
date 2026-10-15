//! Freehand drawing on top of a page.

use crate::Message;
use iced::mouse::{self, Cursor};
use iced::widget::canvas::{self, Action, Event, Frame, Geometry, Path, Program};
use iced::{Color, Point, Rectangle, Renderer, Size, Theme};

#[derive(Debug, Clone)]
pub struct Stroke {
    /// Points relative to the unrotated page: (0, 0) is top-left, (1, 1) bottom-right.
    pub points: Vec<(f32, f32)>,
    pub color: Color,
    /// Line width in PDF points.
    pub width: f32,
}

pub const PEN_WIDTH: f32 = 2.0;

/// The ink layer for one page.
pub struct Ink<'a> {
    pub page: usize,
    pub strokes: &'a [Stroke],
    /// Clockwise rotation in degrees.
    pub rotation: u16,
    /// Screen pixels per PDF point.
    pub scale: f32,
    /// The pen color while drawing mode is on.
    pub pen: Option<Color>,
}

impl Ink<'_> {
    fn to_page(&self, p: Point, size: Size) -> (f32, f32) {
        let (x, y) = ((p.x / size.width).clamp(0.0, 1.0), (p.y / size.height).clamp(0.0, 1.0));
        match self.rotation {
            90 => (y, 1.0 - x),
            180 => (1.0 - x, 1.0 - y),
            270 => (1.0 - y, x),
            _ => (x, y),
        }
    }

    fn to_screen(&self, (u, v): (f32, f32), size: Size) -> Point {
        let (x, y) = match self.rotation {
            90 => (1.0 - v, u),
            180 => (1.0 - u, 1.0 - v),
            270 => (v, 1.0 - u),
            _ => (u, v),
        };
        Point::new(x * size.width, y * size.height)
    }

    fn stroke(&self, frame: &mut Frame, stroke: &Stroke) {
        let size = frame.size();
        let path = Path::new(|path| {
            let mut points = stroke.points.iter().map(|p| self.to_screen(*p, size));
            if let Some(first) = points.next() {
                path.move_to(first);
                points.for_each(|p| path.line_to(p));
            }
        });
        frame.stroke(
            &path,
            canvas::Stroke::default()
                .with_color(stroke.color)
                .with_width(stroke.width * self.scale)
                .with_line_cap(canvas::LineCap::Round)
                .with_line_join(canvas::LineJoin::Round),
        );
    }
}

impl Program<Message> for Ink<'_> {
    /// The stroke being drawn right now.
    type State = Option<Stroke>;

    fn update(
        &self,
        current: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<Message>> {
        let color = self.pen?;
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let p = cursor.position_in(bounds)?;
                *current = Some(Stroke {
                    points: vec![self.to_page(p, bounds.size())],
                    color,
                    width: PEN_WIDTH,
                });
                Some(Action::request_redraw().and_capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let stroke = current.as_mut()?;
                let p = *position - bounds.position();
                stroke.points.push(self.to_page(Point::new(p.x, p.y), bounds.size()));
                Some(Action::request_redraw().and_capture())
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let mut stroke = current.take()?;
                if stroke.points.len() == 1 {
                    // A click leaves a dot.
                    stroke.points.push(stroke.points[0]);
                }
                Some(Action::publish(Message::StrokeDone(self.page, stroke)).and_capture())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        current: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        if self.strokes.is_empty() && current.is_none() {
            return Vec::new();
        }
        let mut frame = Frame::new(renderer, bounds.size());
        for stroke in self.strokes.iter().chain(current) {
            self.stroke(&mut frame, stroke);
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, _: &Self::State, bounds: Rectangle, cursor: Cursor) -> mouse::Interaction {
        if self.pen.is_some() && cursor.is_over(bounds) {
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::default()
        }
    }
}

/// Invisible layer over the document that turns Ctrl/Cmd + wheel into zoom
/// before the scrollable can use it to scroll.
pub struct WheelZoom {
    pub active: bool,
}

impl Program<Message> for WheelZoom {
    type State = ();

    fn update(&self, _: &mut (), event: &Event, bounds: Rectangle, cursor: Cursor) -> Option<Action<Message>> {
        let Event::Mouse(mouse::Event::WheelScrolled { delta }) = event else {
            return None;
        };
        if !self.active {
            return None;
        }
        let at = cursor.position_in(bounds)?;
        let y = match delta {
            mouse::ScrollDelta::Lines { y, .. } => *y,
            mouse::ScrollDelta::Pixels { y, .. } => *y / 40.0,
        };
        Some(Action::publish(Message::WheelZoom(y, at)).and_capture())
    }

    fn draw(&self, _: &(), _: &Renderer, _: &Theme, _: Rectangle, _: Cursor) -> Vec<Geometry> {
        Vec::new()
    }
}
