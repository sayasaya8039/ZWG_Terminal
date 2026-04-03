//! Input acceleration — techniques ported from herm (aduermael/herm).
//!
//! Provides ESC sequence debouncing, bracketed paste detection/folding,
//! UTF-8 multi-byte assembly, and double-tap detection.

use std::time::{Duration, Instant};

/// Timeout for distinguishing a standalone ESC press from an escape sequence.
const ESC_SEQUENCE_TIMEOUT: Duration = Duration::from_millis(20);

/// Default threshold for collapsing large pastes.
const PASTE_COLLAPSE_THRESHOLD: usize = 5000;

/// Double-tap detection window.
const DOUBLE_TAP_WINDOW: Duration = Duration::from_secs(2);

/// Bracketed paste start marker.
const PASTE_START: &[u8] = b"\x1b[200~";

/// Bracketed paste end marker.
const PASTE_END: &[u8] = b"\x1b[201~";

/// Result of processing an input event.
#[derive(Debug, Clone)]
pub enum InputEvent {
    /// A regular character (possibly multi-byte UTF-8).
    Char(char),
    /// An escape key press (standalone, not part of a sequence).
    Escape,
    /// A control key (e.g., Ctrl+C = 3, Ctrl+D = 4).
    Control(u8),
    /// A complete escape sequence (e.g., arrow keys, function keys).
    EscSequence(Vec<u8>),
    /// Pasted text (from bracketed paste mode).
    Paste(PasteData),
    /// Double-tap of a control key detected.
    DoubleTap(u8),
    /// No event (timeout or incomplete data).
    None,
}

/// Data from a bracketed paste event.
#[derive(Debug, Clone)]
pub struct PasteData {
    /// The raw pasted text.
    pub text: String,
    /// Whether the paste was collapsed (too large to display inline).
    pub collapsed: bool,
    /// Original length before collapsing.
    pub original_len: usize,
}

/// ESC key debouncer — distinguishes standalone ESC from escape sequences.
///
/// When an ESC byte (0x1B) arrives, waits up to 50ms for follow-up bytes.
/// If more bytes arrive, it's an escape sequence. If not, it's a standalone ESC.
pub struct EscDebouncer {
    esc_received_at: Option<Instant>,
    sequence_buf: Vec<u8>,
}

impl EscDebouncer {
    pub fn new() -> Self {
        Self {
            esc_received_at: None,
            sequence_buf: Vec::with_capacity(16),
        }
    }

    /// Feed a byte into the debouncer. Returns an InputEvent when one is complete.
    pub fn feed(&mut self, byte: u8) -> InputEvent {
        match byte {
            0x1B => {
                // If we were already collecting a sequence, flush it first
                if !self.sequence_buf.is_empty() {
                    let seq = std::mem::take(&mut self.sequence_buf);
                    self.esc_received_at = Some(Instant::now());
                    return InputEvent::EscSequence(seq);
                }
                self.esc_received_at = Some(Instant::now());
                self.sequence_buf.clear();
                InputEvent::None
            }
            _ if self.esc_received_at.is_some() => {
                self.sequence_buf.push(byte);
                // Check if sequence is complete
                if is_sequence_complete(&self.sequence_buf) {
                    self.esc_received_at = None;
                    let seq = std::mem::take(&mut self.sequence_buf);
                    InputEvent::EscSequence(seq)
                } else {
                    InputEvent::None
                }
            }
            0x01..=0x1A => InputEvent::Control(byte), // Ctrl+A through Ctrl+Z
            _ => InputEvent::None, // Handle via UTF-8 assembler
        }
    }

    /// Check if a pending ESC has timed out (= standalone ESC key).
    /// Call this periodically (e.g., every 10ms).
    pub fn check_timeout(&mut self) -> Option<InputEvent> {
        if let Some(t) = self.esc_received_at {
            if t.elapsed() >= ESC_SEQUENCE_TIMEOUT {
                self.esc_received_at = None;
                if self.sequence_buf.is_empty() {
                    return Some(InputEvent::Escape);
                } else {
                    let seq = std::mem::take(&mut self.sequence_buf);
                    return Some(InputEvent::EscSequence(seq));
                }
            }
        }
        None
    }

    /// Whether we're currently waiting for more ESC sequence bytes.
    pub fn is_pending(&self) -> bool {
        self.esc_received_at.is_some()
    }
}

impl Default for EscDebouncer {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if a CSI/SS3 sequence is complete.
fn is_sequence_complete(seq: &[u8]) -> bool {
    if seq.is_empty() {
        return false;
    }

    match seq[0] {
        b'[' => {
            // CSI sequence: ends with 0x40-0x7E (@ through ~)
            if let Some(&last) = seq.last() {
                (0x40..=0x7E).contains(&last) && seq.len() >= 2
            } else {
                false
            }
        }
        b'O' => {
            // SS3 sequence: single byte after O
            seq.len() >= 2
        }
        _ => {
            // Alt+key: single byte after ESC
            true
        }
    }
}

/// Bracketed paste detector — collects paste content between markers
/// and optionally collapses large pastes.
pub struct PasteDetector {
    /// Whether we're currently inside a bracketed paste.
    in_paste: bool,
    /// Buffer for paste content.
    paste_buf: Vec<u8>,
    /// Marker detection buffer.
    marker_buf: Vec<u8>,
    /// Collapse threshold (chars).
    collapse_threshold: usize,
}

impl PasteDetector {
    pub fn new() -> Self {
        Self {
            in_paste: false,
            paste_buf: Vec::with_capacity(4096),
            marker_buf: Vec::with_capacity(8),
            collapse_threshold: PASTE_COLLAPSE_THRESHOLD,
        }
    }

    /// Set custom collapse threshold.
    pub fn with_threshold(mut self, threshold: usize) -> Self {
        self.collapse_threshold = threshold;
        self
    }

    /// Feed a byte. Returns Some(PasteData) when a paste is complete.
    pub fn feed(&mut self, byte: u8) -> Option<PasteData> {
        if self.in_paste {
            self.paste_buf.push(byte);

            // Check for end marker
            if self.paste_buf.ends_with(PASTE_END) {
                self.in_paste = false;
                let text_len = self.paste_buf.len() - PASTE_END.len();
                let raw = &self.paste_buf[..text_len];

                // Normalize: strip \r, keep \n
                let text: String = String::from_utf8_lossy(raw)
                    .replace('\r', "");

                let original_len = text.len();
                let collapsed = original_len > self.collapse_threshold;

                let result = PasteData {
                    text: if collapsed {
                        format!(
                            "[pasted | {} chars]",
                            original_len
                        )
                    } else {
                        text
                    },
                    collapsed,
                    original_len,
                };

                self.paste_buf.clear();
                return Some(result);
            }
            return None;
        }

        // Not in paste — check for start marker
        self.marker_buf.push(byte);
        if self.marker_buf.len() > PASTE_START.len() {
            self.marker_buf.drain(..self.marker_buf.len() - PASTE_START.len());
        }

        if self.marker_buf == PASTE_START {
            self.in_paste = true;
            self.paste_buf.clear();
            self.marker_buf.clear();
        }

        None
    }

    /// Whether we're currently inside a paste.
    pub fn is_pasting(&self) -> bool {
        self.in_paste
    }
}

impl Default for PasteDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// Double-tap detector for control keys (e.g., Ctrl+C twice to exit).
pub struct DoubleTapDetector {
    last_key: Option<u8>,
    last_time: Option<Instant>,
    window: Duration,
}

impl DoubleTapDetector {
    pub fn new() -> Self {
        Self {
            last_key: None,
            last_time: None,
            window: DOUBLE_TAP_WINDOW,
        }
    }

    /// Feed a control key byte. Returns true if it's a double-tap.
    pub fn feed(&mut self, key: u8) -> bool {
        let now = Instant::now();

        if let (Some(prev_key), Some(prev_time)) = (self.last_key, self.last_time) {
            if prev_key == key && now.duration_since(prev_time) < self.window {
                self.last_key = None;
                self.last_time = None;
                return true;
            }
        }

        self.last_key = Some(key);
        self.last_time = Some(now);
        false
    }
}

impl Default for DoubleTapDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// UTF-8 multi-byte assembler — collects bytes until a complete
/// Unicode codepoint is formed.
pub struct Utf8Assembler {
    buf: [u8; 4],
    len: usize,
    expected: usize,
}

impl Utf8Assembler {
    pub fn new() -> Self {
        Self {
            buf: [0; 4],
            len: 0,
            expected: 0,
        }
    }

    /// Feed a byte. Returns Some(char) when a complete codepoint is assembled.
    pub fn feed(&mut self, byte: u8) -> Option<char> {
        if self.expected == 0 {
            // Start of a new character
            self.expected = utf8_byte_len(byte);
            if self.expected == 0 {
                return None; // invalid
            }
            if self.expected == 1 {
                return char::from_u32(byte as u32);
            }
            self.buf[0] = byte;
            self.len = 1;
            None
        } else {
            // Continuation byte
            if byte & 0xC0 != 0x80 {
                // Invalid continuation — reset
                self.len = 0;
                self.expected = 0;
                return None;
            }
            self.buf[self.len] = byte;
            self.len += 1;

            if self.len == self.expected {
                let result = std::str::from_utf8(&self.buf[..self.len])
                    .ok()
                    .and_then(|s| s.chars().next());
                self.len = 0;
                self.expected = 0;
                result
            } else {
                None
            }
        }
    }

    /// Whether we're in the middle of a multi-byte sequence.
    pub fn is_pending(&self) -> bool {
        self.len > 0
    }
}

impl Default for Utf8Assembler {
    fn default() -> Self {
        Self::new()
    }
}

/// Determine the expected length of a UTF-8 character from its first byte.
fn utf8_byte_len(byte: u8) -> usize {
    match byte {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 0, // invalid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esc_debouncer_standalone_esc() {
        let mut d = EscDebouncer::new();
        assert!(matches!(d.feed(0x1B), InputEvent::None));
        // Simulate timeout
        std::thread::sleep(Duration::from_millis(60));
        let event = d.check_timeout();
        assert!(matches!(event, Some(InputEvent::Escape)));
    }

    #[test]
    fn esc_debouncer_csi_sequence() {
        let mut d = EscDebouncer::new();
        d.feed(0x1B);
        d.feed(b'[');
        let event = d.feed(b'A'); // Up arrow = ESC [ A
        assert!(matches!(event, InputEvent::EscSequence(ref s) if s == &[b'[', b'A']));
    }

    #[test]
    fn esc_debouncer_control_key() {
        let mut d = EscDebouncer::new();
        let event = d.feed(3); // Ctrl+C
        assert!(matches!(event, InputEvent::Control(3)));
    }

    #[test]
    fn utf8_assembler_ascii() {
        let mut a = Utf8Assembler::new();
        assert_eq!(a.feed(b'A'), Some('A'));
        assert!(!a.is_pending());
    }

    #[test]
    fn utf8_assembler_multibyte() {
        let mut a = Utf8Assembler::new();
        // 日 = E6 97 A5
        assert_eq!(a.feed(0xE6), None);
        assert!(a.is_pending());
        assert_eq!(a.feed(0x97), None);
        assert_eq!(a.feed(0xA5), Some('日'));
        assert!(!a.is_pending());
    }

    #[test]
    fn double_tap_detector() {
        let mut d = DoubleTapDetector::new();
        assert!(!d.feed(3)); // first Ctrl+C
        assert!(d.feed(3)); // second Ctrl+C within window
        assert!(!d.feed(3)); // reset, first again
    }

    #[test]
    fn paste_detector_basic() {
        let mut pd = PasteDetector::new();

        // Feed paste start marker
        for &b in PASTE_START {
            assert!(pd.feed(b).is_none());
        }
        assert!(pd.is_pasting());

        // Feed paste content
        for &b in b"hello paste" {
            assert!(pd.feed(b).is_none());
        }

        // Feed paste end marker
        let mut result = None;
        for &b in PASTE_END {
            result = pd.feed(b);
        }
        let data = result.unwrap();
        assert_eq!(data.text, "hello paste");
        assert!(!data.collapsed);
        assert!(!pd.is_pasting());
    }

    #[test]
    fn paste_detector_collapse() {
        let mut pd = PasteDetector::new().with_threshold(10);

        for &b in PASTE_START {
            pd.feed(b);
        }
        for &b in b"this is a very long paste that exceeds threshold" {
            pd.feed(b);
        }
        let mut result = None;
        for &b in PASTE_END {
            result = pd.feed(b);
        }
        let data = result.unwrap();
        assert!(data.collapsed);
        assert!(data.text.contains("chars]"));
    }

    #[test]
    fn utf8_byte_len_values() {
        assert_eq!(utf8_byte_len(b'A'), 1);
        assert_eq!(utf8_byte_len(0xC3), 2); // ä, ö, etc.
        assert_eq!(utf8_byte_len(0xE6), 3); // CJK
        assert_eq!(utf8_byte_len(0xF0), 4); // emoji
        assert_eq!(utf8_byte_len(0x80), 0); // invalid start
    }

    #[test]
    fn is_sequence_complete_csi() {
        assert!(is_sequence_complete(b"[A"));       // Up arrow
        assert!(is_sequence_complete(b"[1;5C"));    // Ctrl+Right
        assert!(!is_sequence_complete(b"[1;5"));    // incomplete
        assert!(!is_sequence_complete(b"["));        // incomplete
    }
}
