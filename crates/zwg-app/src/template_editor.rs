use std::ops::Range;

use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::app::{
    byte_range_to_utf16_range, direct_text_from_input_keystroke,
    should_defer_control_key_to_input_method, should_defer_keystroke_to_input_method,
    should_route_keystroke_via_text_input, toggle_ime_via_imm, utf16_range_to_byte_range,
};
use crate::text_input::ImeTextBuffer;

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
    name: ImeTextBuffer,
    content: ImeTextBuffer,
    note: ImeTextBuffer,
    tags: ImeTextBuffer,
    favorite: bool,
}

impl Default for TemplateEditorDraft {
    fn default() -> Self {
        Self {
            name: ImeTextBuffer::default(),
            content: ImeTextBuffer::default(),
            note: ImeTextBuffer::default(),
            tags: ImeTextBuffer::default(),
            favorite: false,
        }
    }
}

impl TemplateEditorDraft {
    fn can_submit(&self) -> bool {
        !self.name.text().trim().is_empty() && !self.content.text().trim().is_empty()
    }

    fn field(&self, field: TemplateEditorField) -> &ImeTextBuffer {
        match field {
            TemplateEditorField::Name => &self.name,
            TemplateEditorField::Content => &self.content,
            TemplateEditorField::Note => &self.note,
            TemplateEditorField::Tags => &self.tags,
        }
    }

    fn field_mut(&mut self, field: TemplateEditorField) -> &mut ImeTextBuffer {
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
            .text()
            .split([',', '、'])
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(|tag| tag.to_string())
            .collect::<Vec<_>>();

        let note = self.note.text().trim();

        Some(TemplateEditorSubmission {
            name: self.name.text().trim().to_string(),
            content: self.content.text().trim_end().to_string(),
            note: (!note.is_empty()).then(|| note.to_string()),
            tags,
            favorite: self.favorite,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TemplateEditorSubmission {
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
    draft: TemplateEditorDraft,
    active_field: TemplateEditorField,
    pending_outcome: Option<TemplateEditorOutcome>,
}

impl TemplateEditorModal {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            draft: TemplateEditorDraft::default(),
            active_field: TemplateEditorField::Name,
            pending_outcome: None,
        }
    }

    pub(crate) fn focus(&self, window: &mut Window) {
        self.focus_handle.focus(window);
    }

    pub(crate) fn take_outcome(&mut self) -> Option<TemplateEditorOutcome> {
        self.pending_outcome.take()
    }

    fn request_cancel(&mut self, cx: &mut Context<Self>) {
        self.pending_outcome = Some(TemplateEditorOutcome::Cancelled);
        cx.notify();
    }

    fn request_submit(&mut self, cx: &mut Context<Self>) {
        if let Some(submission) = self.draft.submission() {
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
        self.active_field = field;
        self.focus(window);
        cx.notify();
    }

    fn cycle_field(&mut self, step: isize, cx: &mut Context<Self>) {
        self.active_field = self.active_field.next(step);
        cx.notify();
    }

    fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        if self
            .draft
            .field_mut(self.active_field)
            .replace_selection(text)
            .is_some()
        {
            cx.notify();
        }
    }

    fn move_cursor(&mut self, direction: isize, cx: &mut Context<Self>) {
        if self
            .draft
            .field_mut(self.active_field)
            .move_cursor_grapheme(direction)
        {
            cx.notify();
        }
    }

    fn backspace(&mut self, cx: &mut Context<Self>) {
        if self.draft.field_mut(self.active_field).backspace_grapheme() {
            cx.notify();
        }
    }

    fn delete_forward(&mut self, cx: &mut Context<Self>) {
        if self
            .draft
            .field_mut(self.active_field)
            .delete_forward_grapheme()
        {
            cx.notify();
        }
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
        if should_route_keystroke_via_text_input(&event.keystroke) {
            return false;
        }

        let ime_composing = self.draft.field(self.active_field).is_composing();
        if should_defer_control_key_to_input_method(&event.keystroke, ime_composing) {
            return true;
        }

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

        match event.keystroke.key.as_ref() {
            "escape" => {
                self.request_cancel(cx);
                return true;
            }
            "tab" => {
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
            "enter" if event.keystroke.modifiers.control && !event.keystroke.modifiers.alt => {
                self.request_submit(cx);
                return true;
            }
            "enter" => {
                if self.active_field == TemplateEditorField::Content {
                    self.insert_text("\n", cx);
                } else {
                    self.cycle_field(1, cx);
                }
                return true;
            }
            "backspace" => {
                self.backspace(cx);
                return true;
            }
            "delete" => {
                self.delete_forward(cx);
                return true;
            }
            "left" if !event.keystroke.modifiers.control && !event.keystroke.modifiers.alt => {
                self.move_cursor(-1, cx);
                return true;
            }
            "right" if !event.keystroke.modifiers.control && !event.keystroke.modifiers.alt => {
                self.move_cursor(1, cx);
                return true;
            }
            "home" => {
                if self
                    .draft
                    .field_mut(self.active_field)
                    .set_cursor_to_start()
                {
                    cx.notify();
                }
                return true;
            }
            "end" => {
                if self.draft.field_mut(self.active_field).set_cursor_to_end() {
                    cx.notify();
                }
                return true;
            }
            "v" if event.keystroke.modifiers.control && !event.keystroke.modifiers.alt => {
                let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
                    return true;
                };
                self.insert_text(&text, cx);
                return true;
            }
            "insert"
                if event.keystroke.modifiers.shift
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

        if let Some(text) = direct_text_from_input_keystroke(&event.keystroke) {
            self.insert_text(&text, cx);
            return true;
        }

        false
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.handle_key_down(event, window, cx) {
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
        let text = self.draft.field(self.active_field).text().to_string();
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
        let buffer = self.draft.field(self.active_field);
        Some(UTF16Selection {
            range: byte_range_to_utf16_range(buffer.text(), &buffer.selection()),
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let buffer = self.draft.field(self.active_field);
        buffer
            .marked_range()
            .as_ref()
            .map(|range| byte_range_to_utf16_range(buffer.text(), range))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.draft.field_mut(self.active_field).clear_marked_range() {
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
        let buffer = self.draft.field_mut(self.active_field);
        let current_text = buffer.text().to_string();
        let range = range_utf16
            .as_ref()
            .map(|range| utf16_range_to_byte_range(&current_text, range))
            .or_else(|| buffer.marked_range())
            .or_else(|| Some(buffer.selection()))
            .unwrap_or_else(|| current_text.len()..current_text.len());
        if let Some(inserted) = buffer.replace_range(range, text) {
            buffer.clear_marked_range();
            buffer.set_selection(inserted.end..inserted.end);
            cx.notify();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let buffer = self.draft.field_mut(self.active_field);
        let current_text = buffer.text().to_string();
        let range = range_utf16
            .as_ref()
            .map(|value| utf16_range_to_byte_range(&current_text, value))
            .or_else(|| buffer.marked_range())
            .or_else(|| Some(buffer.selection()))
            .unwrap_or_else(|| current_text.len()..current_text.len());
        if let Some(inserted) = buffer.replace_range(range, new_text) {
            let marked_range = if new_text.is_empty() {
                None
            } else {
                Some(inserted.clone())
            };
            let selection_range = new_selected_range_utf16
                .as_ref()
                .map(|value| utf16_range_to_byte_range(new_text, value))
                .map(|value| inserted.start + value.start..inserted.start + value.end)
                .unwrap_or_else(|| inserted.end..inserted.end);
            buffer.set_marked_range(marked_range);
            buffer.set_selection(selection_range);
            cx.notify();
        }
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(Bounds::new(
            point(
                element_bounds.left() + px(12.0),
                element_bounds.top() + px(8.0),
            ),
            size(px(2.0), px(20.0)),
        ))
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
                    .h(px(modal_height))
                    .rounded(px(18.0))
                    .overflow_hidden()
                    .border_1()
                    .border_color(rgba(0xffffff12))
                    .bg(rgb(UI_BG))
                    .shadow_lg()
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
                                    .child("新しい定型文を追加"),
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
                                self.draft.field(TemplateEditorField::Name),
                                "例: メールの署名",
                                self.active_field == TemplateEditorField::Name,
                                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                    this.focus_field(TemplateEditorField::Name, window, cx);
                                }),
                            ))
                            .child(template_editor_section_label("内容 *"))
                            .child(template_editor_text_area(
                                self.draft.field(TemplateEditorField::Content),
                                "定型文の内容を入力...",
                                self.active_field == TemplateEditorField::Content,
                                180.0,
                                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                    this.focus_field(TemplateEditorField::Content, window, cx);
                                }),
                            ))
                            .child(template_editor_section_label("説明（オプション）"))
                            .child(template_editor_input_box(
                                self.draft.field(TemplateEditorField::Note),
                                "この定型文の用途",
                                self.active_field == TemplateEditorField::Note,
                                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                    this.focus_field(TemplateEditorField::Note, window, cx);
                                }),
                            ))
                            .child(template_editor_section_label("タグ（オプション）"))
                            .child(template_editor_input_box(
                                self.draft.field(TemplateEditorField::Tags),
                                "タグをカンマ区切りで入力: 仕事,メール",
                                self.active_field == TemplateEditorField::Tags,
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
                                "追加",
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
    buffer: &ImeTextBuffer,
    placeholder: &'static str,
    active: bool,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let value = buffer.text();
    let is_empty = buffer.is_empty();
    let selection = buffer.selection();
    let cursor_byte = if active && selection.start == selection.end {
        Some(buffer.cursor())
    } else {
        None
    };
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

    if is_empty && !active {
        container = container.child(
            div()
                .font_family(UI_FONT)
                .text_size(px(13.0))
                .text_color(rgb(SUBTEXT1))
                .child(placeholder),
        );
    } else if is_empty && active {
        container = container
            .child(div().w(px(1.5)).h(px(18.0)).bg(rgb(ACCENT)).flex_shrink_0())
            .child(
                div()
                    .font_family(UI_FONT)
                    .text_size(px(13.0))
                    .text_color(rgb(SUBTEXT1))
                    .child(placeholder),
            );
    } else if let Some(cursor) = cursor_byte.filter(|_| active) {
        let cursor = cursor.min(value.len());
        let cursor = if cursor == 0 || cursor >= value.len() {
            cursor
        } else {
            value[..cursor]
                .char_indices()
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(cursor)
        };
        let before = &value[..cursor];
        let after = &value[cursor..];

        let mut row = div()
            .flex()
            .items_center()
            .font_family(UI_FONT)
            .text_size(px(13.0));

        if !before.is_empty() {
            row = row.child(div().text_color(rgb(TEXT)).child(before.to_string()));
        }
        row = row.child(div().w(px(1.5)).h(px(18.0)).bg(rgb(ACCENT)).flex_shrink_0());
        if !after.is_empty() {
            row = row.child(div().text_color(rgb(TEXT)).child(after.to_string()));
        }
        container = container.child(row);
    } else {
        container = container.child(
            div()
                .font_family(UI_FONT)
                .text_size(px(13.0))
                .text_color(rgb(TEXT))
                .child(value.to_string()),
        );
    }

    container
}

fn template_editor_text_area(
    buffer: &ImeTextBuffer,
    placeholder: &'static str,
    active: bool,
    height: f32,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let value = buffer.text();
    let lines = if buffer.is_empty() {
        vec![
            div()
                .font_family(UI_FONT)
                .text_size(px(13.0))
                .text_color(rgb(SUBTEXT1))
                .child(placeholder)
                .into_any_element(),
        ]
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
            .collect::<Vec<_>>()
    };

    let lines = if active && buffer.selection().start == buffer.selection().end {
        append_caret_to_last_line(lines)
    } else {
        lines
    };

    div()
        .w_full()
        .h(px(height))
        .rounded(px(12.0))
        .border_1()
        .border_color(if active {
            rgba(0x7AA2F7FF)
        } else {
            rgba(0xffffff10)
        })
        .bg(rgb(FIELD_BG))
        .p(px(16.0))
        .cursor_text()
        .hover(|style| style.bg(rgb(0x1A1A1C)))
        .on_mouse_down(MouseButton::Left, listener)
        .flex()
        .flex_col()
        .gap(px(6.0))
        .children(lines)
}

fn append_caret_to_last_line(mut lines: Vec<AnyElement>) -> Vec<AnyElement> {
    let caret = div()
        .w(px(1.5))
        .h(px(18.0))
        .bg(rgb(ACCENT))
        .flex_shrink_0()
        .into_any_element();

    if let Some(last_line) = lines.pop() {
        lines.push(
            div()
                .flex()
                .items_center()
                .gap(px(1.0))
                .child(last_line)
                .child(caret)
                .into_any_element(),
        );
        return lines;
    }

    vec![div().child(caret).into_any_element()]
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
    use super::{TemplateEditorDraft, TemplateEditorSubmission};
    use crate::text_input::ImeTextBuffer;

    #[test]
    fn template_editor_draft_builds_submission_with_trimmed_note_and_tags() {
        let draft = TemplateEditorDraft {
            name: ImeTextBuffer::new(" 署名 "),
            content: ImeTextBuffer::new("本文です\n"),
            note: ImeTextBuffer::new(" メール用 "),
            tags: ImeTextBuffer::new("仕事, メール、重要"),
            favorite: true,
        };

        assert_eq!(
            draft.submission(),
            Some(TemplateEditorSubmission {
                name: "署名".into(),
                content: "本文です".into(),
                note: Some("メール用".into()),
                tags: vec!["仕事".into(), "メール".into(), "重要".into()],
                favorite: true,
            })
        );
    }
}
