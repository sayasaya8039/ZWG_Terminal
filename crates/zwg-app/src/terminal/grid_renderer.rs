use gpui::*;
use unicode_width::UnicodeWidthChar;

use super::{DEFAULT_BG, DEFAULT_FG};

const BRAILLE_BLANK: char = '\u{2800}';

#[cfg(feature = "ghostty_vt")]
pub(crate) const GHOSTTY_FLAG_BOLD: u8 = 0x02;
#[cfg(feature = "ghostty_vt")]
pub(crate) const GHOSTTY_FLAG_ITALIC: u8 = 0x04;
#[cfg(feature = "ghostty_vt")]
pub(crate) const GHOSTTY_FLAG_UNDERLINE: u8 = 0x08;
#[cfg(feature = "ghostty_vt")]
pub(crate) const GHOSTTY_FLAG_STRIKETHROUGH: u8 = 0x40;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct SelectionPoint {
    pub row: u16,
    pub col: u16,
}

#[derive(Clone, Default)]
pub(super) struct CachedTerminalRow {
    pub text: SharedString,
    #[cfg(feature = "ghostty_vt")]
    pub style_runs: Vec<ghostty_vt::StyleRun>,
}

#[derive(Clone)]
pub(super) struct TerminalSnapshot {
    pub rows: Vec<CachedTerminalRow>,
    pub cursor_x: u16,
    pub cursor_y: u16,
}

impl TerminalSnapshot {
    pub fn new(rows: u16) -> Self {
        Self {
            rows: vec![CachedTerminalRow::default(); rows as usize],
            cursor_x: 0,
            cursor_y: 0,
        }
    }

    pub fn resize(&mut self, rows: u16) {
        self.rows
            .resize(rows as usize, CachedTerminalRow::default());
    }
}

pub(super) struct GridRendererConfig {
    pub cell_width: f32,
    pub cell_height: f32,
    pub font_family: &'static str,
    pub font_size: f32,
    pub horizontal_text_padding: f32,
    pub term_cols: u16,
}

pub(super) fn char_cell_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0)
}

pub(super) fn col_to_char_index(text: &str, target_col: usize) -> usize {
    let mut col = 0;
    for (i, ch) in text.chars().enumerate() {
        if col >= target_col {
            return i;
        }
        col += char_cell_width(ch);
    }
    text.chars().count()
}

pub(crate) fn is_geometric_block_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{2580}'
            | '\u{2584}'
            | '\u{2588}'
            | '\u{258C}'
            | '\u{2590}'
            | '\u{2591}'
            | '\u{2592}'
            | '\u{2593}'
    )
}

pub(crate) fn sanitize_text_for_shaping(text: &str) -> String {
    text.chars()
        .map(|ch| if is_geometric_block_char(ch) { BRAILLE_BLANK } else { ch })
        .collect()
}

pub(super) fn terminal_canvas(
    snapshot: TerminalSnapshot,
    selection: Option<(SelectionPoint, SelectionPoint)>,
    config: GridRendererConfig,
) -> Canvas<()> {
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, _, window: &mut Window, cx: &mut App| {
            let text_system = window.text_system().clone();
            let font_desc = font(config.font_family);
            let font_size = px(config.font_size);
            let line_height_px = px(config.cell_height);
            let default_fg = Hsla::from(rgb(DEFAULT_FG));
            let num_rows = snapshot.rows.len();

            window.paint_quad(fill(bounds, Hsla::from(rgb(DEFAULT_BG))));

            if let Some((sel_start, sel_end)) = selection {
                let max_row = sel_end.row.min(num_rows.saturating_sub(1) as u16);
                for row in sel_start.row..=max_row {
                    let sc = if row == sel_start.row { sel_start.col } else { 0 };
                    let ec = if row == sel_end.row {
                        sel_end.col
                    } else {
                        config.term_cols
                    };
                    if sc >= ec {
                        continue;
                    }
                    window.paint_quad(fill(
                        Bounds::new(
                            point(
                                bounds.origin.x
                                    + px(
                                        config.horizontal_text_padding
                                            + sc as f32 * config.cell_width,
                                    ),
                                bounds.origin.y + px(row as f32 * config.cell_height),
                            ),
                            size(
                                px((ec - sc) as f32 * config.cell_width),
                                line_height_px,
                            ),
                        ),
                        rgba(0x2F6FED55),
                    ));
                }
            }

            for (row_idx, row_data) in snapshot.rows.iter().enumerate() {
                let row_y = row_idx as f32 * config.cell_height;

                #[cfg(feature = "ghostty_vt")]
                for srun in &row_data.style_runs {
                    let bg_val = ((srun.bg.r as u32) << 16)
                        | ((srun.bg.g as u32) << 8)
                        | (srun.bg.b as u32);
                    if bg_val != DEFAULT_BG {
                        let sc = srun.start_col.saturating_sub(1) as f32;
                        let ec = srun.end_col as f32;
                        if ec > sc {
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(
                                        bounds.origin.x
                                            + px(
                                                config.horizontal_text_padding
                                                    + sc * config.cell_width,
                                            ),
                                        bounds.origin.y + px(row_y),
                                    ),
                                    size(
                                        px((ec - sc) * config.cell_width),
                                        line_height_px,
                                    ),
                                ),
                                Hsla::from(rgb(bg_val)),
                            ));
                        }
                    }
                }

                let text = &row_data.text;
                if text.is_empty() {
                    continue;
                }
                let shaped_text = sanitize_text_for_shaping(text);
                let origin = point(
                    bounds.origin.x + px(config.horizontal_text_padding),
                    bounds.origin.y + px(row_y),
                );

                #[cfg(feature = "ghostty_vt")]
                let runs =
                    build_canvas_text_runs(&shaped_text, &row_data.style_runs, &font_desc, default_fg);
                #[cfg(not(feature = "ghostty_vt"))]
                let runs = vec![TextRun {
                    len: shaped_text.len(),
                    font: font_desc.clone(),
                    color: default_fg,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }];

                let force_width = if text.chars().all(|ch| char_cell_width(ch) <= 1) {
                    Some(px(config.cell_width))
                } else {
                    None
                };
                let shaped = text_system.shape_line(
                    SharedString::from(shaped_text),
                    font_size,
                    &runs,
                    force_width,
                );
                let _ = shaped.paint(origin, line_height_px, window, cx);

                #[cfg(feature = "ghostty_vt")]
                {
                    let mut col: usize = 0;
                    for ch in text.chars() {
                        let w = char_cell_width(ch);
                        if is_geometric_block_char(ch) {
                            let col_1 = (col + 1) as u16;
                            let fg = row_data
                                .style_runs
                                .iter()
                                .find(|r| r.start_col <= col_1 && col_1 <= r.end_col)
                                .map(|r| {
                                    let v = ((r.fg.r as u32) << 16)
                                        | ((r.fg.g as u32) << 8)
                                        | (r.fg.b as u32);
                                    Hsla::from(rgb(v))
                                })
                                .unwrap_or(default_fg);
                            let x = bounds.origin.x
                                + px(
                                    config.horizontal_text_padding
                                        + col as f32 * config.cell_width,
                                );
                            let y = bounds.origin.y + px(row_y);
                            let cw = px(config.cell_width * w as f32);
                            match ch {
                                '\u{2588}' => {
                                    window.paint_quad(fill(
                                        Bounds::new(point(x, y), size(cw, line_height_px)),
                                        fg,
                                    ));
                                }
                                '\u{2580}' => {
                                    let h = px(config.cell_height * 0.5);
                                    window.paint_quad(fill(
                                        Bounds::new(point(x, y), size(cw, h)),
                                        fg,
                                    ));
                                }
                                '\u{2584}' => {
                                    let h = px(config.cell_height * 0.5);
                                    window.paint_quad(fill(
                                        Bounds::new(point(x, y + h), size(cw, h)),
                                        fg,
                                    ));
                                }
                                '\u{258C}' => {
                                    let hw = px(config.cell_width * 0.5);
                                    window.paint_quad(fill(
                                        Bounds::new(point(x, y), size(hw, line_height_px)),
                                        fg,
                                    ));
                                }
                                '\u{2590}' => {
                                    let hw = px(config.cell_width * 0.5);
                                    window.paint_quad(fill(
                                        Bounds::new(point(x + hw, y), size(hw, line_height_px)),
                                        fg,
                                    ));
                                }
                                '\u{2591}' => {
                                    window.paint_quad(fill(
                                        Bounds::new(point(x, y), size(cw, line_height_px)),
                                        fg.opacity(0.25),
                                    ));
                                }
                                '\u{2592}' => {
                                    window.paint_quad(fill(
                                        Bounds::new(point(x, y), size(cw, line_height_px)),
                                        fg.opacity(0.5),
                                    ));
                                }
                                '\u{2593}' => {
                                    window.paint_quad(fill(
                                        Bounds::new(point(x, y), size(cw, line_height_px)),
                                        fg.opacity(0.75),
                                    ));
                                }
                                _ => {}
                            }
                        }
                        col += w;
                    }
                }
            }

            if (snapshot.cursor_y as usize) < num_rows {
                window.paint_quad(fill(
                    Bounds::new(
                        point(
                            bounds.origin.x
                                + px(
                                    config.horizontal_text_padding
                                        + snapshot.cursor_x as f32 * config.cell_width,
                                ),
                            bounds.origin.y + px(snapshot.cursor_y as f32 * config.cell_height),
                        ),
                        size(px(config.cell_width), line_height_px),
                    ),
                    rgba(0xF5F5F780),
                ));
            }
        },
    )
    .size_full()
}

#[cfg(feature = "ghostty_vt")]
fn build_canvas_text_runs(
    text: &str,
    style_runs: &[ghostty_vt::StyleRun],
    font_desc: &Font,
    default_fg: Hsla,
) -> Vec<TextRun> {
    if style_runs.is_empty() || text.is_empty() {
        return vec![TextRun {
            len: text.len(),
            font: font_desc.clone(),
            color: default_fg,
            background_color: None,
            underline: None,
            strikethrough: None,
        }];
    }

    let char_byte_offsets: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();

    let mut runs: Vec<TextRun> = Vec::new();
    let mut covered_to_byte: usize = 0;

    for run in style_runs {
        let start_char = col_to_char_index(text, run.start_col.saturating_sub(1) as usize);
        let end_char = col_to_char_index(text, run.end_col as usize);
        if start_char >= char_byte_offsets.len().saturating_sub(1) || start_char >= end_char {
            continue;
        }

        let byte_start = char_byte_offsets[start_char];
        let byte_end = char_byte_offsets[end_char.min(char_byte_offsets.len() - 1)];

        if byte_start < covered_to_byte {
            continue;
        }

        if covered_to_byte < byte_start {
            runs.push(TextRun {
                len: byte_start - covered_to_byte,
                font: font_desc.clone(),
                color: default_fg,
                background_color: None,
                underline: None,
                strikethrough: None,
            });
        }

        let fg_val = ((run.fg.r as u32) << 16) | ((run.fg.g as u32) << 8) | (run.fg.b as u32);
        let fg_color = Hsla::from(rgb(fg_val));

        let mut run_font = font_desc.clone();
        if run.flags & GHOSTTY_FLAG_BOLD != 0 {
            run_font.weight = FontWeight::BOLD;
        }
        if run.flags & GHOSTTY_FLAG_ITALIC != 0 {
            run_font.style = FontStyle::Italic;
        }

        runs.push(TextRun {
            len: byte_end - byte_start,
            font: run_font,
            color: fg_color,
            background_color: None,
            underline: if run.flags & GHOSTTY_FLAG_UNDERLINE != 0 {
                Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(fg_color),
                    wavy: false,
                })
            } else {
                None
            },
            strikethrough: if run.flags & GHOSTTY_FLAG_STRIKETHROUGH != 0 {
                Some(StrikethroughStyle {
                    thickness: px(1.0),
                    color: Some(fg_color),
                })
            } else {
                None
            },
        });
        covered_to_byte = byte_end;
    }

    if covered_to_byte < text.len() {
        runs.push(TextRun {
            len: text.len() - covered_to_byte,
            font: font_desc.clone(),
            color: default_fg,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }

    runs
}

#[cfg(all(test, feature = "ghostty_vt"))]
mod tests {
    use super::*;

    fn sample_style_run(flags: u8) -> ghostty_vt::StyleRun {
        ghostty_vt::StyleRun {
            start_col: 1,
            end_col: 3,
            fg: ghostty_vt::Rgb {
                r: 0xFF,
                g: 0x88,
                b: 0x33,
            },
            bg: ghostty_vt::Rgb {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            },
            flags,
        }
    }

    #[::core::prelude::v1::test]
    fn build_canvas_text_runs_uses_ghostty_flag_layout() {
        let runs = build_canvas_text_runs(
            "abc",
            &[sample_style_run(
                GHOSTTY_FLAG_BOLD
                    | GHOSTTY_FLAG_ITALIC
                    | GHOSTTY_FLAG_UNDERLINE
                    | GHOSTTY_FLAG_STRIKETHROUGH,
            )],
            &font("Consolas"),
            Hsla::from(rgb(DEFAULT_FG)),
        );

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].font.weight, FontWeight::BOLD);
        assert_eq!(runs[0].font.style, FontStyle::Italic);
        assert!(runs[0].underline.is_some());
        assert!(runs[0].strikethrough.is_some());
    }

    #[::core::prelude::v1::test]
    fn build_canvas_text_runs_does_not_confuse_italic_with_underline() {
        let runs = build_canvas_text_runs(
            "abc",
            &[sample_style_run(GHOSTTY_FLAG_ITALIC)],
            &font("Consolas"),
            Hsla::from(rgb(DEFAULT_FG)),
        );

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].font.style, FontStyle::Italic);
        assert!(runs[0].underline.is_none());
        assert!(runs[0].strikethrough.is_none());
    }

    #[::core::prelude::v1::test]
    fn char_cell_width_treats_wide_glyphs_as_two_cells() {
        assert_eq!(char_cell_width('a'), 1);
        assert_eq!(char_cell_width('あ'), 2);
        assert_eq!(char_cell_width('🔥'), 2);
        assert_eq!(char_cell_width('\u{FE0F}'), 0);
    }

    #[::core::prelude::v1::test]
    fn sanitize_text_for_shaping_replaces_block_glyphs_only() {
        let shaped = sanitize_text_for_shaping("A█▀B");
        assert_eq!(shaped, format!("A{BRAILLE_BLANK}{BRAILLE_BLANK}B"));
    }

    #[::core::prelude::v1::test]
    fn build_canvas_text_runs_maps_style_runs_by_terminal_columns() {
        let runs = build_canvas_text_runs(
            "🔥a",
            &[ghostty_vt::StyleRun {
                start_col: 1,
                end_col: 2,
                fg: ghostty_vt::Rgb {
                    r: 0xFF,
                    g: 0x88,
                    b: 0x33,
                },
                bg: ghostty_vt::Rgb {
                    r: 0x11,
                    g: 0x22,
                    b: 0x33,
                },
                flags: GHOSTTY_FLAG_UNDERLINE,
            }],
            &font("Consolas"),
            Hsla::from(rgb(DEFAULT_FG)),
        );

        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].len, "🔥".len());
        assert!(runs[0].underline.is_some());
        assert!(runs[1].underline.is_none());
    }
}
