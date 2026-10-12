//! One-line labels that run out of room fade out at the end instead of
//! breaking off with an ellipsis. The fade lowers the alpha of the last
//! visible letters, so it suits any background, hovered or selected.

use gpui_kit::*;

/// Width of the fade in font sizes.
const FADE: f32 = 2.5;

/// A label clipped at its right edge, fading out there when it does not fit.
/// The font, colour and decorations come from the surrounding text style.
pub(in crate::ui) struct Faded {
    text: SharedString,
}

/// Line breaks become spaces: the label is a single line.
pub(in crate::ui) fn faded(text: impl Into<SharedString>) -> Faded {
    let text: SharedString = text.into();
    let text = if text.contains(['\n', '\r']) {
        text.replace(['\n', '\r'], " ").into()
    } else {
        text
    };
    Faded { text }
}

struct Metrics {
    font_size: Pixels,
    line_height: Pixels,
    style: TextStyle,
}

fn metrics(window: &Window) -> Metrics {
    let style = window.text_style();
    let font_size = style.font_size.to_pixels(window.rem_size());
    let line_height = window.pixel_snap(
        style
            .line_height
            .to_pixels(font_size.into(), window.rem_size()),
    );
    Metrics {
        font_size,
        line_height,
        style,
    }
}

impl IntoElement for Faded {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Faded {
    type RequestLayoutState = ();
    type PrepaintState = Option<(ShapedLine, Pixels)>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        _: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let Metrics {
            font_size,
            line_height,
            style,
        } = metrics(window);
        let runs = [style.to_run(self.text.len())];
        let width = window
            .text_system()
            .shape_line(self.text.clone(), font_size, &runs, None)
            .width();
        let mut layout = Style::default();
        layout.size.height = line_height.into();
        layout.min_size.width = px(0.).into();
        // Shrinks like clipped content: down to nothing, never past the text.
        let id = window.request_measured_layout(layout, move |known, available, _, _| {
            let width = known.width.unwrap_or(match available.width {
                AvailableSpace::Definite(room) => width.min(room),
                AvailableSpace::MinContent => px(0.),
                AvailableSpace::MaxContent => width,
            });
            size(width, line_height)
        });
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
        let Metrics {
            font_size,
            line_height,
            style,
        } = metrics(window);
        let text = &self.text;
        let system = window.text_system();
        let line = system.shape_line(text.clone(), font_size, &[style.to_run(text.len())], None);
        let room = bounds.size.width;
        if line.width() <= room {
            return Some((line, line_height));
        }
        let fade = (font_size * FADE).min(room);
        let start = line.index_for_x(room - fade).unwrap_or(0);
        let start = text.floor_char_boundary(start);
        let mut runs = Vec::new();
        if start > 0 {
            runs.push(style.to_run(start));
        }
        for (offset, letter) in text[start..].char_indices() {
            let at = start + offset;
            let end = at + letter.len_utf8();
            let middle = (line.x_for_index(at) + line.x_for_index(end)) / 2.;
            let mut run = style.to_run(letter.len_utf8());
            if middle >= room {
                run.len = text.len() - at;
                run.color = run.color.opacity(0.);
                runs.push(run);
                break;
            }
            run.color = run.color.opacity((room - middle) / fade);
            runs.push(run);
        }
        let line = system.shape_line(text.clone(), font_size, &runs, None);
        Some((line, line_height))
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some((line, line_height)) = prepaint.take() else {
            return;
        };
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            let _ = line.paint(
                bounds.origin,
                line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        });
    }
}
