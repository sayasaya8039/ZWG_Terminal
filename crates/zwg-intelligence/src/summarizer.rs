//! Rule-based extractive summarizer for terminal output.
//!
//! Scans terminal output lines and extracts key signals:
//! - Build results (success/failure, error count)
//! - Test results (passed/failed/total)
//! - Exit codes
//! - Last error/warning message
//! - Active process description
//!
//! Produces a short summary string suitable for tab/pane headers.
//! No LLM needed — pure pattern matching, <50µs for 1000 lines.

/// Maximum summary length in characters.
const MAX_SUMMARY_LEN: usize = 60;

/// Minimum lines before summarization triggers.
pub const MIN_LINES_FOR_SUMMARY: usize = 50;

/// Summary of terminal output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSummary {
    /// Short text for tab display (e.g. "Build OK", "3 errors", "Tests: 47/47")
    pub text: String,
    /// Severity for visual styling.
    pub severity: Severity,
}

/// Visual severity of the summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Neutral / informational.
    Info,
    /// Success (green).
    Success,
    /// Warning (yellow).
    Warning,
    /// Error/failure (red).
    Error,
}

/// Summarize terminal output lines.
///
/// Scans lines from end to start (most recent first) looking for
/// recognizable patterns. Returns the first strong match.
pub fn summarize(lines: &[&str]) -> OutputSummary {
    if lines.is_empty() {
        return OutputSummary {
            text: String::new(),
            severity: Severity::Info,
        };
    }

    // Scan from end (most recent output is most relevant)
    let scan_limit = lines.len().min(200);
    let scan_range = &lines[lines.len() - scan_limit..];

    // Try extractors in priority order
    if let Some(s) = try_test_result(scan_range) { return s; }
    if let Some(s) = try_build_result(scan_range) { return s; }
    if let Some(s) = try_exit_code(scan_range) { return s; }
    if let Some(s) = try_error_count(scan_range) { return s; }
    if let Some(s) = try_last_error(scan_range) { return s; }
    if let Some(s) = try_last_warning(scan_range) { return s; }
    if let Some(s) = try_active_process(scan_range) { return s; }

    // Fallback: last non-empty line truncated
    let last = lines.iter().rev().find(|l| !l.trim().is_empty());
    match last {
        Some(line) => {
            let trimmed = line.trim();
            let text = if trimmed.len() > MAX_SUMMARY_LEN {
                format!("{}...", &trimmed[..MAX_SUMMARY_LEN - 3])
            } else {
                trimmed.to_string()
            };
            OutputSummary {
                text,
                severity: Severity::Info,
            }
        }
        None => OutputSummary {
            text: String::new(),
            severity: Severity::Info,
        },
    }
}

// --- Extractors ---

fn try_test_result(lines: &[&str]) -> Option<OutputSummary> {
    for line in lines.iter().rev() {
        let lower = line.to_ascii_lowercase();

        // Rust: "test result: ok. 47 passed; 0 failed"
        if lower.contains("test result:") {
            if let Some(summary) = parse_rust_test_result(line) {
                return Some(summary);
            }
        }

        // Generic: "Tests: 47 passed, 3 failed" / "47 passing, 3 failing"
        if (lower.contains("passed") || lower.contains("passing"))
            && (lower.contains("failed") || lower.contains("failing") || lower.contains("total"))
        {
            return Some(extract_test_summary(line));
        }

        // pytest: "== 5 passed in 1.23s =="
        if lower.contains("passed") && lower.contains(" in ") && lower.contains("==") {
            return Some(extract_test_summary(line));
        }

        // Jest: "Tests: 3 passed, 3 total"
        if lower.starts_with("tests:") && lower.contains("total") {
            return Some(extract_test_summary(line));
        }
    }
    None
}

fn try_build_result(lines: &[&str]) -> Option<OutputSummary> {
    for line in lines.iter().rev() {
        let lower = line.to_ascii_lowercase();

        // Cargo: "Finished `release` profile"
        if lower.contains("finished") && (lower.contains("profile") || lower.contains("target")) {
            return Some(OutputSummary {
                text: truncate_summary(line.trim()),
                severity: Severity::Success,
            });
        }

        // npm/yarn: "added 123 packages"
        if lower.contains("added") && lower.contains("package") {
            return Some(OutputSummary {
                text: truncate_summary(line.trim()),
                severity: Severity::Success,
            });
        }

        // Go: "go: downloading" → skip, "go build" success has no output
        // MSBuild: "Build succeeded."
        if lower.contains("build succeeded") {
            return Some(OutputSummary {
                text: "Build succeeded".to_string(),
                severity: Severity::Success,
            });
        }

        // "BUILD SUCCESSFUL" (Gradle)
        if lower.contains("build successful") {
            return Some(OutputSummary {
                text: "Build successful".to_string(),
                severity: Severity::Success,
            });
        }

        // "Build failed" / "BUILD FAILED"
        if lower.contains("build failed") {
            return Some(OutputSummary {
                text: "Build failed".to_string(),
                severity: Severity::Error,
            });
        }

        // "could not compile"
        if lower.contains("could not compile") {
            return Some(OutputSummary {
                text: truncate_summary(line.trim()),
                severity: Severity::Error,
            });
        }
    }
    None
}

fn try_exit_code(lines: &[&str]) -> Option<OutputSummary> {
    for line in lines.iter().rev() {
        let lower = line.to_ascii_lowercase();
        for prefix in ["exit code ", "exit status ", "exited with code "] {
            if let Some(idx) = lower.find(prefix) {
                let after = &line[idx + prefix.len()..];
                let code_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(code) = code_str.parse::<u32>() {
                    let severity = if code == 0 {
                        Severity::Success
                    } else {
                        Severity::Error
                    };
                    return Some(OutputSummary {
                        text: format!("Exit code {code}"),
                        severity,
                    });
                }
            }
        }
    }
    None
}

fn try_error_count(lines: &[&str]) -> Option<OutputSummary> {
    for line in lines.iter().rev() {
        let lower = line.to_ascii_lowercase();

        // "error: aborting due to 3 previous errors"
        if lower.contains("aborting due to") && lower.contains("error") {
            return Some(OutputSummary {
                text: truncate_summary(line.trim()),
                severity: Severity::Error,
            });
        }

        // "Found 5 errors" / "5 errors generated"
        if (lower.contains("found") || lower.contains("generated"))
            && lower.contains("error")
        {
            if let Some(n) = extract_number_before(&lower, "error") {
                return Some(OutputSummary {
                    text: format!("{n} error{}", if n == 1 { "" } else { "s" }),
                    severity: Severity::Error,
                });
            }
        }

        // "N warnings generated"
        if lower.contains("warning") && lower.contains("generated") {
            if let Some(n) = extract_number_before(&lower, "warning") {
                return Some(OutputSummary {
                    text: format!("{n} warning{}", if n == 1 { "" } else { "s" }),
                    severity: Severity::Warning,
                });
            }
        }
    }
    None
}

fn try_last_error(lines: &[&str]) -> Option<OutputSummary> {
    for line in lines.iter().rev() {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("error") || lower.contains(": error:") || lower.contains("fatal:") {
            return Some(OutputSummary {
                text: truncate_summary(line.trim()),
                severity: Severity::Error,
            });
        }
    }
    None
}

fn try_last_warning(lines: &[&str]) -> Option<OutputSummary> {
    // Only if no errors found (callers try_last_error first)
    for line in lines.iter().rev() {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("warning") || lower.contains(": warning:") {
            return Some(OutputSummary {
                text: truncate_summary(line.trim()),
                severity: Severity::Warning,
            });
        }
    }
    None
}

fn try_active_process(lines: &[&str]) -> Option<OutputSummary> {
    for line in lines.iter().rev() {
        let lower = line.to_ascii_lowercase();

        // Claude Code / AI agent activity
        if lower.contains("claude") && (lower.contains("thinking") || lower.contains("writing")) {
            return Some(OutputSummary {
                text: truncate_summary(line.trim()),
                severity: Severity::Info,
            });
        }

        // "Compiling foo v1.2.3"
        if lower.starts_with("   compiling ") || lower.starts_with("  compiling ") {
            return Some(OutputSummary {
                text: truncate_summary(line.trim()),
                severity: Severity::Info,
            });
        }

        // "Downloading crates ..."
        if lower.contains("downloading") && lower.contains("crate") {
            return Some(OutputSummary {
                text: "Downloading crates...".to_string(),
                severity: Severity::Info,
            });
        }
    }
    None
}

// --- Helpers ---

fn parse_rust_test_result(line: &str) -> Option<OutputSummary> {
    let lower = line.to_ascii_lowercase();
    let passed = extract_number_before(&lower, "passed")?;
    // Use rfind for "failed" to skip "test result: FAILED." prefix
    let failed = extract_number_before_last(&lower, "failed").unwrap_or(0);

    let severity = if failed > 0 {
        Severity::Error
    } else {
        Severity::Success
    };

    let text = if failed > 0 {
        format!("Tests: {passed} passed, {failed} failed")
    } else {
        format!("Tests: {passed}/{passed} passed")
    };

    Some(OutputSummary { text, severity })
}

fn extract_test_summary(line: &str) -> OutputSummary {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();

    let has_fail = lower.contains("fail");
    let severity = if has_fail {
        Severity::Error
    } else {
        Severity::Success
    };

    OutputSummary {
        text: truncate_summary(trimmed),
        severity,
    }
}

/// Find the number immediately before the first occurrence of a keyword.
fn extract_number_before(text: &str, keyword: &str) -> Option<u32> {
    let idx = text.find(keyword)?;
    extract_trailing_number(&text[..idx])
}

/// Find the number immediately before the last occurrence of a keyword.
fn extract_number_before_last(text: &str, keyword: &str) -> Option<u32> {
    let idx = text.rfind(keyword)?;
    extract_trailing_number(&text[..idx])
}

/// Extract the trailing number from a string (e.g. "foo 42 " → 42).
fn extract_trailing_number(text: &str) -> Option<u32> {
    let trimmed = text.trim_end();
    let num_str: String = trimmed
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    num_str.parse().ok()
}

fn truncate_summary(s: &str) -> String {
    if s.len() <= MAX_SUMMARY_LEN {
        s.to_string()
    } else if s.is_char_boundary(MAX_SUMMARY_LEN - 3) {
        format!("{}...", &s[..MAX_SUMMARY_LEN - 3])
    } else {
        // Find nearest char boundary
        let mut end = MAX_SUMMARY_LEN - 3;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_test_result_ok() {
        let lines = vec![
            "running 47 tests",
            "test result: ok. 47 passed; 0 failed; 0 ignored",
        ];
        let s = summarize(&lines.iter().map(|l| *l).collect::<Vec<_>>());
        assert_eq!(s.severity, Severity::Success);
        assert!(s.text.contains("47"));
    }

    #[test]
    fn rust_test_result_fail() {
        let lines = vec![
            "test result: FAILED. 45 passed; 2 failed; 0 ignored",
        ];
        let s = summarize(&lines);
        assert_eq!(s.severity, Severity::Error);
        assert!(s.text.contains("2 failed"));
    }

    #[test]
    fn cargo_build_success() {
        let lines = vec![
            "   Compiling smux v0.8.19",
            "    Finished `release` profile [optimized] target(s) in 20.10s",
        ];
        let s = summarize(&lines);
        assert_eq!(s.severity, Severity::Success);
        assert!(s.text.contains("Finished"));
    }

    #[test]
    fn build_failed() {
        let lines = vec![
            "error[E0308]: mismatched types",
            "could not compile `smux`",
        ];
        let s = summarize(&lines);
        assert_eq!(s.severity, Severity::Error);
    }

    #[test]
    fn exit_code_nonzero() {
        let lines = vec![
            "Process finished with exit code 1",
        ];
        let s = summarize(&lines);
        assert_eq!(s.severity, Severity::Error);
        assert_eq!(s.text, "Exit code 1");
    }

    #[test]
    fn exit_code_zero() {
        let lines = vec![
            "Process finished with exit code 0",
        ];
        let s = summarize(&lines);
        assert_eq!(s.severity, Severity::Success);
        assert_eq!(s.text, "Exit code 0");
    }

    #[test]
    fn error_count() {
        let lines = vec![
            "error: aborting due to 3 previous errors",
        ];
        let s = summarize(&lines);
        assert_eq!(s.severity, Severity::Error);
        assert!(s.text.contains("aborting"));
    }

    #[test]
    fn warning_only() {
        let lines = vec![
            "warning: unused variable `x`",
        ];
        let s = summarize(&lines);
        assert_eq!(s.severity, Severity::Warning);
    }

    #[test]
    fn pytest_summary() {
        let lines = vec![
            "===== 5 passed in 1.23s =====",
        ];
        let s = summarize(&lines);
        assert_eq!(s.severity, Severity::Success);
    }

    #[test]
    fn jest_summary() {
        let lines = vec![
            "Tests: 3 passed, 3 total",
        ];
        let s = summarize(&lines);
        assert_eq!(s.severity, Severity::Success);
    }

    #[test]
    fn npm_install() {
        let lines = vec![
            "added 123 packages in 5s",
        ];
        let s = summarize(&lines);
        assert_eq!(s.severity, Severity::Success);
    }

    #[test]
    fn empty_lines() {
        let s = summarize(&[]);
        assert_eq!(s.text, "");
        assert_eq!(s.severity, Severity::Info);
    }

    #[test]
    fn fallback_last_line() {
        let lines = vec!["some random output"];
        let s = summarize(&lines);
        assert_eq!(s.text, "some random output");
        assert_eq!(s.severity, Severity::Info);
    }

    #[test]
    fn truncation() {
        let long = "a".repeat(100);
        let t = truncate_summary(&long);
        assert!(t.len() <= MAX_SUMMARY_LEN);
        assert!(t.ends_with("..."));
    }

    #[test]
    fn extract_number() {
        assert_eq!(extract_number_before("47 passed", "passed"), Some(47));
        assert_eq!(extract_number_before("due to 3 previous errors", "previous"), Some(3));
        assert_eq!(extract_number_before("no number here", "here"), None);
    }

    #[test]
    fn gradle_build_successful() {
        let lines = vec!["BUILD SUCCESSFUL in 10s"];
        let s = summarize(&lines);
        assert_eq!(s.severity, Severity::Success);
        assert_eq!(s.text, "Build successful");
    }
}
