use std::ops::Range;

use gpui::prelude::FluentBuilder;
use gpui::*;
#[cfg(test)]
use unicode_segmentation::UnicodeSegmentation;

use crate::app::{
    byte_index_to_utf16_offset, byte_range_to_utf16_range, direct_text_from_input_keystroke,
    should_defer_keystroke_to_input_method, should_route_keystroke_via_text_input,
    toggle_ime_via_imm, utf16_range_to_byte_range,
};

const TEXT: u32 = 0xF5F5F7;
const SUBTEXT1: u32 = 0x8E8E93;
const ACCENT: u32 = 0x0A84FF;
const UI_BG: u32 = 0x2B2B2D;
const FIELD_BG: u32 = 0x171719;
const UI_FONT: &str = crate::config::DEFAULT_UI_FONT_FAMILY;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TemplateEditorField {
    Name,
    Content,
    Note,
    Tags,
}

impl TemplateEditorField {
    fn all() -> [Self; 4] {
        [Self::Name, Self::Content, Self::Note, Self::Tags]
    }

    fn next(self, step: isize) -> Self {
        let fields = Self::all();
        let current_index = fields.iter().position(|field| *field == self).unwrap_or(0);
        let next_index = (current_index as isize + step).rem_euclid(fields.len() as isize) as usize;
        fields[next_index]
    }
}

#[derive(Clone, Debug)]
struct TemplateEditorDraft {
    name: String,
    content: String,
    note: String,
    tags: String,
    favorite: bool,
}

impl Default for TemplateEditorDraft {
    fn default() -> Self {
        Self {
            name: String::new(),
            content: String::new(),
            note: String::new(),
            tags: String::new(),
            favorite: false,
        }
    }
}

impl TemplateEditorDraft {
    fn can_submit(&self) -> bool {
        !self.name.trim().is_empty() && !self.content.trim().is_empty()
    }

    fn field(&self, field: TemplateEditorField) -> &str {
        match field {
            TemplateEditorField::Name => self.name.as_str(),
            TemplateEditorField::Content => self.content.as_str(),
            TemplateEditorField::Note => self.note.as_str(),
            TemplateEditorField::Tags => self.tags.as_str(),
        }
    }

    fn field_mut(&mut self, field: TemplateEditorField) -> &mut String {
        match field {
            TemplateEditorField::Name => &mut self.name,
            TemplateEditorField::Content => &mut self.content,
            TemplateEditorField::Note => &mut self.note,
            TemplateEditorField::Tags => &mut self.tags,
        }
    }

    fn submission(&self) -> Option<TemplateEditorSubmission> {
        if !self.can_submit() {
            return None;
        }

        let tags = self
            .tags
            .split([',', '、'])
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(|tag| tag.to_string())
            .collect::<Vec<_>>();

        let note = self.note.trim();

        Some(TemplateEditorSubmission {
            editing_id: None, // set by request_submit
            name: self.name.trim().to_string(),
            content: self.content.trim_end().to_string(),
            note: (!note.is_empty()).then(|| note.to_string()),
            tags,
            favorite: self.favorite,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TemplateEditorSubmission {
    pub(crate) editing_id: Option<String>,
    pub(crate) name: String,
    pub(crate) content: String,
    pub(crate) note: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) favorite: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TemplateEditorOutcome {
    Cancelled,
    Submitted(TemplateEditorSubmission),
}

pub(crate) struct TemplateEditorModal {
    focus_handle: FocusHandle,
    editing_id: Option<String>,
    draft: TemplateEditorDraft,
    active_field: TemplateEditorField,
    cursor: usize, // byte offset into active field
    preedit_text: String,
    marked_range: Option<Range<usize>>,
    pending_outcome: Option<TemplateEditorOutcome>,
}

impl TemplateEditorModal {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            editing_id: None,
            draft: TemplateEditorDraft::default(),
            active_field: TemplateEditorField::Name,
            cursor: 0,
            preedit_text: String::new(),
            marked_range: None,
            pending_outcome: None,
        }
    }

    pub(crate) fn new_prefilled(
        name: String,
        content: String,
        note: String,
        tags: String,
        favorite: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let cursor = name.len();
        Self {
            focus_handle: cx.focus_handle(),
            editing_id: None,
            draft: TemplateEditorDraft {
                name,
                content,
                note,
                tags,
                favorite,
            },
            active_field: TemplateEditorField::Name,
            cursor,
            preedit_text: String::new(),
            marked_range: None,
            pending_outcome: None,
        }
    }

    pub(crate) fn new_edit(
        id: String,
        name: String,
        content: String,
        note: String,
        tags: String,
        favorite: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let cursor = name.len();
        Self {
            focus_handle: cx.focus_handle(),
            editing_id: Some(id),
            draft: TemplateEditorDraft {
                name,
                content,
                note,
                tags,
                favorite,
            },
            active_field: TemplateEditorField::Name,
            cursor,
            preedit_text: String::new(),
            marked_range: None,
            pending_outcome: None,
        }
    }

    pub(crate) fn focus(&self, window: &mut Window) {
        self.focus_handle.focus(window);
    }

    pub(crate) fn take_outcome(&mut self) -> Option<TemplateEditorOutcome> {
        self.pending_outcome.take()
    }

    fn is_composing(&self) -> bool {
        self.marked_range.is_some() || !self.preedit_text.is_empty()
    }

    fn clear_preedit(&mut self) {
        self.preedit_text.clear();
        self.marked_range = None;
    }

    fn active_text(&self) -> &str {
        self.draft.field(self.active_field)
    }

    fn display_text_for(&self, field: TemplateEditorField) -> String {
        let text = self.draft.field(field).to_string();
        if self.active_field == field && !self.preedit_text.is_empty() {
            let insert_pos = self.cursor.min(text.len());
            // Snap to char boundary
            let insert_pos = if insert_pos == 0 || text.is_char_boundary(insert_pos) {
                insert_pos
            } else {
                text[..insert_pos]
                    .char_indices()
                    .last()
                    .map(|(i, c)| i + c.len_utf8())
                    .unwrap_or(0)
            };
            let mut result = String::with_capacity(text.len() + self.preedit_text.len());
            result.push_str(&text[..insert_pos]);
            result.push_str(&self.preedit_text);
            result.push_str(&text[insert_pos..]);
            result
        } else {
            text
        }
    }

    fn request_cancel(&mut self, cx: &mut Context<Self>) {
        self.clear_preedit();
        self.pending_outcome = Some(TemplateEditorOutcome::Cancelled);
        cx.notify();
    }

    fn request_submit(&mut self, cx: &mut Context<Self>) {
        if let Some(mut submission) = self.draft.submission() {
            submission.editing_id = self.editing_id.clone();
            self.pending_outcome = Some(TemplateEditorOutcome::Submitted(submission));
            cx.notify();
        }
    }

    fn focus_field(
        &mut self,
        field: TemplateEditorField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_field != field {
            self.clear_preedit();
            self.active_field = field;
            self.cursor = self.active_text().len();
        }
        self.focus(window);
        cx.notify();
    }

    fn cycle_field(&mut self, step: isize, cx: &mut Context<Self>) {
        self.clear_preedit();
        self.active_field = self.active_field.next(step);
        self.cursor = self.active_text().len(); // cursor at end of new field
        cx.notify();
    }

    /// Clamp cursor to valid char-boundary position within active field.
    fn clamp_cursor(&mut self) {
        let len = self.draft.field(self.active_field).len();
        if self.cursor > len {
            self.cursor = len;
        } else if self.cursor > 0
            && !self
                .draft
                .field(self.active_field)
                .is_char_boundary(self.cursor)
        {
            // Snap backward to the nearest char boundary
            let snapped = self.draft.field(self.active_field)[..self.cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.cursor = snapped;
        }
    }

    fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        self.clear_preedit();
        self.clamp_cursor();
        let cur = self.cursor;
        self.draft
            .field_mut(self.active_field)
            .insert_str(cur, text);
        self.cursor = cur + text.len();
        cx.notify();
    }

    fn backspace(&mut self, cx: &mut Context<Self>) {
        self.clamp_cursor();
        if self.cursor == 0 {
            return;
        }
        let cur = self.cursor;
        let prev = {
            let t = self.draft.field(self.active_field);
            t[..cur]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0)
        };
        self.draft.field_mut(self.active_field).drain(prev..cur);
        self.cursor = prev;
        cx.notify();
    }

    fn delete_forward(&mut self, cx: &mut Context<Self>) {
        self.clamp_cursor();
        let cur = self.cursor;
        let next = {
            let t = self.draft.field(self.active_field);
            if cur >= t.len() {
                return;
            }
            t[cur..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| cur + i)
                .unwrap_or(t.len())
        };
        self.draft.field_mut(self.active_field).drain(cur..next);
        cx.notify();
    }

    fn move_cursor(&mut self, direction: isize, cx: &mut Context<Self>) {
        self.clamp_cursor();
        let cur = self.cursor;
        let new_pos = {
            let t = self.draft.field(self.active_field);
            if direction < 0 {
                t[..cur]
                    .char_indices()
                    .next_back()
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            } else {
                t[cur..]
                    .char_indices()
                    .nth(1)
                    .map(|(i, _)| cur + i)
                    .unwrap_or(t.len())
            }
        };
        self.cursor = new_pos;
        cx.notify();
    }

    fn move_cursor_home(&mut self, cx: &mut Context<Self>) {
        self.cursor = 0;
        cx.notify();
    }

    fn move_cursor_end(&mut self, cx: &mut Context<Self>) {
        self.cursor = self.active_text().len();
        cx.notify();
    }

    fn toggle_favorite(&mut self, cx: &mut Context<Self>) {
        self.draft.favorite = !self.draft.favorite;
        cx.notify();
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.keystroke.modifiers.control
            && !event.keystroke.modifiers.alt
            && event.keystroke.key == "space"
        {
            toggle_ime_via_imm();
            cx.notify();
            return true;
        }

        if should_defer_keystroke_to_input_method(&event.keystroke) {
            cx.notify();
            return true;
        }

        if should_route_keystroke_via_text_input(&event.keystroke) {
            return false;
        }

        let composing = self.is_composing();

        match event.keystroke.key.as_ref() {
            "escape" => {
                if composing {
                    self.clear_preedit();
                    cx.notify();
                } else {
                    self.request_cancel(cx);
                }
                return true;
            }
            "tab" if !composing => {
                self.cycle_field(
                    if event.keystroke.modifiers.shift {
                        -1
                    } else {
                        1
                    },
                    cx,
                );
                return true;
            }
            "enter"
                if !composing
                    && event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.alt =>
            {
                self.request_submit(cx);
                return true;
            }
            "enter" if !composing => {
                if self.active_field == TemplateEditorField::Content {
                    self.insert_text("\n", cx);
                } else {
                    self.cycle_field(1, cx);
                }
                return true;
            }
            "backspace" if !composing => {
                self.backspace(cx);
                return true;
            }
            "delete" if !composing => {
                self.delete_forward(cx);
                return true;
            }
            "left" if !composing => {
                self.move_cursor(-1, cx);
                return true;
            }
            "right" if !composing => {
                self.move_cursor(1, cx);
                return true;
            }
            "home" if !composing => {
                self.move_cursor_home(cx);
                return true;
            }
            "end" if !composing => {
                self.move_cursor_end(cx);
                return true;
            }
            "v" if !composing
                && event.keystroke.modifiers.control
                && !event.keystroke.modifiers.alt =>
            {
                let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
                    return true;
                };
                self.insert_text(&text, cx);
                return true;
            }
            "insert"
                if !composing
                    && event.keystroke.modifiers.shift
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.alt =>
            {
                let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
                    return true;
                };
                self.insert_text(&text, cx);
                return true;
            }
            _ => {}
        }

        if !composing
            && !event.keystroke.modifiers.control
            && !event.keystroke.modifiers.alt
            && !event.keystroke.modifiers.platform
        {
            if let Some(text) = direct_text_from_input_keystroke(&event.keystroke) {
                self.insert_text(&text, cx);
                return true;
            }
        }

        false
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let handled = self.handle_key_down(event, window, cx);
        if handled {
            cx.stop_propagation();
        }
    }
}

impl Focusable for TemplateEditorModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for TemplateEditorModal {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let text = self.active_text().to_string();
        let range = utf16_range_to_byte_range(&text, &range_utf16);
        adjusted_range.replace(byte_range_to_utf16_range(&text, &range));
        Some(text[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        self.clamp_cursor();
        let text = self.active_text();
        let cursor_utf16 = byte_index_to_utf16_offset(text, self.cursor);
        Some(UTF16Selection {
            range: cursor_utf16..cursor_utf16,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range.clone()
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.is_composing() {
            self.clear_preedit();
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let had_composition = self.is_composing();
        self.clear_preedit();
        self.clamp_cursor();

        if let Some(range) = range_utf16 {
            let field_text = self.active_text().to_string();
            let byte_range = utf16_range_to_byte_range(&field_text, &range);
            let start = byte_range.start.min(field_text.len());
            let end = byte_range.end.min(field_text.len());
            // Snap to char boundaries
            let start = if start == 0 || field_text.is_char_boundary(start) {
                start
            } else {
                field_text[..start]
                    .char_indices()
                    .last()
                    .map(|(i, c)| i + c.len_utf8())
                    .unwrap_or(0)
            };
            let end = if end == 0 || field_text.is_char_boundary(end) {
                end
            } else {
                field_text[..end]
                    .char_indices()
                    .last()
                    .map(|(i, c)| i + c.len_utf8())
                    .unwrap_or(0)
            };
            let field = self.draft.field_mut(self.active_field);
            field.drain(start..end);
            field.insert_str(start, text);
            self.cursor = start + text.len();
            cx.notify();
            return;
        }

        if text.is_empty() && had_composition {
            cx.notify();
            return;
        }

        if text.is_empty() {
            self.backspace(cx);
            return;
        }

        // No range specified — insert at cursor position
        let cur = self.cursor.min(self.active_text().len());
        let cur = if cur == 0 || self.active_text().is_char_boundary(cur) {
            cur
        } else {
            self.active_text()[..cur]
                .char_indices()
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(0)
        };
        self.draft
            .field_mut(self.active_field)
            .insert_str(cur, text);
        self.cursor = cur + text.len();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clamp_cursor();
        self.preedit_text = new_text.to_string();
        let cursor_utf16 = byte_index_to_utf16_offset(self.active_text(), self.cursor);
        let preedit_utf16_len = new_text.encode_utf16().count();
        self.marked_range = if preedit_utf16_len == 0 {
            None
        } else {
            Some(cursor_utf16..cursor_utf16 + preedit_utf16_len)
        };
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for TemplateEditorModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // gpui steals focus when mouse events are processed on parent elements.
        // Re-apply focus every render to ensure on_key_down always fires.
        if !self.focus_handle.is_focused(_window) {
            self.focus_handle.focus(_window);
        }
        let modal_width = 654.0;
        let modal_height = 720.0;
        let entity = cx.entity();
        let can_submit = self.draft.can_submit();

        div()
            .id("template-editor-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .bg(rgba(0x00000066))
            .flex()
            .justify_center()
            .items_start()
            .pt(px(20.0))
            .child(
                div()
                    .id("template-editor-modal")
                    .track_focus(&self.focus_handle)
                    .on_key_down(cx.listener(Self::on_key_down))
                    .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.request_cancel(cx);
                    }))
                    .relative()
                    .w(px(modal_width))
                    .max_h(px(modal_height))
                    .rounded(px(18.0))
                    .overflow_hidden()
                    .border_1()
                    .border_color(rgba(0xffffff12))
                    .bg(rgb(UI_BG))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .child(template_editor_input_overlay(
                        entity.clone(),
                        self.focus_handle.clone(),
                    ))
                    .child(
                        div()
                            .h(px(60.0))
                            .px(px(24.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .font_family(UI_FONT)
                                    .text_size(px(18.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(TEXT))
                                    .child(if self.editing_id.is_some() {
                                        "定型文を編集"
                                    } else {
                                        "新しい定型文を追加"
                                    }),
                            )
                            .child(
                                div()
                                    .w(px(28.0))
                                    .h(px(28.0))
                                    .rounded(px(8.0))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgba(0xffffff10)))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .font_family(UI_FONT)
                                    .text_size(px(16.0))
                                    .text_color(rgb(SUBTEXT1))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                            this.request_cancel(cx);
                                        }),
                                    )
                                    .child("x"),
                            ),
                    )
                    .child(
                        div()
                            .id("template-editor-scroll")
                            .flex_1()
                            .min_h(px(0.0))
                            .overflow_scroll()
                            .scrollbar_width(px(6.0))
                            .px(px(24.0))
                            .pb(px(16.0))
                            .flex()
                            .flex_col()
                            .gap(px(18.0))
                            .child(template_editor_section_label("名前 *"))
                            .child(template_editor_input_box(
                                self.display_text_for(TemplateEditorField::Name),
                                "例: メールの署名",
                                self.active_field == TemplateEditorField::Name,
                                if self.active_field == TemplateEditorField::Name {
                                    Some(self.cursor)
                                } else {
                                    None
                                },
                                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                    this.focus_field(TemplateEditorField::Name, window, cx);
                                }),
                            ))
                            .child(template_editor_section_label("内容 *"))
                            .child(template_editor_text_area(
                                self.display_text_for(TemplateEditorField::Content),
                                "定型文の内容を入力...",
                                self.active_field == TemplateEditorField::Content,
                                180.0,
                                if self.active_field == TemplateEditorField::Content {
                                    Some(self.cursor)
                                } else {
                                    None
                                },
                                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                    this.focus_field(TemplateEditorField::Content, window, cx);
                                }),
                            ))
                            .child(template_editor_section_label("説明（オプション）"))
                            .child(template_editor_input_box(
                                self.display_text_for(TemplateEditorField::Note),
                                "この定型文の用途",
                                self.active_field == TemplateEditorField::Note,
                                if self.active_field == TemplateEditorField::Note {
                                    Some(self.cursor)
                                } else {
                                    None
                                },
                                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                    this.focus_field(TemplateEditorField::Note, window, cx);
                                }),
                            ))
                            .child(template_editor_section_label("タグ（オプション）"))
                            .child(template_editor_input_box(
                                self.display_text_for(TemplateEditorField::Tags),
                                "タグをカンマ区切りで入力: 仕事,メール",
                                self.active_field == TemplateEditorField::Tags,
                                if self.active_field == TemplateEditorField::Tags {
                                    Some(self.cursor)
                                } else {
                                    None
                                },
                                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                    this.focus_field(TemplateEditorField::Tags, window, cx);
                                }),
                            ))
                            .child(template_editor_favorite_button(
                                self.draft.favorite,
                                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                    this.toggle_favorite(cx);
                                }),
                            )),
                    )
                    .child(
                        div()
                            .h(px(84.0))
                            .px(px(24.0))
                            .pb(px(18.0))
                            .flex()
                            .items_end()
                            .justify_end()
                            .gap(px(14.0))
                            .child(template_editor_footer_button(
                                "キャンセル",
                                false,
                                true,
                                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                    this.request_cancel(cx);
                                }),
                            ))
                            .child(template_editor_footer_button(
                                if self.editing_id.is_some() {
                                    "保存"
                                } else {
                                    "追加"
                                },
                                true,
                                can_submit,
                                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                    this.request_submit(cx);
                                }),
                            )),
                    ),
            )
    }
}

fn template_editor_input_overlay(
    entity: Entity<TemplateEditorModal>,
    focus_handle: FocusHandle,
) -> AnyElement {
    canvas(
        |_bounds, _window, _cx| {},
        move |bounds, _, window, cx| {
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, entity.clone()),
                cx,
            );
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

fn template_editor_section_label(label: &'static str) -> Div {
    div()
        .font_family(UI_FONT)
        .text_size(px(13.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(TEXT))
        .child(label)
}

fn template_editor_input_box(
    value: String,
    placeholder: &'static str,
    active: bool,
    cursor_byte: Option<usize>,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let is_empty = value.is_empty();
    let caret = div().w(px(1.5)).h(px(18.0)).bg(rgb(ACCENT)).flex_shrink_0();
    let mut container = div()
        .w_full()
        .h(px(52.0))
        .rounded(px(12.0))
        .border_1()
        .border_color(if active {
            rgba(0x7AA2F7FF)
        } else {
            rgba(0xffffff10)
        })
        .bg(rgb(FIELD_BG))
        .px(px(16.0))
        .cursor_text()
        .hover(|style| style.bg(rgb(0x1A1A1C)))
        .on_mouse_down(MouseButton::Left, listener)
        .flex()
        .items_center()
        .overflow_hidden();

    if !active {
        // Inactive: show text or placeholder
        container = container.child(
            div()
                .font_family(UI_FONT)
                .text_size(px(13.0))
                .text_color(rgb(if is_empty { SUBTEXT1 } else { TEXT }))
                .child(if is_empty {
                    placeholder.to_string()
                } else {
                    value
                }),
        );
    } else if is_empty {
        // Active + empty: caret then placeholder
        container = container.child(
            div()
                .flex()
                .items_center()
                .font_family(UI_FONT)
                .text_size(px(13.0))
                .child(caret)
                .child(div().text_color(rgb(SUBTEXT1)).child(placeholder)),
        );
    } else if let Some(pos) = cursor_byte {
        // Active + text + cursor position
        let pos = pos.min(value.len());
        // Snap to char boundary
        let pos = value[..pos]
            .char_indices()
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0)
            .min(value.len());
        let before = &value[..pos];
        let after = &value[pos..];
        let mut row = div()
            .flex()
            .items_center()
            .font_family(UI_FONT)
            .text_size(px(13.0));
        if !before.is_empty() {
            row = row.child(div().text_color(rgb(TEXT)).child(before.to_string()));
        }
        row = row.child(caret);
        if !after.is_empty() {
            row = row.child(div().text_color(rgb(TEXT)).child(after.to_string()));
        }
        container = container.child(row);
    } else {
        // Active + text, no cursor info: caret at end
        container = container.child(
            div()
                .flex()
                .items_center()
                .font_family(UI_FONT)
                .text_size(px(13.0))
                .child(div().text_color(rgb(TEXT)).child(value))
                .child(caret),
        );
    }

    container
}

fn template_editor_text_area(
    value: String,
    placeholder: &'static str,
    active: bool,
    height: f32,
    cursor_byte: Option<usize>,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let caret = || div().w(px(1.5)).h(px(18.0)).bg(rgb(ACCENT)).flex_shrink_0();

    let lines: Vec<AnyElement> = if value.is_empty() && !active {
        vec![
            div()
                .font_family(UI_FONT)
                .text_size(px(13.0))
                .text_color(rgb(SUBTEXT1))
                .child(placeholder)
                .into_any_element(),
        ]
    } else if value.is_empty() && active {
        vec![
            div()
                .flex()
                .items_center()
                .font_family(UI_FONT)
                .text_size(px(13.0))
                .child(caret())
                .child(div().text_color(rgb(SUBTEXT1)).child(placeholder))
                .into_any_element(),
        ]
    } else if active {
        let cursor_pos = cursor_byte
            .map(|c| c.min(value.len()))
            .unwrap_or(value.len());
        // Snap to char boundary
        let cursor_pos = if cursor_pos == 0 {
            0
        } else {
            value[..cursor_pos]
                .char_indices()
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(0)
                .min(value.len())
        };
        // Build lines with cursor inserted at the right position
        let before = &value[..cursor_pos];
        let after = &value[cursor_pos..];
        let mut result: Vec<AnyElement> = Vec::new();

        // Lines before cursor
        let before_lines: Vec<&str> = before.split('\n').collect();
        let after_lines: Vec<&str> = after.split('\n').collect();

        // All lines before the cursor line
        for line in &before_lines[..before_lines.len().saturating_sub(1)] {
            result.push(
                div()
                    .font_family(UI_FONT)
                    .text_size(px(13.0))
                    .text_color(rgb(TEXT))
                    .child(if line.is_empty() {
                        " ".to_string()
                    } else {
                        line.to_string()
                    })
                    .into_any_element(),
            );
        }

        // Cursor line: [last_before_part] [caret] [first_after_part]
        let cursor_line_before = before_lines.last().unwrap_or(&"");
        let cursor_line_after = after_lines.first().unwrap_or(&"");
        let mut cursor_row = div()
            .flex()
            .items_center()
            .font_family(UI_FONT)
            .text_size(px(13.0));
        if !cursor_line_before.is_empty() {
            cursor_row = cursor_row.child(
                div()
                    .text_color(rgb(TEXT))
                    .child(cursor_line_before.to_string()),
            );
        }
        cursor_row = cursor_row.child(caret());
        if !cursor_line_after.is_empty() {
            cursor_row = cursor_row.child(
                div()
                    .text_color(rgb(TEXT))
                    .child(cursor_line_after.to_string()),
            );
        }
        result.push(cursor_row.into_any_element());

        // All lines after the cursor line
        for line in &after_lines[1..] {
            result.push(
                div()
                    .font_family(UI_FONT)
                    .text_size(px(13.0))
                    .text_color(rgb(TEXT))
                    .child(if line.is_empty() {
                        " ".to_string()
                    } else {
                        line.to_string()
                    })
                    .into_any_element(),
            );
        }
        result
    } else {
        value
            .lines()
            .map(|line| {
                div()
                    .font_family(UI_FONT)
                    .text_size(px(13.0))
                    .text_color(rgb(TEXT))
                    .child(if line.is_empty() {
                        " ".to_string()
                    } else {
                        line.to_string()
                    })
                    .into_any_element()
            })
            .collect()
    };

    div()
        .w_full()
        .min_h(px(height))
        .max_h(px(320.0))
        .rounded(px(12.0))
        .border_1()
        .border_color(if active {
            rgba(0x7AA2F7FF)
        } else {
            rgba(0xffffff10)
        })
        .bg(rgb(FIELD_BG))
        .cursor_text()
        .hover(|style| style.bg(rgb(0x1A1A1C)))
        .on_mouse_down(MouseButton::Left, listener)
        .overflow_hidden()
        .child(
            div()
                .id("template-content-scroll")
                .w_full()
                .h_full()
                .overflow_y_scroll()
                .scrollbar_width(px(4.0))
                .p(px(16.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .children(lines),
        )
}

#[cfg(test)]
fn pop_last_grapheme(text: &mut String) -> bool {
    let Some((index, _)) = UnicodeSegmentation::grapheme_indices(text.as_str(), true).next_back()
    else {
        return false;
    };
    text.truncate(index);
    true
}

fn template_editor_favorite_button(
    favorite: bool,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .h(px(40.0))
        .px(px(16.0))
        .rounded(px(12.0))
        .cursor_pointer()
        .border_1()
        .border_color(if favorite {
            rgba(0x7AA2F7FF)
        } else {
            rgba(0xffffff10)
        })
        .bg(if favorite {
            rgba(0x0A84FF22)
        } else {
            rgba(0xffffff10)
        })
        .flex()
        .items_center()
        .gap(px(10.0))
        .on_mouse_down(MouseButton::Left, listener)
        .child(
            svg()
                .path(if favorite {
                    "ui/star-filled.svg"
                } else {
                    "ui/star.svg"
                })
                .size(px(18.0))
                .text_color(if favorite { rgb(ACCENT) } else { rgb(SUBTEXT1) }),
        )
        .child(
            div()
                .font_family(UI_FONT)
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(TEXT))
                .child("お気に入り"),
        )
}

fn template_editor_footer_button(
    label: &'static str,
    primary: bool,
    enabled: bool,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let base_bg = if primary {
        if enabled {
            rgba(0x0A84FFFF)
        } else {
            rgba(0x0A84FF66)
        }
    } else {
        rgba(0xffffff10)
    };

    let hover_bg = if primary {
        rgba(0x2A93FFFF)
    } else {
        rgba(0xffffff14)
    };

    div()
        .w(px(102.0))
        .h(px(44.0))
        .rounded(px(12.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .bg(base_bg)
        .when(enabled, |button| {
            button.hover(move |style| style.bg(hover_bg))
        })
        .when(enabled, |button| {
            button.on_mouse_down(MouseButton::Left, listener)
        })
        .child(
            div()
                .font_family(UI_FONT)
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if primary && enabled {
                    rgb(0xFFFFFF)
                } else if primary {
                    rgba(0xffffffb0)
                } else {
                    rgb(TEXT)
                })
                .child(label),
        )
}

#[cfg(test)]
mod tests {
    use super::pop_last_grapheme;
    use super::{TemplateEditorDraft, TemplateEditorSubmission};

    #[test]
    fn template_editor_draft_builds_submission_with_trimmed_note_and_tags() {
        let draft = TemplateEditorDraft {
            name: " 署名 ".into(),
            content: "本文です\n".into(),
            note: " メール用 ".into(),
            tags: "仕事, メール、重要".into(),
            favorite: true,
        };

        assert_eq!(
            draft.submission(),
            Some(TemplateEditorSubmission {
                editing_id: None,
                name: "署名".into(),
                content: "本文です".into(),
                note: Some("メール用".into()),
                tags: vec!["仕事".into(), "メール".into(), "重要".into()],
                favorite: true,
            })
        );
    }

    #[test]
    fn pop_last_grapheme_removes_single_emoji_cluster() {
        let mut text = "A👍🏽B".to_string();

        assert!(pop_last_grapheme(&mut text));
        assert_eq!(text, "A👍🏽");
        assert!(pop_last_grapheme(&mut text));
        assert_eq!(text, "A");
    }
}
