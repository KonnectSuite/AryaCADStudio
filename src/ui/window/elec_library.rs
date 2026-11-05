//! The electrical library palette.
//!
//! Lists the symbols a library loaded, each with a thumbnail of its geometry, and inserts
//! one on click. The thumbnail matters: block names like `HT0_01` say nothing, and picking
//! a symbol by sight is the whole point of a library browser.

use iced::widget::canvas::{Frame, Geometry, Path, Program, Stroke};
use iced::widget::{button, canvas, column, container, row, scrollable, text};
use iced::{Element, Fill, Length, Theme};

use crate::app::Message;
use crate::ui::modal::ModalSizing;

/// One symbol as the palette shows it: name, class and the segments to draw.
pub type SymbolRow = (String, String, Vec<((f64, f64), (f64, f64))>);

/// What a click on a symbol runs.
///
/// Reusing the command message keeps the palette free of new message plumbing: the button
/// dispatches the same command a user would type.
fn insert_command(symbol: &str) -> Message {
    Message::Command(format!("ELECPLACE {symbol}"))
}

/// Draws one symbol's segments scaled into the canvas.
struct Thumbnail {
    segments: Vec<((f64, f64), (f64, f64))>,
}

impl Program<Message> for Thumbnail {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        theme: &Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        if self.segments.is_empty() {
            return vec![frame.into_geometry()];
        }

        // Fit the symbol into the canvas with a small margin.
        let (mut min_x, mut min_y) = (f64::MAX, f64::MAX);
        let (mut max_x, mut max_y) = (f64::MIN, f64::MIN);
        for ((ax, ay), (bx, by)) in &self.segments {
            min_x = min_x.min(*ax).min(*bx);
            min_y = min_y.min(*ay).min(*by);
            max_x = max_x.max(*ax).max(*bx);
            max_y = max_y.max(*ay).max(*by);
        }
        let width = (max_x - min_x).max(1e-6);
        let height = (max_y - min_y).max(1e-6);
        let size = bounds.size();
        let margin = 3.0_f32;
        let scale = ((size.width - margin * 2.0) / width as f32)
            .min((size.height - margin * 2.0) / height as f32)
            .max(0.0001);
        // Symbols are drawn with Y up; the canvas has Y down.
        let to_view = |x: f64, y: f64| {
            iced::Point::new(
                margin + ((x - min_x) as f32) * scale,
                size.height - margin - ((y - min_y) as f32) * scale,
            )
        };

        let color = theme.palette().background.base.text;
        for ((ax, ay), (bx, by)) in &self.segments {
            let path = Path::line(to_view(*ax, *ay), to_view(*bx, *by));
            frame.stroke(&path, Stroke::default().with_color(color).with_width(1.0));
        }
        vec![frame.into_geometry()]
    }
}

/// One row: thumbnail, name, class.
fn symbol_row(symbol: &SymbolRow) -> Element<'static, Message> {
    let (name, class, segments) = symbol;
    let thumbnail = canvas(Thumbnail {
        segments: segments.clone(),
    })
        .width(Length::Fixed(64.0))
        .height(Length::Fixed(40.0));

    button(
        row![
            container(thumbnail).width(Length::Fixed(64.0)),
            column![
                text(name.clone()).size(13),
                // The count is here while previews are being finished: it tells a reader
                // whether an empty thumbnail means "no geometry" or "drawing failed".
                text(format!("{} · {} seg", class, segments.len()))
                    .size(11)
                    .style(|theme: &Theme| iced::widget::text::Style {
                        color: Some(theme.palette().background.base.text.scale_alpha(0.6)),
                    }),
            ]
            .spacing(1),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
    )
    .width(Fill)
    .padding([4, 8])
    .on_press(insert_command(name))
    .into()
}

/// The palette body.
pub fn view(symbols: &[SymbolRow], sizing: ModalSizing) -> Element<'static, Message> {
    if symbols.is_empty() {
        return container(
            column![
                text("No symbols loaded.").size(13),
                text("Load a library with  ELECLIB <folder>  (the harvester writes one into AyraCADElectrical\\library).")
                    .size(11)
                    .style(|theme: &Theme| iced::widget::text::Style {
                        color: Some(theme.palette().background.base.text.scale_alpha(0.6)),
                    }),
            ]
            .spacing(6),
        )
        .padding(12)
        .width(sizing.width)
        .height(Length::Fill)
        .into();
    }

    let mut list = column![].spacing(2).width(Fill);
    let mut current_class = String::new();
    for symbol in symbols {
        if symbol.1 != current_class {
            list = list.push(
                text(symbol.1.clone())
                    .size(11)
                    .style(|theme: &Theme| iced::widget::text::Style {
                        color: Some(theme.palette().primary.base.color),
                    }),
            );
            current_class = symbol.1.clone();
        }
        list = list.push(symbol_row(symbol));
    }

    container(scrollable(list).height(Length::Fill))
        .padding(12)
        .width(sizing.width)
        .height(Length::Fill)
        .into()
}


