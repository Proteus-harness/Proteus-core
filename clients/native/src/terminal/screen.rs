//! Immutable snapshot of the visible terminal screen with resolved colors.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Term, point_to_viewport};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};

/// The host theme's default colors; cells without an explicit color use them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Base {
    pub foreground: [u8; 3],
    pub background: [u8; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub foreground: [u8; 3],
    /// `None` keeps the view's own background.
    pub background: Option<[u8; 3]>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

/// Consecutive cells of one row sharing a style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub column: usize,
    /// Number of grid columns the run covers (wide glyphs take two).
    pub width: usize,
    pub text: String,
    pub style: Style,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub columns: usize,
    pub rows: Vec<Vec<Run>>,
    /// Row and column of the cursor when it is visible on screen.
    pub cursor: Option<(usize, usize)>,
    /// Lines scrolled back from the live screen.
    pub scrolled: usize,
}

impl Screen {
    #[cfg(test)]
    pub fn text(&self) -> String {
        let lines: Vec<String> = self
            .rows
            .iter()
            .map(|row| row.iter().map(|run| run.text.as_str()).collect())
            .collect();
        lines.join("\n")
    }
}

pub(super) fn snapshot<T>(term: &Term<T>, base: Base) -> Screen
where
    T: alacritty_terminal::event::EventListener,
{
    let content = term.renderable_content();
    let columns = term.columns();
    let mut rows: Vec<Vec<Run>> = vec![Vec::new(); term.screen_lines()];
    for indexed in content.display_iter {
        let cell = indexed.cell;
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        let Some(point) = point_to_viewport(content.display_offset, indexed.point) else {
            continue;
        };
        let Some(row) = rows.get_mut(point.line) else {
            continue;
        };
        let style = style(cell.fg, cell.bg, cell.flags, content.colors, base);
        let width = if cell.flags.contains(Flags::WIDE_CHAR) {
            2
        } else {
            1
        };
        let mut text = String::new();
        text.push(if cell.flags.contains(Flags::HIDDEN) {
            ' '
        } else {
            cell.c
        });
        if let Some(extra) = cell.zerowidth() {
            text.extend(extra);
        }
        match row.last_mut() {
            Some(run) if run.style == style && run.column + run.width == point.column.0 => {
                run.text.push_str(&text);
                run.width += width;
            }
            _ => row.push(Run {
                column: point.column.0,
                width,
                text,
                style,
            }),
        }
    }
    for row in &mut rows {
        trim(row);
    }
    let cursor = (content.cursor.shape != CursorShape::Hidden)
        .then(|| point_to_viewport(content.display_offset, content.cursor.point))
        .flatten()
        .map(|point| (point.line, point.column.0));
    Screen {
        columns,
        rows,
        cursor,
        scrolled: content.display_offset,
    }
}

/// Drops trailing blank space that paints nothing.
fn trim(row: &mut Vec<Run>) {
    while let Some(run) = row.last_mut() {
        if run.style.background.is_some() || run.style.underline || run.style.strike {
            return;
        }
        let kept = run.text.trim_end_matches(' ').len();
        if kept > 0 {
            let dropped = run.text.len() - kept;
            run.text.truncate(kept);
            run.width -= dropped;
            return;
        }
        row.pop();
    }
}

fn style(fg: Color, bg: Color, flags: Flags, colors: &Colors, base: Base) -> Style {
    let mut foreground = resolve(fg, colors, base);
    let mut background = match bg {
        Color::Named(NamedColor::Background) => None,
        color => Some(resolve(color, colors, base)),
    };
    if flags.contains(Flags::DIM) {
        foreground = dim(foreground);
    }
    if flags.contains(Flags::INVERSE) {
        let back = background.unwrap_or(base.background);
        background = Some(foreground);
        foreground = back;
    }
    Style {
        foreground,
        background,
        bold: flags.contains(Flags::BOLD),
        italic: flags.contains(Flags::ITALIC),
        underline: flags.intersects(Flags::ALL_UNDERLINES),
        strike: flags.contains(Flags::STRIKEOUT),
    }
}

fn resolve(color: Color, colors: &Colors, base: Base) -> [u8; 3] {
    match color {
        Color::Spec(rgb) => rgb_array(rgb),
        Color::Indexed(index) => colors[index as usize]
            .map(rgb_array)
            .unwrap_or_else(|| indexed(index)),
        Color::Named(name) => {
            if let Some(rgb) = colors[name] {
                return rgb_array(rgb);
            }
            match name {
                NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::Cursor => {
                    base.foreground
                }
                NamedColor::DimForeground => dim(base.foreground),
                NamedColor::Background => base.background,
                other => {
                    let value = other as usize;
                    if value < 16 {
                        ANSI[value]
                    } else {
                        // Dim variants follow the 16 base colors in order.
                        let dimmed = value - NamedColor::DimBlack as usize;
                        dim(ANSI[dimmed.min(7)])
                    }
                }
            }
        }
    }
}

fn rgb_array(rgb: Rgb) -> [u8; 3] {
    [rgb.r, rgb.g, rgb.b]
}

fn dim([r, g, b]: [u8; 3]) -> [u8; 3] {
    let scale = |value: u8| (value as f32 * 0.66) as u8;
    [scale(r), scale(g), scale(b)]
}

/// xterm-like palette tuned for dark and light backgrounds alike.
const ANSI: [[u8; 3]; 16] = [
    [0x1d, 0x1f, 0x21],
    [0xcc, 0x66, 0x66],
    [0xa3, 0xbe, 0x8c],
    [0xe5, 0xc0, 0x7b],
    [0x61, 0xaf, 0xef],
    [0xc6, 0x78, 0xdd],
    [0x56, 0xb6, 0xc2],
    [0xc5, 0xc8, 0xc6],
    [0x66, 0x66, 0x66],
    [0xf0, 0x7a, 0x7a],
    [0xb9, 0xd8, 0x9d],
    [0xf3, 0xd4, 0x93],
    [0x82, 0xc4, 0xff],
    [0xd9, 0x9c, 0xee],
    [0x7d, 0xd6, 0xe0],
    [0xff, 0xff, 0xff],
];

fn indexed(index: u8) -> [u8; 3] {
    match index {
        0..=15 => ANSI[index as usize],
        16..=231 => {
            let value = index - 16;
            let level = |step: u8| if step == 0 { 0 } else { 55 + step * 40 };
            [level(value / 36), level(value / 6 % 6), level(value % 6)]
        }
        _ => {
            let gray = 8 + (index - 232) * 10;
            [gray, gray, gray]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_covers_cube_and_grays() {
        assert_eq!(indexed(16), [0, 0, 0]);
        assert_eq!(indexed(231), [255, 255, 255]);
        assert_eq!(indexed(232), [8, 8, 8]);
        assert_eq!(indexed(255), [238, 238, 238]);
    }

    #[test]
    fn inverse_swaps_default_colors() {
        let base = Base {
            foreground: [1, 1, 1],
            background: [9, 9, 9],
        };
        let style = style(
            Color::Named(NamedColor::Foreground),
            Color::Named(NamedColor::Background),
            Flags::INVERSE,
            &Colors::default(),
            base,
        );
        assert_eq!(style.foreground, [9, 9, 9]);
        assert_eq!(style.background, Some([1, 1, 1]));
    }

    #[test]
    fn trailing_blanks_are_trimmed() {
        let plain = Style {
            foreground: [0; 3],
            background: None,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
        };
        let mut row = vec![
            Run {
                column: 0,
                width: 4,
                text: "ab  ".into(),
                style: plain,
            },
            Run {
                column: 4,
                width: 2,
                text: "  ".into(),
                style: Style {
                    bold: true,
                    ..plain
                },
            },
        ];
        trim(&mut row);
        assert_eq!(row.len(), 1);
        assert_eq!(row[0].text, "ab");
        assert_eq!(row[0].width, 2);
    }
}
