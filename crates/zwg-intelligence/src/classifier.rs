//! Rule-based terminal output line classifier.
//!
//! Classifies lines into categories using regex + heuristics.
//! No NN model needed — fast enough for real-time classification (~1µs/line).

use serde::{Deserialize, Serialize};

/// Classification of a terminal output line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LineKind {
    /// Plain text, no special classification.
    Text,
    /// Error message (compiler error, stack trace header, etc.)
    Error,
    /// Warning message.
    Warning,
    /// A URL (http/https).
    Url,
    /// A file path, optionally with line:col.
    FilePath,
    /// Part of a stack trace.
    StackTrace,
    /// Part of a diff (git diff, unified diff).
    Diff,
    /// JSON content.
    Json,
    /// Tabular data (CSV-like, column-aligned).
    Table,
    /// A shell command or prompt.
    Command,
    /// Markdown content.
    Markdown,
}

impl LineKind {
    /// Human-readable label for display.
    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Url => "url",
            Self::FilePath => "file",
            Self::StackTrace => "stacktrace",
            Self::Diff => "diff",
            Self::Json => "json",
            Self::Table => "table",
            Self::Command => "command",
            Self::Markdown => "markdown",
        }
    }

    /// Whether this kind supports a "jump to" action (file:line).
    pub fn is_jumpable(self) -> bool {
        matches!(self, Self::Error | Self::FilePath | Self::StackTrace)
    }

    /// Whether this kind contains a clickable URL.
    pub fn has_url(self) -> bool {
        matches!(self, Self::Url)
    }
}

/// Result of classifying a line.
#[derive(Debug, Clone)]
pub struct LineClassification {
    pub kind: LineKind,
    /// Confidence score (0.0 - 1.0). Higher = more certain.
    pub confidence: f32,
    /// Extracted file path (if applicable).
    pub file_path: Option<String>,
    /// Extracted line number (if applicable).
    pub line_number: Option<u32>,
    /// Extracted column number (if applicable).
    pub column: Option<u32>,
    /// Extracted URL (if applicable).
    pub url: Option<String>,
}

impl Default for LineClassification {
    fn default() -> Self {
        Self {
            kind: LineKind::Text,
            confidence: 0.0,
            file_path: None,
            line_number: None,
            column: None,
            url: None,
        }
    }
}

/// Classify a single terminal output line.
///
/// Returns the best-matching classification. Runs in ~1µs per line
/// (pure string matching, no regex engine overhead for hot paths).
///
/// **Known limitation**: If `line` contains raw ANSI escape sequences
/// (e.g. `\x1b[31merror\x1b[0m`), patterns like `starts_with("error")`
/// will not match. Callers should strip ANSI sequences before classifying
/// colorized compiler/linter output for best results.
// TODO: add an ANSI stripping step or accept pre-stripped text
pub fn classify_line(line: &str) -> LineClassification {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return LineClassification::default();
    }

    // Try classifiers in priority order (most specific first)
    if let Some(c) = try_diff(trimmed) { return c; }
    if let Some(c) = try_error(trimmed) { return c; }
    if let Some(c) = try_warning(trimmed) { return c; }
    if let Some(c) = try_stacktrace(trimmed) { return c; }
    if let Some(c) = try_url(trimmed) { return c; }
    if let Some(c) = try_filepath(trimmed) { return c; }
    if let Some(c) = try_json(trimmed) { return c; }
    if let Some(c) = try_table(trimmed) { return c; }
    if let Some(c) = try_command(trimmed) { return c; }
    if let Some(c) = try_markdown(trimmed) { return c; }

    LineClassification::default()
}

/// Classify a batch of lines.
pub fn classify_lines(lines: &[&str]) -> Vec<LineClassification> {
    lines.iter().map(|line| classify_line(line)).collect()
}

// --- Individual classifiers ---

fn try_diff(line: &str) -> Option<LineClassification> {
    // Only match unambiguous diff headers — single +/- lines produce too many
    // false positives (bash xtrace `+ cmd`, markdown lists `- item`, etc.)
    if line.starts_with("diff --git ")
        || line.starts_with("--- a/")
        || line.starts_with("+++ b/")
        || line.starts_with("@@ ")
    {
        return Some(LineClassification {
            kind: LineKind::Diff,
            confidence: 0.95,
            ..Default::default()
        });
    }
    None
}

fn try_error(line: &str) -> Option<LineClassification> {
    let lower = line.to_ascii_lowercase();

    // Compiler error patterns: "file:line:col: error:"
    if let Some(c) = parse_file_line_col_message(line, "error") {
        return Some(c);
    }

    // Rust: "error[E0123]: message"
    if lower.starts_with("error[") || lower.starts_with("error:") {
        return Some(LineClassification {
            kind: LineKind::Error,
            confidence: 0.95,
            ..Default::default()
        });
    }

    // Generic error indicators
    if lower.contains("error:") || lower.contains("fatal:") || lower.contains("failed:") {
        return Some(LineClassification {
            kind: LineKind::Error,
            confidence: 0.8,
            ..Default::default()
        });
    }

    // Exit code errors — parse actual numeric code and flag non-zero
    for prefix in ["exit code ", "exit status "] {
        if let Some(idx) = lower.find(prefix) {
            let after = &line[idx + prefix.len()..];
            let code_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(code) = code_str.parse::<u32>() {
                if code != 0 {
                    return Some(LineClassification {
                        kind: LineKind::Error,
                        confidence: 0.75,
                        ..Default::default()
                    });
                }
            }
        }
    }

    None
}

fn try_warning(line: &str) -> Option<LineClassification> {
    let lower = line.to_ascii_lowercase();

    if let Some(c) = parse_file_line_col_message(line, "warning") {
        return Some(LineClassification { kind: LineKind::Warning, ..c });
    }

    if lower.starts_with("warning[") || lower.starts_with("warning:") {
        return Some(LineClassification {
            kind: LineKind::Warning,
            confidence: 0.95,
            ..Default::default()
        });
    }

    if lower.contains("warning:") || lower.contains("warn:") {
        return Some(LineClassification {
            kind: LineKind::Warning,
            confidence: 0.7,
            ..Default::default()
        });
    }

    None
}

fn try_stacktrace(line: &str) -> Option<LineClassification> {
    let trimmed = line.trim();

    // Rust backtrace: "  0: std::panicking::begin_panic"
    if trimmed.chars().next().map_or(false, |c| c.is_ascii_digit())
        && trimmed.contains(": ")
        && (trimmed.contains("::") || trimmed.contains("at "))
    {
        return Some(LineClassification {
            kind: LineKind::StackTrace,
            confidence: 0.85,
            ..Default::default()
        });
    }

    // Python: "  File \"path\", line 42, in func"
    if trimmed.starts_with("File \"") && trimmed.contains(", line ") {
        let path = trimmed
            .strip_prefix("File \"")
            .and_then(|s| s.split('"').next())
            .map(|s| s.to_string());
        let line_num = trimmed
            .split(", line ")
            .nth(1)
            .and_then(|s| s.split(',').next())
            .and_then(|s| s.trim().parse::<u32>().ok());
        return Some(LineClassification {
            kind: LineKind::StackTrace,
            confidence: 0.9,
            file_path: path,
            line_number: line_num,
            ..Default::default()
        });
    }

    // Node.js/JS: "    at Function.run (path:line:col)"
    if trimmed.starts_with("at ") && trimmed.contains('(') && trimmed.contains(':') {
        return Some(LineClassification {
            kind: LineKind::StackTrace,
            confidence: 0.85,
            ..Default::default()
        });
    }

    // Go: "goroutine 1 [running]:" or "\tpath/file.go:42"
    if trimmed.starts_with("goroutine ") || (trimmed.starts_with('\t') && trimmed.ends_with(')')) {
        return Some(LineClassification {
            kind: LineKind::StackTrace,
            confidence: 0.8,
            ..Default::default()
        });
    }

    None
}

fn try_url(line: &str) -> Option<LineClassification> {
    // Find http:// or https:// URLs
    for prefix in ["https://", "http://"] {
        if let Some(start) = line.find(prefix) {
            let url_part = &line[start..];
            let end = url_part
                .find(|c: char| c.is_whitespace() || c == ')' || c == ']' || c == '>' || c == '"' || c == '\'')
                .unwrap_or(url_part.len());
            let url = &url_part[..end];
            if url.len() > prefix.len() + 3 {
                return Some(LineClassification {
                    kind: LineKind::Url,
                    confidence: 0.95,
                    url: Some(url.to_string()),
                    ..Default::default()
                });
            }
        }
    }
    None
}

fn try_filepath(line: &str) -> Option<LineClassification> {
    let trimmed = line.trim();

    // Skip URLs
    if trimmed.contains("://") {
        return None;
    }

    // Try parsing path:line:col from the end using rsplit_once
    // Pattern: <path>:<line>:<col> or <path>:<line>
    if let Some((rest, last)) = trimmed.rsplit_once(':') {
        if let Ok(last_num) = last.trim().parse::<u32>() {
            // Could be path:line or path:line:col (last_num is col)
            if let Some((path_part, mid)) = rest.rsplit_once(':') {
                if let Ok(line_num) = mid.trim().parse::<u32>() {
                    // path:line:col
                    if looks_like_filepath(path_part) {
                        return Some(LineClassification {
                            kind: LineKind::FilePath,
                            confidence: 0.85,
                            file_path: Some(path_part.to_string()),
                            line_number: Some(line_num),
                            column: Some(last_num),
                            ..Default::default()
                        });
                    }
                }
            }
            // path:line (no col)
            if looks_like_filepath(rest) {
                return Some(LineClassification {
                    kind: LineKind::FilePath,
                    confidence: 0.8,
                    file_path: Some(rest.to_string()),
                    line_number: Some(last_num),
                    column: None,
                    ..Default::default()
                });
            }
        }
    }

    // Standalone file path
    if looks_like_filepath(trimmed) {
        return Some(LineClassification {
            kind: LineKind::FilePath,
            confidence: 0.6,
            file_path: Some(trimmed.to_string()),
            ..Default::default()
        });
    }

    None
}

fn try_json(line: &str) -> Option<LineClassification> {
    let trimmed = line.trim();
    if (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
    {
        return Some(LineClassification {
            kind: LineKind::Json,
            confidence: 0.85,
            ..Default::default()
        });
    }
    // JSON object start
    if trimmed == "{" || trimmed == "[" || trimmed.starts_with("{\"") || trimmed.starts_with("[{") {
        return Some(LineClassification {
            kind: LineKind::Json,
            confidence: 0.7,
            ..Default::default()
        });
    }
    None
}

fn try_table(line: &str) -> Option<LineClassification> {
    let trimmed = line.trim();

    // Pipe-delimited tables: "| col1 | col2 |"
    if trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.matches('|').count() >= 3 {
        return Some(LineClassification {
            kind: LineKind::Table,
            confidence: 0.9,
            ..Default::default()
        });
    }

    // Separator lines: "+---+---+" or "|---|---|"
    if (trimmed.starts_with('+') || trimmed.starts_with('|'))
        && trimmed.chars().all(|c| c == '+' || c == '-' || c == '|' || c == '=' || c == ' ')
        && trimmed.len() > 5
    {
        return Some(LineClassification {
            kind: LineKind::Table,
            confidence: 0.85,
            ..Default::default()
        });
    }

    // Tab-separated with multiple columns
    if trimmed.matches('\t').count() >= 2 {
        return Some(LineClassification {
            kind: LineKind::Table,
            confidence: 0.5,
            ..Default::default()
        });
    }

    None
}

fn try_command(line: &str) -> Option<LineClassification> {
    let trimmed = line.trim();

    // Shell prompts: "$ ", "> ", "% ", "PS> "
    if trimmed.starts_with("$ ")
        || trimmed.starts_with("> ")
        || trimmed.starts_with("% ")
        || trimmed.starts_with("PS>")
        || trimmed.starts_with("PS ")
    {
        return Some(LineClassification {
            kind: LineKind::Command,
            confidence: 0.8,
            ..Default::default()
        });
    }

    None
}

fn try_markdown(line: &str) -> Option<LineClassification> {
    let trimmed = line.trim();

    // Headers: "# ", "## ", "### "
    if trimmed.starts_with("# ") || trimmed.starts_with("## ") || trimmed.starts_with("### ") {
        return Some(LineClassification {
            kind: LineKind::Markdown,
            confidence: 0.8,
            ..Default::default()
        });
    }

    // Fenced code blocks
    if trimmed.starts_with("```") {
        return Some(LineClassification {
            kind: LineKind::Markdown,
            confidence: 0.9,
            ..Default::default()
        });
    }

    None
}

// --- Helpers ---

/// Parse "file:line:col: error/warning: message" patterns.
fn parse_file_line_col_message(line: &str, keyword: &str) -> Option<LineClassification> {
    // Look for ": error:" or ": warning:" in the line
    let lower = line.to_ascii_lowercase();
    let pattern = format!(": {}:", keyword);
    let idx = lower.find(&pattern)?;

    let prefix = &line[..idx];
    // Try to parse as file:line:col
    let parts: Vec<&str> = prefix.rsplitn(4, ':').collect();
    if parts.len() >= 3 {
        let col = parts[0].trim().parse::<u32>().ok();
        let line_num = parts[1].trim().parse::<u32>().ok();
        if line_num.is_some() {
            let path_end = prefix.len()
                - parts[0].len() - 1
                - parts[1].len() - 1;
            let path = &prefix[..path_end];
            return Some(LineClassification {
                kind: LineKind::Error,
                confidence: 0.95,
                file_path: Some(path.to_string()),
                line_number: line_num,
                column: col,
                url: None,
            });
        }
    }

    None
}

/// Check if a string looks like a file path.
fn looks_like_filepath(s: &str) -> bool {
    if s.len() < 3 || s.len() > 512 {
        return false;
    }

    // Windows absolute: C:\  or C:/
    if s.len() >= 3
        && s.as_bytes()[0].is_ascii_alphabetic()
        && s.as_bytes()[1] == b':'
        && (s.as_bytes()[2] == b'\\' || s.as_bytes()[2] == b'/')
    {
        return true;
    }

    // Unix absolute
    if s.starts_with('/') && !s.starts_with("//") {
        return true;
    }

    // Relative with extension: src/foo.rs, ./bar.py
    if (s.starts_with("./") || s.starts_with("../") || s.contains('/'))
        && s.contains('.')
        && !s.contains(' ')
    {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_rust_error() {
        let c = classify_line("error[E0308]: mismatched types");
        assert_eq!(c.kind, LineKind::Error);
        assert!(c.confidence > 0.9);
    }

    #[test]
    fn classify_compiler_error_with_path() {
        let c = classify_line("src/main.rs:42:5: error: expected `;`");
        assert_eq!(c.kind, LineKind::Error);
        assert_eq!(c.file_path.as_deref(), Some("src/main.rs"));
        assert_eq!(c.line_number, Some(42));
        assert_eq!(c.column, Some(5));
    }

    #[test]
    fn classify_warning() {
        let c = classify_line("warning: unused variable `x`");
        assert_eq!(c.kind, LineKind::Warning);
    }

    #[test]
    fn classify_url() {
        let c = classify_line("Visit https://github.com/smux-terminal/smux for details");
        assert_eq!(c.kind, LineKind::Url);
        assert_eq!(c.url.as_deref(), Some("https://github.com/smux-terminal/smux"));
    }

    #[test]
    fn classify_diff_header_plus() {
        let c = classify_line("+++ b/src/main.rs");
        assert_eq!(c.kind, LineKind::Diff);
    }

    #[test]
    fn single_plus_not_diff() {
        // Single + lines (bash xtrace, etc.) should NOT be classified as Diff
        let c = classify_line("+ echo hello");
        assert_ne!(c.kind, LineKind::Diff);
    }

    #[test]
    fn classify_diff_header() {
        let c = classify_line("diff --git a/src/main.rs b/src/main.rs");
        assert_eq!(c.kind, LineKind::Diff);
    }

    #[test]
    fn classify_python_stacktrace() {
        let c = classify_line("  File \"app.py\", line 42, in main");
        assert_eq!(c.kind, LineKind::StackTrace);
        assert_eq!(c.file_path.as_deref(), Some("app.py"));
        assert_eq!(c.line_number, Some(42));
    }

    #[test]
    fn classify_node_stacktrace() {
        let c = classify_line("    at Function.run (/app/server.js:123:45)");
        assert_eq!(c.kind, LineKind::StackTrace);
    }

    #[test]
    fn classify_json() {
        let c = classify_line("{\"key\": \"value\", \"num\": 42}");
        assert_eq!(c.kind, LineKind::Json);
    }

    #[test]
    fn classify_table() {
        let c = classify_line("| Name | Age | City |");
        assert_eq!(c.kind, LineKind::Table);
    }

    #[test]
    fn classify_command() {
        let c = classify_line("$ cargo build --release");
        assert_eq!(c.kind, LineKind::Command);
    }

    #[test]
    fn classify_markdown_header() {
        let c = classify_line("## Installation");
        assert_eq!(c.kind, LineKind::Markdown);
    }

    #[test]
    fn classify_exit_code_nonzero() {
        let c = classify_line("Process exited with exit code 1");
        assert_eq!(c.kind, LineKind::Error);
    }

    #[test]
    fn classify_exit_code_zero_not_error() {
        let c = classify_line("Process exited with exit code 0, took 1.2s");
        assert_ne!(c.kind, LineKind::Error);
    }

    #[test]
    fn classify_plain_text() {
        let c = classify_line("Hello, world!");
        assert_eq!(c.kind, LineKind::Text);
    }

    #[test]
    fn classify_empty() {
        let c = classify_line("");
        assert_eq!(c.kind, LineKind::Text);
    }

    #[test]
    fn classify_filepath_standalone() {
        let c = classify_line("src/terminal/view.rs");
        assert_eq!(c.kind, LineKind::FilePath);
    }

    #[test]
    fn classify_windows_path() {
        let c = classify_line("C:\\Users\\Owner\\project\\main.rs");
        assert_eq!(c.kind, LineKind::FilePath);
    }

    #[test]
    fn url_not_filepath() {
        let c = classify_line("https://example.com/path/file.rs");
        assert_eq!(c.kind, LineKind::Url);
    }
}
