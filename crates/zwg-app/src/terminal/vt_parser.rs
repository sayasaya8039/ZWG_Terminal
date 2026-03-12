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

/// H8: Convert 256-color index to RGB u32
fn color_256(idx: usize) -> u32 {
    if idx < 16 {
        ANSI_COLORS[idx]
    } else if idx < 232 {
        // 6x6x6 color cube (indices 16-231)
        let i = idx - 16;
        let b = (i % 6) as u32;
        let g = ((i / 6) % 6) as u32;
        let r = (i / 36) as u32;
        let to_byte = |v: u32| if v == 0 { 0u32 } else { 55 + v * 40 };
        (to_byte(r) << 16) | (to_byte(g) << 8) | to_byte(b)
    } else if idx < 256 {
        // Grayscale ramp (indices 232-255)
        let v = (8 + (idx - 232) * 10) as u32;
        (v << 16) | (v << 8) | v
    } else {
        super::DEFAULT_FG
    }
}

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
    osc_buf: Vec<u8>,
    has_param: bool,
    // C1: UTF-8 decode state machine
    utf8_buf: [u8; 4],
    utf8_len: u8,
    utf8_expected: u8,
}

impl VtParser {
    pub fn new() -> Self {
        Self {
            state: ParserState::Ground,
            params: Vec::with_capacity(16),
            current_param: 0,
            osc_buf: Vec::new(),
            has_param: false,
            utf8_buf: [0; 4],
            utf8_len: 0,
            utf8_expected: 0,
        }
    }

    /// Process a chunk of bytes through the VT parser
    pub fn process(&mut self, data: &[u8], screen: &mut ScreenBuffer) {
        for &byte in data {
            self.process_byte(byte, screen);
        }
    }

    /// Flush any incomplete UTF-8 sequence as replacement characters
    fn flush_utf8_incomplete(&mut self, screen: &mut ScreenBuffer) {
        if self.utf8_expected > 0 {
            screen.write_char(char::REPLACEMENT_CHARACTER);
            self.utf8_len = 0;
            self.utf8_expected = 0;
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
                // C1: UTF-8 decode state machine
                if byte < 0x80 {
                    // ASCII — flush any incomplete UTF-8 sequence, then write
                    self.flush_utf8_incomplete(screen);
                    screen.write_char(byte as char);
                } else if byte & 0xC0 == 0x80 {
                    // Continuation byte (10xxxxxx)
                    if self.utf8_expected > 0 {
                        self.utf8_buf[self.utf8_len as usize] = byte;
                        self.utf8_len += 1;
                        if self.utf8_len == self.utf8_expected {
                            // Sequence complete — decode
                            let s = &self.utf8_buf[..self.utf8_len as usize];
                            match std::str::from_utf8(s) {
                                Ok(decoded) => {
                                    for ch in decoded.chars() {
                                        screen.write_char(ch);
                                    }
                                }
                                Err(_) => {
                                    screen.write_char(char::REPLACEMENT_CHARACTER);
                                }
                            }
                            self.utf8_len = 0;
                            self.utf8_expected = 0;
                        }
                    } else {
                        // Stray continuation byte
                        screen.write_char(char::REPLACEMENT_CHARACTER);
                    }
                } else {
                    // Start byte — flush any incomplete, start new sequence
                    self.flush_utf8_incomplete(screen);
                    let expected = if byte & 0xE0 == 0xC0 {
                        2
                    } else if byte & 0xF0 == 0xE0 {
                        3
                    } else if byte & 0xF8 == 0xF0 {
                        4
                    } else {
                        0 // invalid
                    };
                    if expected > 0 {
                        self.utf8_buf[0] = byte;
                        self.utf8_len = 1;
                        self.utf8_expected = expected;
                    } else {
                        screen.write_char(char::REPLACEMENT_CHARACTER);
                    }
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
            self.params
                .get(i)
                .copied()
                .filter(|&v| v > 0)
                .unwrap_or(def)
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
                        1 => screen.current_flags |= 0x01, // Bold
                        3 => screen.current_flags |= 0x02, // Italic
                        4 => screen.current_flags |= 0x04, // Underline
                        7 => screen.current_flags |= 0x08, // Inverse
                        9 => screen.current_flags |= 0x10, // Strikethrough
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
                                // H8: 256-color (0-15: ANSI, 16-231: 6x6x6 cube, 232-255: grayscale)
                                let idx = self.params[i + 2] as usize;
                                screen.current_fg = color_256(idx);
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
                                // H8: 256-color
                                let idx = self.params[i + 2] as usize;
                                screen.current_bg = color_256(idx);
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
                            (0..screen.rows)
                                .map(|_| super::TerminalLine::new())
                                .collect(),
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
                // L5: ESC starts ST (ESC \). Execute OSC and transition to Escape
                // state so the backslash is consumed properly.
                self.execute_osc(screen);
                self.state = ParserState::Escape;
            }
            _ => {
                // M6: store raw bytes, decode as UTF-8 in execute_osc
                if self.osc_buf.len() < 4096 {
                    self.osc_buf.push(byte);
                }
            }
        }
    }

    fn execute_osc(&self, screen: &mut ScreenBuffer) {
        // M6: decode OSC buffer as UTF-8 (lossy)
        let osc_str = String::from_utf8_lossy(&self.osc_buf);
        // OSC 0 or 2: Set window title
        if let Some(rest) = osc_str
            .strip_prefix("0;")
            .or_else(|| osc_str.strip_prefix("2;"))
        {
            screen.title = Some(rest.to_string());
        }
    }
}
