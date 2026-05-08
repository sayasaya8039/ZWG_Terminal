use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const STORE_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ArticleStatus {
    Idea,
    Draft,
    Revising,
    Ready,
    Published,
}

impl ArticleStatus {
    pub const ALL: [Self; 5] = [
        Self::Idea,
        Self::Draft,
        Self::Revising,
        Self::Ready,
        Self::Published,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Idea => "ネタメモ",
            Self::Draft => "下書き",
            Self::Revising => "推敲中",
            Self::Ready => "投稿待ち",
            Self::Published => "公開済み",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorMode {
    Markdown,
    Preview,
    NoteHtml,
}

impl EditorMode {
    pub const ALL: [Self; 3] = [Self::Markdown, Self::Preview, Self::NoteHtml];

    pub fn label(self) -> &'static str {
        match self {
            Self::Markdown => "Markdown",
            Self::Preview => "プレビュー",
            Self::NoteHtml => "note貼り付け",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssistantTask {
    Outline,
    Titles,
    Readability,
    Thumbnail,
    Preflight,
    Format,
}

impl AssistantTask {
    pub const ALL: [Self; 6] = [
        Self::Outline,
        Self::Titles,
        Self::Readability,
        Self::Thumbnail,
        Self::Preflight,
        Self::Format,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Outline => "構成提案",
            Self::Titles => "タイトル案",
            Self::Readability => "読みやすさ",
            Self::Thumbnail => "サムネ指示",
            Self::Preflight => "投稿前チェック",
            Self::Format => "note向け整形",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Article {
    pub id: String,
    pub title: String,
    pub status: ArticleStatus,
    pub idea_memo: String,
    pub markdown: String,
    pub thumbnail_brief: String,
    pub reflection: String,
    pub tags: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub published_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChecklistItem {
    pub label: &'static str,
    pub passed: bool,
    pub detail: &'static str,
}

#[derive(Debug, Serialize, Deserialize)]
struct ArticleStore {
    version: u32,
    articles: Vec<Article>,
}

pub fn default_store_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("ZWG")
        .join("note-writing-command-center")
        .join("articles.json")
}

pub fn load_articles(path: &PathBuf) -> anyhow::Result<Vec<Article>> {
    match fs::read_to_string(path) {
        Ok(raw) => {
            let store = serde_json::from_str::<ArticleStore>(&raw)?;
            if store.version != STORE_VERSION {
                return Ok(seed_articles());
            }
            let articles = store
                .articles
                .into_iter()
                .map(sanitize_article)
                .collect::<Vec<_>>();
            Ok(if articles.is_empty() {
                seed_articles()
            } else {
                articles
            })
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let articles = seed_articles();
            save_articles(path, &articles)?;
            Ok(articles)
        }
        Err(err) => Err(err.into()),
    }
}

pub fn save_articles(path: &PathBuf, articles: &[Article]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let store = ArticleStore {
        version: STORE_VERSION,
        articles: articles.iter().cloned().map(sanitize_article).collect(),
    };
    fs::write(path, format!("{}\n", serde_json::to_string_pretty(&store)?))?;
    Ok(())
}

pub fn new_article() -> Article {
    let now = timestamp();
    Article {
        id: format!("note-{}", Uuid::new_v4().simple()),
        title: "新しいnote記事".to_string(),
        status: ArticleStatus::Idea,
        idea_memo: "読者の悩み、体験、結論をここにメモする。".to_string(),
        markdown:
            "# 新しいnote記事\n\n## 読者の前提\n\n## 伝えたい結論\n\n## 具体例\n\n## まとめ\n"
                .to_string(),
        thumbnail_brief: "テーマ、読後感、入れたい文字、避けたい表現を整理する。".to_string(),
        reflection: String::new(),
        tags: vec!["note".to_string(), "執筆".to_string()],
        created_at: now.clone(),
        updated_at: now,
        published_at: None,
    }
}

pub fn seed_articles() -> Vec<Article> {
    vec![
        Article {
            id: "article-quiet-workflow".to_string(),
            title: "毎朝30分でnoteを書き始めるための小さな儀式".to_string(),
            status: ArticleStatus::Draft,
            idea_memo: "忙しい会社員が、白紙の怖さを減らして朝に本文へ入るための手順を書く。"
                .to_string(),
            markdown: "# 毎朝30分でnoteを書き始めるための小さな儀式\n\n朝の執筆は、気合いよりも入口の設計で決まります。\n\n## 最初の5分で昨日の素材を見る\n\n前日に残した一文、読者の悩み、結論だけを見返します。ここで新しい調査を始めないことが大切です。\n\n## 10分で見出しを並べる\n\n見出しは完成形ではなく、本文へ降りるための足場です。順番はあとで直せます。\n\n## 15分で一番書ける段落だけ書く\n\n冒頭から書く必要はありません。具体例や失敗談から入ると、自然に本文の温度が上がります。\n\n## まとめ\n\n朝の30分は、完成ではなく前進を作る時間です。明日の自分が戻りやすい一文を最後に置きましょう。\n".to_string(),
            thumbnail_brief:
                "朝の机、ノート、淡い光。文字は「朝30分のnote習慣」。静かで実務的な雰囲気。"
                    .to_string(),
            reflection: String::new(),
            tags: vec!["執筆習慣".to_string(), "note".to_string(), "朝活".to_string()],
            created_at: "2026-05-08T00:00:00Z".to_string(),
            updated_at: "2026-05-08T00:00:00Z".to_string(),
            published_at: None,
        },
        Article {
            id: "article-review-checklist".to_string(),
            title: "投稿前チェックリストでnoteの読み落としを減らす".to_string(),
            status: ArticleStatus::Ready,
            idea_memo:
                "公開直前に迷いがちなタイトル、導入、見出し、貼り付け崩れを1画面で見る話。"
                    .to_string(),
            markdown: "# 投稿前チェックリストでnoteの読み落としを減らす\n\n公開前の不安は、才能ではなく確認項目で減らせます。\n\n## タイトルは読者の状況から始める\n\n自分の主張より、読者が今いる場所を先に置くとクリック前の摩擦が下がります。\n\n## 導入で約束を一つに絞る\n\n記事が何を解決するのかを一文で言える状態にします。\n\n## note貼り付け後に見出し間隔を見る\n\nMarkdownからHTMLへ変換した後、改行やリストの詰まりを確認します。\n\n## 最後に次の行動を置く\n\n読後に何を試せばよいかが見えると、保存や共有につながります。\n".to_string(),
            thumbnail_brief: "チェックリストと公開ボタンのイメージ。文字は「投稿前に見る7項目」。"
                .to_string(),
            reflection: String::new(),
            tags: vec!["チェックリスト".to_string(), "note運用".to_string()],
            created_at: "2026-05-08T00:00:00Z".to_string(),
            updated_at: "2026-05-08T00:00:00Z".to_string(),
            published_at: None,
        },
        Article {
            id: "article-published-retro".to_string(),
            title: "公開後メモを残すと次の記事が楽になる".to_string(),
            status: ArticleStatus::Published,
            idea_memo: "数字だけではなく、反応、書きにくかった箇所、次の仮説を残す運用。"
                .to_string(),
            markdown: "# 公開後メモを残すと次の記事が楽になる\n\n公開して終わりにすると、得られた学びが次の記事に残りません。\n\n## 反応は量より種類で見る\n\nスキ、コメント、SNSでの引用を分けて記録します。\n\n## 書きにくかった箇所を残す\n\n次回の構成づくりで同じ詰まりを避けられます。\n\n## 次の仮説に変える\n\n振り返りは反省ではなく、次のネタを作る材料です。\n".to_string(),
            thumbnail_brief:
                "公開後のメモ帳と小さな分析グラフ。文字は「公開後メモ」。".to_string(),
            reflection: "導入に対する反応が多かった。次はチェックリスト形式にすると保存されやすそう。"
                .to_string(),
            tags: vec!["振り返り".to_string(), "note分析".to_string()],
            created_at: "2026-05-08T00:00:00Z".to_string(),
            updated_at: "2026-05-08T00:00:00Z".to_string(),
            published_at: Some("2026-05-01T09:00:00Z".to_string()),
        },
    ]
}

pub fn status_counts(articles: &[Article]) -> BTreeMap<ArticleStatus, usize> {
    ArticleStatus::ALL
        .into_iter()
        .map(|status| {
            (
                status,
                articles
                    .iter()
                    .filter(|article| article.status == status)
                    .count(),
            )
        })
        .collect()
}

pub fn extract_outline(markdown: &str) -> Vec<String> {
    markdown
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            let marks = trimmed.chars().take_while(|ch| *ch == '#').count();
            if (1..=3).contains(&marks) && trimmed.chars().nth(marks) == Some(' ') {
                Some(format!(
                    "{}{}",
                    "  ".repeat(marks.saturating_sub(1)),
                    trimmed[marks + 1..].trim()
                ))
            } else {
                None
            }
        })
        .collect()
}

pub fn suggested_outline(article: &Article) -> String {
    let title = article_title(article);
    [
        format!("# {title}"),
        String::new(),
        "## 読者の悩み".to_string(),
        String::new(),
        "## この記事で持ち帰れること".to_string(),
        String::new(),
        "## 背景と具体例".to_string(),
        String::new(),
        "## 今日から試せる手順".to_string(),
        String::new(),
        "## まとめ".to_string(),
        String::new(),
    ]
    .join("\n")
}

pub fn markdown_to_note_html(markdown: &str) -> String {
    let mut out = Vec::new();
    let mut list_items: Vec<String> = Vec::new();
    let flush_list = |out: &mut Vec<String>, list_items: &mut Vec<String>| {
        if !list_items.is_empty() {
            let items = list_items
                .iter()
                .map(|item| format!("<li>{}</li>", inline_markdown(item)))
                .collect::<String>();
            out.push(format!("<ul>{items}</ul>"));
            list_items.clear();
        }
    };

    for raw in markdown.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            flush_list(&mut out, &mut list_items);
            continue;
        }

        let heading_marks = line.chars().take_while(|ch| *ch == '#').count();
        if (1..=3).contains(&heading_marks) && line.chars().nth(heading_marks) == Some(' ') {
            flush_list(&mut out, &mut list_items);
            let body = line[heading_marks + 1..].trim();
            out.push(format!(
                "<h{heading_marks}>{}</h{heading_marks}>",
                inline_markdown(body)
            ));
            continue;
        }

        if let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            list_items.push(item.to_string());
            continue;
        }

        if let Some(quote) = line.strip_prefix(">") {
            flush_list(&mut out, &mut list_items);
            out.push(format!(
                "<blockquote>{}</blockquote>",
                inline_markdown(quote.trim())
            ));
            continue;
        }

        flush_list(&mut out, &mut list_items);
        out.push(format!("<p>{}</p>", inline_markdown(line)));
    }

    flush_list(&mut out, &mut list_items);
    out.join("\n")
}

pub fn title_candidates(article: &Article) -> Vec<String> {
    let title = article_title(article);
    let topic = article
        .tags
        .iter()
        .find(|tag| !matches!(tag.as_str(), "note" | "執筆"))
        .cloned()
        .unwrap_or_else(|| key_phrase(&article.idea_memo));

    [
        format!("{title}を無理なく続ける方法"),
        format!("{topic}で迷ったときに見直す小さな手順"),
        format!("書く前に整える{topic}の考え方"),
        format!("{title}: 今日から試せる実務メモ"),
        format!("{topic}をnoteで伝えるためのチェックリスト"),
    ]
    .into_iter()
    .fold(Vec::new(), |mut acc, title| {
        if !acc.contains(&title) {
            acc.push(title);
        }
        acc
    })
}

pub fn thumbnail_prompt(article: &Article) -> String {
    format!(
        "noteサムネイル制作指示\n主題: {}\nキーワード: {}\n画面要素: {}\nトーン: macOS風の静かな作業感、余白多め、文字は短く高コントラスト\n避けること: 情報量過多、派手な煽り、本文と無関係な写真",
        article_title(article),
        if article.tags.is_empty() {
            "note、執筆".to_string()
        } else {
            article.tags.join("、")
        },
        if article.thumbnail_brief.trim().is_empty() {
            "本文テーマが一目で伝わるサムネイル"
        } else {
            article.thumbnail_brief.trim()
        }
    )
}

pub fn preflight_checklist(article: &Article) -> Vec<ChecklistItem> {
    let markdown = article.markdown.trim();
    let outline = extract_outline(markdown);
    let paragraphs = markdown
        .split("\n\n")
        .filter(|paragraph| !paragraph.trim().is_empty())
        .count();
    let longest_line = markdown
        .lines()
        .map(|line| line.trim().chars().count())
        .max()
        .unwrap_or(0);
    let tail = markdown
        .chars()
        .rev()
        .take(320)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    let has_action = ["試", "始め", "確認", "保存", "書いて", "やって", "次"]
        .iter()
        .any(|needle| tail.contains(needle));

    vec![
        ChecklistItem {
            label: "タイトル",
            passed: (12..=42).contains(&article.title.trim().chars().count()),
            detail: "12〜42文字を目安に、読者の状況か得られる結果を入れる。",
        },
        ChecklistItem {
            label: "見出し構成",
            passed: outline
                .iter()
                .filter(|line| !line.starts_with("  "))
                .count()
                >= 3,
            detail: "H2以上の見出しを3つ以上置き、流れを見える状態にする。",
        },
        ChecklistItem {
            label: "本文量",
            passed: markdown.chars().count() >= 700,
            detail: "導入、具体例、まとめまで含めて700字以上を初期目安にする。",
        },
        ChecklistItem {
            label: "段落密度",
            passed: paragraphs >= 5 && longest_line <= 140,
            detail: "長すぎる段落を避け、スマホで読みやすい余白を残す。",
        },
        ChecklistItem {
            label: "サムネイル指示",
            passed: article.thumbnail_brief.trim().chars().count() >= 20,
            detail: "主題、入れたい文字、避けたい表現が分かる指示を残す。",
        },
        ChecklistItem {
            label: "読後アクション",
            passed: has_action,
            detail: "最後に読者が試すこと、保存する理由、次の行動を置く。",
        },
    ]
}

pub fn readability_suggestions(article: &Article) -> Vec<String> {
    let markdown = &article.markdown;
    let mut suggestions = Vec::new();
    if markdown
        .lines()
        .any(|line| line.trim().chars().count() > 140)
    {
        suggestions
            .push("140文字を超える行があるため、スマホ表示では2〜3文で改段落する。".to_string());
    }
    if extract_outline(markdown).len() < 4 {
        suggestions
            .push("見出しが少ないため、読者の悩み、結論、具体例、手順に分ける。".to_string());
    }
    if !markdown.contains("例えば")
        && !markdown.contains("具体的")
        && !markdown.contains("たとえば")
    {
        suggestions.push("具体例を1つ追加し、主張だけで終わらない本文にする。".to_string());
    }
    if suggestions.is_empty() {
        suggestions.push(
            "本文の段落、見出し、具体例のバランスは初稿として扱いやすい状態です。".to_string(),
        );
    }
    suggestions
}

pub fn local_assistant_response(task: AssistantTask, article: &Article) -> String {
    match task {
        AssistantTask::Outline => suggested_outline(article),
        AssistantTask::Titles => title_candidates(article)
            .into_iter()
            .enumerate()
            .map(|(idx, title)| format!("{}. {title}", idx + 1))
            .collect::<Vec<_>>()
            .join("\n"),
        AssistantTask::Readability => readability_suggestions(article)
            .into_iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n"),
        AssistantTask::Thumbnail => thumbnail_prompt(article),
        AssistantTask::Preflight => preflight_checklist(article)
            .into_iter()
            .map(|item| {
                format!(
                    "{}: {} - {}",
                    if item.passed { "OK" } else { "要確認" },
                    item.label,
                    item.detail
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        AssistantTask::Format => [
            "note貼り付け前の整形メモ",
            "- H2/H3の階層を崩さない",
            "- 箇条書きの前後に空行を残す",
            "- 太字は強調箇所だけに絞る",
            "- 変換HTMLを貼り付け後、見出し間隔とリスト表示を確認する",
        ]
        .join("\n"),
    }
}

pub fn build_assistant_prompt(task: AssistantTask, article: &Article) -> String {
    format!(
        "あなたはnote記事の編集者です。日本語で短く実用的に返答してください。\n\n依頼: {}\nタイトル: {}\n状態: {}\nネタメモ: {}\nタグ: {}\n\n本文Markdown:\n{}",
        task.label(),
        article.title,
        article.status.label(),
        article.idea_memo,
        article.tags.join(", "),
        article.markdown.chars().take(6000).collect::<String>()
    )
}

pub fn stamp_updated(article: &mut Article) {
    article.updated_at = timestamp();
    if article.status == ArticleStatus::Published && article.published_at.is_none() {
        article.published_at = Some(article.updated_at.clone());
    }
}

fn sanitize_article(mut article: Article) -> Article {
    article.title = article.title.trim().chars().take(160).collect();
    article.tags = article
        .tags
        .into_iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .fold(Vec::new(), |mut acc, tag| {
            if !acc.contains(&tag) && acc.len() < 12 {
                acc.push(tag);
            }
            acc
        });
    article
}

fn article_title(article: &Article) -> String {
    let trimmed = article.title.trim();
    if trimmed.is_empty() {
        "note記事".to_string()
    } else {
        trimmed.to_string()
    }
}

fn key_phrase(text: &str) -> String {
    let phrase = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if phrase.is_empty() {
        "note執筆".to_string()
    } else {
        phrase.chars().take(16).collect()
    }
}

fn inline_markdown(text: &str) -> String {
    let escaped = escape_html(text);
    apply_inline_emphasis(&escaped)
}

fn apply_inline_emphasis(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    loop {
        let Some(start) = rest.find("**") else {
            out.push_str(rest);
            break;
        };
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("**") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        out.push_str("<strong>");
        out.push_str(&after_start[..end]);
        out.push_str("</strong>");
        rest = &after_start[end + 2..];
    }
    out
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format!("{seconds}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_to_html_escapes_script_and_formats_heading() {
        let html = markdown_to_note_html("# 見出し\n\n本文 **強調** <script>\n\n- one\n- two");
        assert!(html.contains("<h1>見出し</h1>"));
        assert!(html.contains("<strong>強調</strong>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("<ul><li>one</li><li>two</li></ul>"));
    }

    #[test]
    fn outline_extracts_up_to_h3() {
        assert_eq!(
            extract_outline("# A\n## B\n### C\n#### D"),
            vec!["A", "  B", "    C"]
        );
    }

    #[test]
    fn preflight_flags_short_drafts() {
        let mut article = new_article();
        article.title = "短い".to_string();
        article.markdown = "# 短い\n\n本文だけ".to_string();
        article.thumbnail_brief.clear();
        let checklist = preflight_checklist(&article);
        assert!(checklist.iter().any(|item| !item.passed));
        assert!(
            !checklist
                .iter()
                .find(|item| item.label == "タイトル")
                .unwrap()
                .passed
        );
    }

    #[test]
    fn title_candidates_are_unique() {
        let article = seed_articles().remove(0);
        let titles = title_candidates(&article);
        let mut deduped = titles.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(titles.len(), deduped.len());
        assert!(titles.join("\n").contains("執筆習慣"));
    }
}
