//! Built-in VT100/xterm escape sequence parser (Phase 0)
//! Will be replaced by Ghostty's VT parser in Phase 1

use super::ScreenBuffer;

/// ANSI 256-color palette (standard 16 colors)
const ANSI_COLORS: [u32; 16] = [
    0x45475a, // 0: black (Catppuccin Surface1)
    0xf38ba8, // 1: red
    0xa6e3a1, // 2: green
    0xf9e2af, // 3: yellow
    0x89b4fa, // 4: blue
    0xf5c2e7, // 5: magenta
    0x94e2d5, // 6: cyan
    0xbac2de, // 7: white (Subtext1)
    0x585b70, // 8: bright black (Surface2)
    0xf38ba8, // 9: bright red
    0xa6e3a1, // 10: bright green
    0xf9e2af, // 11: bright yellow
    0x89b4fa, // 12: bright blue
    0xf5c2e7, // 13: bright magenta
    0x94e2d5, // 14: bright cyan
    0xcdd6f4, // 15: bright white (Text)
];

#[derive(Debug, Clone, Copy, PartialEq)]
enum ParserState {
    Ground,
    Escape,
    CsiEntry,
    CsiParam,
    OscString,
}

pub struct VtParser {
    state: ParserState,
    params: Vec<u16>,
    current_param: u16,
    osc_buf: String,
    has_param: bool,
}

impl VtParser {
    pub fn new() -> Self {
        Self {
            state: ParserState::Ground,
            params: Vec::with_capacity(16),
            current_param: 0,
            osc_buf: String::new(),
            has_param: false,
        }
    }

    /// Process a chunk of bytes through the VT parser
    pub fn process(&mut self, data: &[u8], screen: &mut ScreenBuffer) {
        for &byte in data {
            self.process_byte(byte, screen);
        }
    }

    fn process_byte(&mut self, byte: u8, screen: &mut ScreenBuffer) {
        match self.state {
            ParserState::Ground => self.ground(byte, screen),
            ParserState::Escape => self.escape(byte, screen),
            ParserState::CsiEntry | ParserState::CsiParam => self.csi(byte, screen),
            ParserState::OscString => self.osc(byte, screen),
        }
    }

    fn ground(&mut self, byte: u8, screen: &mut ScreenBuffer) {
        match byte {
            0x1b => {
                self.state = ParserState::Escape;
            }
            b'\n' => {
                screen.cursor.y += 1;
                if screen.cursor.y >= screen.rows {
                    screen.scroll_up();
                    screen.cursor.y = screen.rows - 1;
                }
            }
            b'\r' => {
                screen.cursor.x = 0;
            }
            0x08 => {
                // Backspace
                screen.cursor.x = screen.cursor.x.saturating_sub(1);
            }
            0x07 => {
                // Bell — ignore
            }
            0x09 => {
                // Tab — advance to next 8-column stop
                let next = ((screen.cursor.x / 8) + 1) * 8;
                screen.cursor.x = next.min(screen.cols - 1);
            }
            0x00..=0x1f => {
                // Other control chars — ignore
            }
            _ => {
                // Printable character — handle UTF-8
                if byte < 0x80 {
                    screen.write_char(byte as char);
                } else {
                    // Simple UTF-8 handling: treat as individual bytes for now
                    // Full UTF-8 decoding will come with Ghostty VT in Phase 1
                    screen.write_char(char::REPLACEMENT_CHARACTER);
                }
            }
        }
    }

    fn escape(&mut self, byte: u8, screen: &mut ScreenBuffer) {
        match byte {
            b'[' => {
                self.state = ParserState::CsiEntry;
                self.params.clear();
                self.current_param = 0;
                self.has_param = false;
            }
            b']' => {
                self.state = ParserState::OscString;
                self.osc_buf.clear();
            }
            b'M' => {
                // Reverse Index — scroll down
                if screen.cursor.y == 0 {
                    // Insert line at top
                    screen.viewport.push_front(super::TerminalLine::new());
                    if screen.viewport.len() > screen.rows as usize {
                        screen.viewport.pop_back();
                    }
                    screen.generation = screen.generation.wrapping_add(1);
                    for rg in &mut screen.row_generations {
                        *rg = screen.generation;
                    }
                } else {
                    screen.cursor.y -= 1;
                }
                self.state = ParserState::Ground;
            }
            b'7' => {
                // Save cursor (DECSC) — simplified
                self.state = ParserState::Ground;
            }
            b'8' => {
                // Restore cursor (DECRC) — simplified
                self.state = ParserState::Ground;
            }
            b'=' | b'>' | b'(' | b')' => {
                // Keypad/charset modes — ignore for now
                self.state = ParserState::Ground;
            }
            _ => {
                self.state = ParserState::Ground;
            }
        }
    }

    fn csi(&mut self, byte: u8, screen: &mut ScreenBuffer) {
        match byte {
            b'0'..=b'9' => {
                self.has_param = true;
                self.current_param = self.current_param.saturating_mul(10) + (byte - b'0') as u16;
                self.state = ParserState::CsiParam;
            }
            b';' => {
                self.params.push(if self.has_param {
                    self.current_param
                } else {
                    0
                });
                self.current_param = 0;
                self.has_param = false;
                self.state = ParserState::CsiParam;
            }
            b'?' => {
                // Private mode prefix — continue parsing
                self.state = ParserState::CsiParam;
            }
            _ => {
                // Final byte — execute CSI sequence
                if self.has_param {
                    self.params.push(self.current_param);
                }
                self.execute_csi(byte, screen);
                self.state = ParserState::Ground;
            }
        }
    }

    fn execute_csi(&mut self, cmd: u8, screen: &mut ScreenBuffer) {
        let p = |i: usize, def: u16| -> u16 {
            self.params.get(i).copied().filter(|&v| v > 0).unwrap_or(def)
        };

        match cmd {
            b'A' => {
                // Cursor Up
                let n = p(0, 1);
                screen.cursor.y = screen.cursor.y.saturating_sub(n);
            }
            b'B' => {
                // Cursor Down
                let n = p(0, 1);
                screen.cursor.y = (screen.cursor.y + n).min(screen.rows - 1);
            }
            b'C' => {
                // Cursor Forward
                let n = p(0, 1);
                screen.cursor.x = (screen.cursor.x + n).min(screen.cols - 1);
            }
            b'D' => {
                // Cursor Back
                let n = p(0, 1);
                screen.cursor.x = screen.cursor.x.saturating_sub(n);
            }
            b'H' | b'f' => {
                // Cursor Position
                let row = p(0, 1).saturating_sub(1);
                let col = p(1, 1).saturating_sub(1);
                screen.cursor.y = row.min(screen.rows.saturating_sub(1));
                screen.cursor.x = col.min(screen.cols.saturating_sub(1));
            }
            b'J' => {
                // Erase in Display
                let mode = p(0, 0);
                match mode {
                    0 => {
                        // Clear from cursor to end
                        let y = screen.cursor.y as usize;
                        let x = screen.cursor.x as usize;
                        if let Some(line) = screen.viewport.get_mut(y) {
                            if x < line.text.len() {
                                line.text.truncate(x);
                            }
                            line.is_dirty = true;
                        }
                        for row in (y + 1)..screen.viewport.len() {
                            screen.viewport[row] = super::TerminalLine::new();
                        }
                    }
                    1 => {
                        // Clear from start to cursor
                        let y = screen.cursor.y as usize;
                        for row in 0..y {
                            screen.viewport[row] = super::TerminalLine::new();
                        }
                        if let Some(line) = screen.viewport.get_mut(y) {
                            let x = screen.cursor.x as usize;
                            let spaces: String = " ".repeat(x.min(line.text.len()));
                            line.text.replace_range(..spaces.len(), &spaces);
                            line.is_dirty = true;
                        }
                    }
                    2 | 3 => {
                        // Clear entire screen
                        for line in &mut screen.viewport {
                            *line = super::TerminalLine::new();
                        }
                        screen.cursor.x = 0;
                        screen.cursor.y = 0;
                    }
                    _ => {}
                }
                screen.generation = screen.generation.wrapping_add(1);
                for rg in &mut screen.row_generations {
                    *rg = screen.generation;
                }
            }
            b'K' => {
                // Erase in Line
                let mode = p(0, 0);
                let y = screen.cursor.y as usize;
                if let Some(line) = screen.viewport.get_mut(y) {
                    let x = screen.cursor.x as usize;
                    match mode {
                        0 => {
                            // Clear from cursor to end
                            if x < line.text.len() {
                                line.text.truncate(x);
                            }
                        }
                        1 => {
                            // Clear from start to cursor
                            let len = x.min(line.text.len());
                            let spaces = " ".repeat(len);
                            line.text.replace_range(..len, &spaces);
                        }
                        2 => {
                            line.text.clear();
                        }
                        _ => {}
                    }
                    line.is_dirty = true;
                    screen.generation = screen.generation.wrapping_add(1);
                    if let Some(rg) = screen.row_generations.get_mut(y) {
                        *rg = screen.generation;
                    }
                }
            }
            b'L' => {
                // Insert Lines
                let n = p(0, 1) as usize;
                let y = screen.cursor.y as usize;
                for _ in 0..n {
                    if screen.viewport.len() > y {
                        screen.viewport.insert(y, super::TerminalLine::new());
                        if screen.viewport.len() > screen.rows as usize {
                            screen.viewport.pop_back();
                        }
                    }
                }
                screen.generation = screen.generation.wrapping_add(1);
                for rg in &mut screen.row_generations {
                    *rg = screen.generation;
                }
            }
            b'M' => {
                // Delete Lines
                let n = p(0, 1) as usize;
                let y = screen.cursor.y as usize;
                for _ in 0..n {
                    if y < screen.viewport.len() {
                        screen.viewport.remove(y);
                        screen.viewport.push_back(super::TerminalLine::new());
                    }
                }
                screen.generation = screen.generation.wrapping_add(1);
                for rg in &mut screen.row_generations {
                    *rg = screen.generation;
                }
            }
            b'P' => {
                // Delete Characters
                let n = p(0, 1) as usize;
                let y = screen.cursor.y as usize;
                let x = screen.cursor.x as usize;
                if let Some(line) = screen.viewport.get_mut(y) {
                    let char_count = line.text.chars().count();
                    if x < char_count {
                        let end = (x + n).min(char_count);
                        let byte_start = line
                            .text
                            .char_indices()
                            .nth(x)
                            .map(|(i, _)| i)
                            .unwrap_or(line.text.len());
                        let byte_end = line
                            .text
                            .char_indices()
                            .nth(end)
                            .map(|(i, _)| i)
                            .unwrap_or(line.text.len());
                        line.text.replace_range(byte_start..byte_end, "");
                        line.is_dirty = true;
                    }
                }
            }
            b'm' => {
                // SGR — Select Graphic Rendition
                if self.params.is_empty() {
                    screen.current_fg = super::DEFAULT_FG;
                    screen.current_bg = super::DEFAULT_BG;
                    screen.current_flags = 0;
                    return;
                }
                let mut i = 0;
                while i < self.params.len() {
                    match self.params[i] {
                        0 => {
                            screen.current_fg = super::DEFAULT_FG;
                            screen.current_bg = super::DEFAULT_BG;
                            screen.current_flags = 0;
                        }
                        1 => screen.current_flags |= 0x01,  // Bold
                        3 => screen.current_flags |= 0x02,  // Italic
                        4 => screen.current_flags |= 0x04,  // Underline
                        7 => screen.current_flags |= 0x08,  // Inverse
                        9 => screen.current_flags |= 0x10,  // Strikethrough
                        22 => screen.current_flags &= !0x01, // Not bold
                        23 => screen.current_flags &= !0x02, // Not italic
                        24 => screen.current_flags &= !0x04, // Not underline
                        27 => screen.current_flags &= !0x08, // Not inverse
                        29 => screen.current_flags &= !0x10, // Not strikethrough
                        30..=37 => {
                            screen.current_fg = ANSI_COLORS[(self.params[i] - 30) as usize];
                        }
                        38 => {
                            // Extended foreground
                            if i + 2 < self.params.len() && self.params[i + 1] == 5 {
                                // 256-color
                                let idx = self.params[i + 2] as usize;
                                if idx < 16 {
                                    screen.current_fg = ANSI_COLORS[idx];
                                }
                                i += 2;
                            } else if i + 4 < self.params.len() && self.params[i + 1] == 2 {
                                // RGB
                                let r = self.params[i + 2] as u32;
                                let g = self.params[i + 3] as u32;
                                let b = self.params[i + 4] as u32;
                                screen.current_fg = (r << 16) | (g << 8) | b;
                                i += 4;
                            }
                        }
                        39 => screen.current_fg = super::DEFAULT_FG,
                        40..=47 => {
                            screen.current_bg = ANSI_COLORS[(self.params[i] - 40) as usize];
                        }
                        48 => {
                            // Extended background
                            if i + 2 < self.params.len() && self.params[i + 1] == 5 {
                                let idx = self.params[i + 2] as usize;
                                if idx < 16 {
                                    screen.current_bg = ANSI_COLORS[idx];
                                }
                                i += 2;
                            } else if i + 4 < self.params.len() && self.params[i + 1] == 2 {
                                let r = self.params[i + 2] as u32;
                                let g = self.params[i + 3] as u32;
                                let b = self.params[i + 4] as u32;
                                screen.current_bg = (r << 16) | (g << 8) | b;
                                i += 4;
                            }
                        }
                        49 => screen.current_bg = super::DEFAULT_BG,
                        90..=97 => {
                            screen.current_fg = ANSI_COLORS[(self.params[i] - 90 + 8) as usize];
                        }
                        100..=107 => {
                            screen.current_bg = ANSI_COLORS[(self.params[i] - 100 + 8) as usize];
                        }
                        _ => {}
                    }
                    i += 1;
                }
            }
            b'r' => {
                // Set Scrolling Region (DECSTBM) — simplified
            }
            b'h' | b'l' => {
                // Set/Reset Mode — handle cursor visibility
                if !self.params.is_empty() && self.params[0] == 25 {
                    screen.cursor.visible = cmd == b'h';
                }
                // 1049: Alt screen buffer
                if !self.params.is_empty() && self.params[0] == 1049 {
                    if cmd == b'h' {
                        // Switch to alt screen
                        let saved = std::mem::replace(
                            &mut screen.viewport,
                            (0..screen.rows).map(|_| super::TerminalLine::new()).collect(),
                        );
                        screen.alternate_screen = Some(saved);
                        screen.cursor.x = 0;
                        screen.cursor.y = 0;
                    } else if let Some(saved) = screen.alternate_screen.take() {
                        screen.viewport = saved;
                    }
                    screen.generation = screen.generation.wrapping_add(1);
                    for rg in &mut screen.row_generations {
                        *rg = screen.generation;
                    }
                }
            }
            b'd' => {
                // Vertical Position Absolute
                let row = p(0, 1).saturating_sub(1);
                screen.cursor.y = row.min(screen.rows.saturating_sub(1));
            }
            b'G' => {
                // Cursor Horizontal Absolute
                let col = p(0, 1).saturating_sub(1);
                screen.cursor.x = col.min(screen.cols.saturating_sub(1));
            }
            b'@' => {
                // Insert Characters
                let n = p(0, 1) as usize;
                let y = screen.cursor.y as usize;
                let x = screen.cursor.x as usize;
                if let Some(line) = screen.viewport.get_mut(y) {
                    let byte_pos = line
                        .text
                        .char_indices()
                        .nth(x)
                        .map(|(i, _)| i)
                        .unwrap_or(line.text.len());
                    let spaces: String = " ".repeat(n);
                    line.text.insert_str(byte_pos, &spaces);
                    // Truncate to column width
                    let cols = screen.cols as usize;
                    if line.text.chars().count() > cols {
                        let trunc_pos = line
                            .text
                            .char_indices()
                            .nth(cols)
                            .map(|(i, _)| i)
                            .unwrap_or(line.text.len());
                        line.text.truncate(trunc_pos);
                    }
                    line.is_dirty = true;
                }
            }
            b'X' => {
                // Erase Characters
                let n = p(0, 1) as usize;
                let y = screen.cursor.y as usize;
                let x = screen.cursor.x as usize;
                if let Some(line) = screen.viewport.get_mut(y) {
                    let char_count = line.text.chars().count();
                    if x < char_count {
                        let end = (x + n).min(char_count);
                        let byte_start = line
                            .text
                            .char_indices()
                            .nth(x)
                            .map(|(i, _)| i)
                            .unwrap_or(line.text.len());
                        let byte_end = line
                            .text
                            .char_indices()
                            .nth(end)
                            .map(|(i, _)| i)
                            .unwrap_or(line.text.len());
                        let spaces = " ".repeat(end - x);
                        line.text.replace_range(byte_start..byte_end, &spaces);
                        line.is_dirty = true;
                    }
                }
            }
            _ => {
                // Unhandled CSI — ignore
            }
        }
    }

    fn osc(&mut self, byte: u8, screen: &mut ScreenBuffer) {
        match byte {
            0x07 => {
                // BEL terminates OSC
                self.execute_osc(screen);
                self.state = ParserState::Ground;
            }
            0x1b => {
                // ESC might terminate OSC (ST = ESC \)
                // Simplified: just execute
                self.execute_osc(screen);
                self.state = ParserState::Ground;
            }
            _ => {
                if self.osc_buf.len() < 4096 {
                    self.osc_buf.push(byte as char);
                }
            }
        }
    }

    fn execute_osc(&self, screen: &mut ScreenBuffer) {
        // OSC 0 or 2: Set window title
        if let Some(rest) = self.osc_buf.strip_prefix("0;").or(self.osc_buf.strip_prefix("2;")) {
            screen.title = Some(rest.to_string());
        }
    }
}
