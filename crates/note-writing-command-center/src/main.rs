#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod codex_bridge;
mod model;

use codex_bridge::{AssistantAnswer, ask_assistant};
use eframe::egui::{
    self, CentralPanel, Color32, Context, FontId, RichText, ScrollArea, SidePanel, TextEdit,
    TopBottomPanel, Ui,
};
use model::{
    Article, ArticleStatus, AssistantTask, EditorMode, default_store_path, extract_outline,
    load_articles, markdown_to_note_html, new_article, preflight_checklist, save_articles,
    seed_articles, stamp_updated, status_counts, suggested_outline,
};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

fn main() -> eframe::Result {
    let viewport = egui::ViewportBuilder::default()
        .with_title("note執筆司令塔")
        .with_inner_size([1440.0, 920.0])
        .with_min_inner_size([1120.0, 740.0]);
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "note執筆司令塔",
        options,
        Box::new(|cc| Ok(Box::new(NoteCommandCenterApp::new(cc)))),
    )
}

struct NoteCommandCenterApp {
    articles: Vec<Article>,
    selected_id: String,
    status_filter: Option<ArticleStatus>,
    query: String,
    editor_mode: EditorMode,
    assistant_task: AssistantTask,
    assistant_text: String,
    assistant_source: &'static str,
    assistant_error: Option<String>,
    assistant_rx: Option<Receiver<AssistantAnswer>>,
    store_path: PathBuf,
    last_save_error: Option<String>,
}

impl NoteCommandCenterApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_style(&cc.egui_ctx);
        let store_path = default_store_path();
        let articles = load_articles(&store_path).unwrap_or_else(|_| seed_articles());
        let selected = articles.first().cloned().unwrap_or_else(new_article);
        Self {
            articles: if articles.is_empty() {
                vec![selected.clone()]
            } else {
                articles
            },
            selected_id: selected.id.clone(),
            status_filter: None,
            query: String::new(),
            editor_mode: EditorMode::Markdown,
            assistant_task: AssistantTask::Outline,
            assistant_text: model::local_assistant_response(AssistantTask::Outline, &selected),
            assistant_source: "local",
            assistant_error: None,
            assistant_rx: None,
            store_path,
            last_save_error: None,
        }
    }

    fn selected_index(&self) -> usize {
        self.articles
            .iter()
            .position(|article| article.id == self.selected_id)
            .unwrap_or(0)
    }

    fn selected_article(&self) -> &Article {
        &self.articles[self.selected_index()]
    }

    fn selected_article_mut(&mut self) -> &mut Article {
        let index = self.selected_index();
        &mut self.articles[index]
    }

    fn mark_changed(&mut self) {
        stamp_updated(self.selected_article_mut());
        if let Err(err) = save_articles(&self.store_path, &self.articles) {
            self.last_save_error = Some(format!("{err:#}"));
        } else {
            self.last_save_error = None;
        }
    }

    fn create_article(&mut self) {
        let article = new_article();
        self.selected_id = article.id.clone();
        self.articles.insert(0, article.clone());
        self.editor_mode = EditorMode::Markdown;
        self.assistant_task = AssistantTask::Outline;
        self.assistant_text = model::local_assistant_response(AssistantTask::Outline, &article);
        self.assistant_source = "local";
        let _ = save_articles(&self.store_path, &self.articles);
    }

    fn run_assistant(&mut self, task: AssistantTask) {
        let article = self.selected_article().clone();
        let (tx, rx) = mpsc::channel();
        self.assistant_task = task;
        self.assistant_text = "Codexに確認中です...".to_string();
        self.assistant_source = "pending";
        self.assistant_error = None;
        self.assistant_rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(ask_assistant(task, article));
        });
    }

    fn poll_assistant(&mut self) {
        let Some(rx) = &self.assistant_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(answer) => {
                self.assistant_text = answer.text;
                self.assistant_source = answer.source;
                self.assistant_error = answer.error;
                self.assistant_rx = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.assistant_text =
                    model::local_assistant_response(self.assistant_task, self.selected_article());
                self.assistant_source = "local";
                self.assistant_error = Some("Codex応答スレッドが終了しました。".to_string());
                self.assistant_rx = None;
            }
        }
    }
}

impl eframe::App for NoteCommandCenterApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.poll_assistant();

        TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("note執筆司令塔")
                        .font(FontId::proportional(21.0))
                        .strong(),
                );
                ui.separator();
                ui.label("Windowsネイティブ / Rust");
                if let Some(err) = &self.last_save_error {
                    ui.colored_label(Color32::from_rgb(180, 60, 48), format!("保存失敗: {err}"));
                }
            });
        });

        SidePanel::left("article_list")
            .exact_width(292.0)
            .resizable(false)
            .show(ctx, |ui| self.render_left_panel(ui));

        SidePanel::right("assistant")
            .exact_width(364.0)
            .resizable(false)
            .show(ctx, |ui| self.render_right_panel(ui));

        CentralPanel::default().show(ctx, |ui| self.render_editor(ui));
    }
}

impl NoteCommandCenterApp {
    fn render_left_panel(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.heading("記事一覧");
            if ui.button("+ 新規").clicked() {
                self.create_article();
            }
        });
        ui.add(TextEdit::singleline(&mut self.query).hint_text("タイトル、タグ、本文を検索"));
        ui.add_space(8.0);

        let counts = status_counts(&self.articles);
        ui.horizontal_wrapped(|ui| {
            let all_active = self.status_filter.is_none();
            if selectable_chip(ui, all_active, format!("すべて {}", self.articles.len())).clicked()
            {
                self.status_filter = None;
            }
            for status in ArticleStatus::ALL {
                let active = self.status_filter == Some(status);
                let label = format!(
                    "{} {}",
                    status.label(),
                    counts.get(&status).copied().unwrap_or(0)
                );
                if selectable_chip(ui, active, label).clicked() {
                    self.status_filter = Some(status);
                }
            }
        });
        ui.separator();

        let query = self.query.trim().to_lowercase();
        let visible = self
            .articles
            .iter()
            .enumerate()
            .filter(|(_, article)| {
                self.status_filter
                    .map_or(true, |status| status == article.status)
                    && (query.is_empty()
                        || format!(
                            "{}\n{}\n{}\n{}",
                            article.title,
                            article.idea_memo,
                            article.markdown,
                            article.tags.join(" ")
                        )
                        .to_lowercase()
                        .contains(&query))
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();

        ScrollArea::vertical().show(ui, |ui| {
            for index in visible {
                let article = &self.articles[index];
                let selected = article.id == self.selected_id;
                let button = egui::Button::new(
                    RichText::new(format!(
                        "{}\n{} / {}",
                        article.title,
                        article.status.label(),
                        article.tags.join(" / ")
                    ))
                    .size(13.0),
                )
                .selected(selected)
                .min_size([260.0, 54.0].into());
                if ui.add(button).clicked() {
                    self.selected_id = article.id.clone();
                    self.assistant_text =
                        model::local_assistant_response(self.assistant_task, article);
                    self.assistant_source = "local";
                    self.assistant_error = None;
                }
                ui.add_space(4.0);
            }
        });
    }

    fn render_editor(&mut self, ui: &mut Ui) {
        let selected_index = self.selected_index();
        ui.add_space(10.0);
        let mut changed = false;
        {
            let article = &mut self.articles[selected_index];
            ui.horizontal(|ui| {
                ui.label("状態");
                egui::ComboBox::from_id_salt("article_status")
                    .selected_text(article.status.label())
                    .show_ui(ui, |ui| {
                        for status in ArticleStatus::ALL {
                            changed |= ui
                                .selectable_value(&mut article.status, status, status.label())
                                .changed();
                        }
                    });
            });
            changed |= ui
                .add(TextEdit::singleline(&mut article.title).font(FontId::proportional(24.0)))
                .changed();
            ui.horizontal(|ui| {
                ui.label("タグ");
                let mut tags = article.tags.join(", ");
                if ui.add(TextEdit::singleline(&mut tags)).changed() {
                    article.tags = tags
                        .split([',', '、'])
                        .map(str::trim)
                        .filter(|tag| !tag.is_empty())
                        .map(ToString::to_string)
                        .collect();
                    changed = true;
                }
            });
            ui.columns(2, |columns| {
                columns[0].label("ネタメモ");
                changed |= columns[0]
                    .add(TextEdit::multiline(&mut article.idea_memo).desired_rows(4))
                    .changed();
                columns[1].label("サムネイル指示");
                changed |= columns[1]
                    .add(TextEdit::multiline(&mut article.thumbnail_brief).desired_rows(4))
                    .changed();
            });
        }

        ui.horizontal_wrapped(|ui| {
            for mode in EditorMode::ALL {
                if selectable_chip(ui, self.editor_mode == mode, mode.label()).clicked() {
                    self.editor_mode = mode;
                }
            }
            if ui.button("構成テンプレを追加").clicked() {
                let suggested = suggested_outline(self.selected_article());
                let article = self.selected_article_mut();
                if !article.markdown.ends_with("\n\n") {
                    article.markdown.push_str("\n\n");
                }
                article.markdown.push_str(&suggested);
                article.status = if article.status == ArticleStatus::Idea {
                    ArticleStatus::Draft
                } else {
                    article.status
                };
                changed = true;
            }
            if ui.button("HTMLをクリップボードへ").clicked() {
                ui.ctx()
                    .copy_text(markdown_to_note_html(&self.selected_article().markdown));
            }
        });

        if changed {
            self.mark_changed();
        }

        ui.separator();
        match self.editor_mode {
            EditorMode::Markdown => {
                let response = ui.add(
                    TextEdit::multiline(&mut self.selected_article_mut().markdown)
                        .font(FontId::monospace(14.0))
                        .desired_rows(28)
                        .lock_focus(true),
                );
                if response.changed() {
                    self.mark_changed();
                }
            }
            EditorMode::Preview => render_markdown_preview(ui, &self.selected_article().markdown),
            EditorMode::NoteHtml => {
                let mut html = markdown_to_note_html(&self.selected_article().markdown);
                ui.add(
                    TextEdit::multiline(&mut html)
                        .font(FontId::monospace(13.0))
                        .desired_rows(28)
                        .interactive(false),
                );
            }
        }
    }

    fn render_right_panel(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);
        ui.heading("Codexアシスタント");
        ui.label(format!("source: {}", self.assistant_source));
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            for task in AssistantTask::ALL {
                let enabled = self.assistant_rx.is_none();
                if ui
                    .add_enabled(
                        enabled,
                        egui::Button::new(task.label()).selected(self.assistant_task == task),
                    )
                    .clicked()
                {
                    self.run_assistant(task);
                }
            }
        });
        ui.separator();
        ui.label(RichText::new(self.assistant_task.label()).strong());
        ScrollArea::vertical().max_height(190.0).show(ui, |ui| {
            ui.add(
                TextEdit::multiline(&mut self.assistant_text)
                    .font(FontId::monospace(12.5))
                    .desired_rows(8)
                    .interactive(false),
            );
        });
        if let Some(err) = &self.assistant_error {
            ui.colored_label(Color32::from_rgb(170, 74, 44), err);
        }

        ui.separator();
        ui.label(RichText::new("見出し構成").strong());
        ScrollArea::vertical().max_height(135.0).show(ui, |ui| {
            let outline = extract_outline(&self.selected_article().markdown);
            if outline.is_empty() {
                ui.label("見出しがまだありません。");
            } else {
                for item in outline {
                    ui.label(item);
                }
            }
        });

        ui.separator();
        ui.label(RichText::new("投稿前チェック").strong());
        for item in preflight_checklist(self.selected_article()) {
            ui.horizontal(|ui| {
                let color = if item.passed {
                    Color32::from_rgb(40, 140, 88)
                } else {
                    Color32::from_rgb(175, 120, 32)
                };
                ui.colored_label(color, if item.passed { "OK" } else { "確認" });
                ui.vertical(|ui| {
                    ui.label(RichText::new(item.label).strong());
                    ui.small(item.detail);
                });
            });
        }

        ui.separator();
        ui.label(RichText::new("公開後の振り返り").strong());
        let changed = ui
            .add(TextEdit::multiline(&mut self.selected_article_mut().reflection).desired_rows(5))
            .changed();
        if changed {
            self.mark_changed();
        }
    }
}

fn render_markdown_preview(ui: &mut Ui, markdown: &str) {
    ScrollArea::vertical().show(ui, |ui| {
        for line in markdown.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                ui.add_space(8.0);
            } else if let Some(text) = trimmed.strip_prefix("# ") {
                ui.label(
                    RichText::new(text)
                        .font(FontId::proportional(27.0))
                        .strong(),
                );
            } else if let Some(text) = trimmed.strip_prefix("## ") {
                ui.add_space(10.0);
                ui.label(
                    RichText::new(text)
                        .font(FontId::proportional(20.0))
                        .strong(),
                );
            } else if let Some(text) = trimmed.strip_prefix("### ") {
                ui.label(
                    RichText::new(text)
                        .font(FontId::proportional(16.0))
                        .strong(),
                );
            } else if let Some(text) = trimmed.strip_prefix("- ") {
                ui.label(format!("• {text}"));
            } else {
                ui.label(trimmed);
            }
        }
    });
}

fn selectable_chip(ui: &mut Ui, selected: bool, text: impl Into<String>) -> egui::Response {
    ui.add(egui::Button::new(text.into()).selected(selected))
}

fn configure_style(ctx: &Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals.window_fill = Color32::from_rgb(248, 247, 244);
    style.visuals.panel_fill = Color32::from_rgb(250, 249, 247);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(47, 111, 159);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(235, 240, 242);
    ctx.set_style(style);
}
