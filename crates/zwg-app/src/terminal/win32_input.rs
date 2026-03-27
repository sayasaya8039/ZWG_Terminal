use std::fmt::Write as _;

const WIN32_INPUT_MODE_ENABLE: &[u8] = b"\x1b[?9001h";
const WIN32_INPUT_MODE_DISABLE: &[u8] = b"\x1b[?9001l";
const WIN32_INPUT_MODE_TAIL_LEN: usize =
    if WIN32_INPUT_MODE_ENABLE.len() > WIN32_INPUT_MODE_DISABLE.len() {
        WIN32_INPUT_MODE_ENABLE.len() - 1
    } else {
        WIN32_INPUT_MODE_DISABLE.len() - 1
    };

#[derive(Debug, Default)]
pub(crate) struct Win32InputModeTracker {
    active: bool,
    tail: Vec<u8>,
}

impl Win32InputModeTracker {
    pub(crate) fn observe(&mut self, bytes: &[u8]) -> bool {
        let mut haystack = Vec::with_capacity(self.tail.len() + bytes.len());
        haystack.extend_from_slice(&self.tail);
        haystack.extend_from_slice(bytes);

        for index in 0..haystack.len() {
            if haystack[index..].starts_with(WIN32_INPUT_MODE_ENABLE) {
                self.active = true;
            } else if haystack[index..].starts_with(WIN32_INPUT_MODE_DISABLE) {
                self.active = false;
            }
        }

        let tail_start = haystack.len().saturating_sub(WIN32_INPUT_MODE_TAIL_LEN);
        self.tail.clear();
        self.tail.extend_from_slice(&haystack[tail_start..]);
        self.active
    }

    #[cfg(test)]
    pub(crate) fn is_active(&self) -> bool {
        self.active
    }
}

pub(crate) fn encode_win32_input_text(text: &str) -> Vec<u8> {
    let mut out = String::new();
    for code_unit in text.encode_utf16() {
        push_key_event_record(&mut out, 0, 0, code_unit, true, 0, 1);
        push_key_event_record(&mut out, 0, 0, code_unit, false, 0, 1);
    }
    out.into_bytes()
}

fn push_key_event_record(
    out: &mut String,
    virtual_key_code: u16,
    virtual_scan_code: u16,
    unicode_char: u16,
    key_down: bool,
    control_key_state: u32,
    repeat_count: u16,
) {
    let _ = write!(
        out,
        "\x1b[{virtual_key_code};{virtual_scan_code};{unicode_char};{};{control_key_state};{repeat_count}_",
        if key_down { 1 } else { 0 }
    );
}

#[cfg(test)]
mod tests {
    use super::{Win32InputModeTracker, encode_win32_input_text};

    #[test]
    fn tracker_detects_win32_input_mode_across_chunk_boundaries() {
        let mut tracker = Win32InputModeTracker::default();

        assert!(!tracker.observe(b"\x1b[?90"));
        assert!(tracker.observe(b"01h"));
        assert!(tracker.is_active());

        assert!(tracker.observe(b"\x1b[?900"));
        assert!(!tracker.observe(b"1l"));
        assert!(!tracker.is_active());
    }

    #[test]
    fn encode_win32_input_text_emits_key_records_for_each_utf16_unit() {
        let encoded = String::from_utf8(encode_win32_input_text("・"))
            .expect("encoded sequence should be utf8");

        assert_eq!(encoded, "\x1b[0;0;12539;1;0;1_\x1b[0;0;12539;0;0;1_");
    }
}
