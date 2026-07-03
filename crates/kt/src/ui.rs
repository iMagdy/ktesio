use std::fmt::Display;
use std::time::Duration;

use console::{measure_text_width, pad_str, style, Alignment, Term};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

const CHECK: &str = "✓";
const CROSS: &str = "✗";
const WARN: &str = "!";
const INFO: &str = "•";
const ARROW: &str = "→";
const ELLIPSIS: &str = "…";
const TABLE_GAP: &str = "  ";
const MAX_TABLE_WIDTH: usize = 132;
const MIN_TABLE_WIDTH: usize = 40;

pub fn install_progress(mp: &MultiProgress, name: &str) -> ProgressBar {
    let pb = mp.add(ProgressBar::new(100));
    pb.set_style(progress_style());
    pb.set_prefix(format!(
        "{} {}",
        style(ARROW).cyan(),
        style("install").bold()
    ));
    pb.set_message(format!("Preparing {}", skill_name(name)));
    pb.enable_steady_tick(Duration::from_millis(90));
    pb
}

pub fn upgrade_progress(mp: &MultiProgress, name: &str) -> ProgressBar {
    let pb = mp.add(ProgressBar::new(100));
    pb.set_style(progress_style());
    pb.set_prefix(format!(
        "{} {}",
        style(ARROW).cyan(),
        style("upgrade").bold()
    ));
    pb.set_message(format!("Preparing {}", skill_name(name)));
    pb.enable_steady_tick(Duration::from_millis(90));
    pb
}

pub fn init_progress(mp: &MultiProgress, name: &str) -> ProgressBar {
    let pb = mp.add(ProgressBar::new(100));
    pb.set_style(progress_style());
    pb.set_prefix(format!("{} {}", style(ARROW).cyan(), style("init").bold()));
    pb.set_message(format!("Checking {}", skill_name(name)));
    pb.enable_steady_tick(Duration::from_millis(90));
    pb
}

pub fn finish_success(pb: &ProgressBar, message: impl Display) {
    pb.set_position(100);
    pb.finish_with_message(success_text(message));
}

pub fn finish_error(pb: &ProgressBar, message: impl Display) {
    pb.finish_with_message(error_text(message));
}

pub fn success(message: impl Display) {
    println!("{}", success_text(message));
}

pub fn warning(message: impl Display) {
    eprintln!("{}", warning_text(message));
}

pub fn error(message: impl Display) {
    eprintln!("{}", error_text(message));
}

pub(crate) fn update_notice(current_version: &str, latest_version: &str) {
    eprintln!("{}", update_notice_text(current_version, latest_version));
}

pub(crate) fn update_notice_text(current_version: &str, latest_version: &str) -> String {
    format!(
        "{} Ktesio {latest_version} is available (current {current_version}). Update: kt self-update",
        warning_text("Update available:")
    )
}

pub fn info(message: impl Display) {
    println!("{} {}", style(INFO).cyan(), message);
}

pub fn success_text(message: impl Display) -> String {
    format!("{} {}", style(CHECK).green().bold(), message)
}

pub fn warning_text(message: impl Display) -> String {
    format!("{} {}", style(WARN).yellow().bold(), message)
}

pub fn error_text(message: impl Display) -> String {
    format!("{} {}", style(CROSS).red().bold(), message)
}

pub fn skill_name(name: &str) -> String {
    style(name).cyan().bold().to_string()
}

pub fn label(name: &str) -> String {
    style(name).dim().bold().to_string()
}

pub fn table_header(label: &str) -> String {
    style(label).color256(81).bold().to_string()
}

pub fn status_label(status: &str) -> String {
    match status {
        "installed" => format!("{} {}", style(CHECK).green().bold(), style(status).green()),
        "missing" => format!("{} {}", style(CROSS).red().bold(), style(status).red()),
        "not locked" | "not installed" => {
            format!("{} {}", style(WARN).yellow().bold(), style(status).yellow())
        }
        "orphaned" => format!(
            "{} {}",
            style(INFO).magenta().bold(),
            style(status).magenta()
        ),
        other => other.to_string(),
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum CellStyle {
    Plain,
    Muted,
    Skill,
    Number,
    Command,
    Status,
}

#[derive(Clone, Debug)]
pub(crate) struct TableCell {
    text: String,
    style: CellStyle,
}

impl TableCell {
    pub(crate) fn plain(text: impl Into<String>) -> Self {
        Self::new(text, CellStyle::Plain)
    }

    pub(crate) fn muted(text: impl Into<String>) -> Self {
        Self::new(text, CellStyle::Muted)
    }

    pub(crate) fn skill(text: impl Into<String>) -> Self {
        Self::new(text, CellStyle::Skill)
    }

    pub(crate) fn number(text: impl Into<String>) -> Self {
        Self::new(text, CellStyle::Number)
    }

    pub(crate) fn command(text: impl Into<String>) -> Self {
        Self::new(text, CellStyle::Command)
    }

    pub(crate) fn status(text: impl Into<String>) -> Self {
        Self::new(text, CellStyle::Status)
    }

    fn new(text: impl Into<String>, style: CellStyle) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    fn styled_text(&self) -> String {
        match self.style {
            CellStyle::Plain => self.text.clone(),
            CellStyle::Muted => style(&self.text).dim().to_string(),
            CellStyle::Skill => skill_name(&self.text),
            CellStyle::Number => style(&self.text).color256(141).to_string(),
            CellStyle::Command => style(&self.text).color256(117).to_string(),
            CellStyle::Status => status_label(&self.text),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TableColumn {
    header: &'static str,
    min_width: usize,
    max_width: usize,
    align: Alignment,
}

impl TableColumn {
    pub(crate) fn new(header: &'static str, min_width: usize, max_width: usize) -> Self {
        Self {
            header,
            min_width,
            max_width,
            align: Alignment::Left,
        }
    }

    pub(crate) fn right(mut self) -> Self {
        self.align = Alignment::Right;
        self
    }
}

pub(crate) fn print_table(title: &str, columns: &[TableColumn], rows: &[Vec<TableCell>]) {
    for line in render_table(title, columns, rows, table_display_width()) {
        println!("{line}");
    }
}

pub(crate) fn compact_source(source: &str) -> String {
    let source = source.trim_end_matches(".git");

    if let Some(rest) = source.strip_prefix("https://github.com/") {
        return rest.to_string();
    }

    if let Some(rest) = source.strip_prefix("git@github.com:") {
        return rest.to_string();
    }

    if source.starts_with('/') || source.starts_with("~/") {
        let prefix = if source.starts_with('/') { "/" } else { "~/" };
        let parts = source
            .trim_start_matches('/')
            .trim_start_matches("~/")
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        if parts.len() > 4 {
            return format!("{ELLIPSIS}/{}", parts[parts.len() - 4..].join("/"));
        }
        return format!("{prefix}{}", parts.join("/"));
    }

    source.to_string()
}

pub(crate) fn short_commit(commit: &str) -> String {
    if commit == "—" || commit == "-" {
        return "—".to_string();
    }

    if commit.len() >= 8 && commit.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return commit.chars().take(8).collect();
    }

    console::truncate_str(commit, 10, ELLIPSIS).into_owned()
}

pub(crate) fn print_diagnostics(title: &str, messages: &[String], kind: DiagnosticKind) {
    let width = table_display_width();
    let count = messages.len();
    let label = if count == 1 { "item" } else { "items" };
    let title = format!("{title} ({count} {label})");
    let header = match kind {
        DiagnosticKind::Error => format!(
            "{} {}",
            style(CROSS).red().bold(),
            style(title).red().bold()
        ),
        DiagnosticKind::Warning => format!(
            "{} {}",
            style(WARN).yellow().bold(),
            style(title).yellow().bold()
        ),
    };
    eprintln!("{header}");

    let text_width = width.saturating_sub(6).max(24);
    for message in messages {
        let wrapped = wrap_text(message, text_width);
        for (index, line) in wrapped.iter().enumerate() {
            if index == 0 {
                eprintln!("  {} {}", style(INFO).dim(), line);
            } else {
                eprintln!("    {line}");
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum DiagnosticKind {
    Error,
    Warning,
}

fn progress_style() -> ProgressStyle {
    ProgressStyle::with_template("{spinner:.cyan} {prefix} [{wide_bar:.cyan/blue}] {pos:>3}% {msg}")
        .unwrap()
        .progress_chars("=> ")
        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
}

fn render_table(
    title: &str,
    columns: &[TableColumn],
    rows: &[Vec<TableCell>],
    display_width: usize,
) -> Vec<String> {
    let widths = table_widths(columns, rows, display_width);
    let table_width =
        widths.iter().sum::<usize>() + TABLE_GAP.len() * columns.len().saturating_sub(1);
    let mut lines = Vec::with_capacity(rows.len() + 3);

    lines.push(format!(
        "{} {} {}",
        style("▸").color256(81).bold(),
        style(title).bold(),
        style(row_count(rows.len())).dim()
    ));

    let headers = columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            pad_str(
                &table_header(column.header),
                widths[index],
                column.align,
                Some(ELLIPSIS),
            )
            .into_owned()
        })
        .collect::<Vec<_>>();
    lines.push(headers.join(TABLE_GAP));
    lines.push(style("─".repeat(table_width)).dim().to_string());

    for row in rows {
        lines.push(format_row(columns, row, &widths));
    }

    lines
}

fn format_row(columns: &[TableColumn], row: &[TableCell], widths: &[usize]) -> String {
    columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let cell = row
                .get(index)
                .cloned()
                .unwrap_or_else(|| TableCell::plain(""));
            pad_str(
                &cell.styled_text(),
                widths[index],
                column.align,
                Some(ELLIPSIS),
            )
            .into_owned()
        })
        .collect::<Vec<_>>()
        .join(TABLE_GAP)
}

fn table_widths(
    columns: &[TableColumn],
    rows: &[Vec<TableCell>],
    display_width: usize,
) -> Vec<usize> {
    if columns.is_empty() {
        return Vec::new();
    }

    let gap_width = TABLE_GAP.len() * columns.len().saturating_sub(1);
    let available = display_width
        .saturating_sub(gap_width)
        .max(columns.len() * 4);
    let mut widths = columns
        .iter()
        .map(|column| {
            let header_width = measure_text_width(column.header);
            column
                .min_width
                .max(header_width)
                .min(column.max_width.max(4))
        })
        .collect::<Vec<_>>();
    let desired = desired_widths(columns, rows);

    while widths.iter().sum::<usize>() > available {
        let Some(index) = widest_shrinkable_column(&widths) else {
            break;
        };
        widths[index] -= 1;
    }

    loop {
        let total = widths.iter().sum::<usize>();
        if total >= available {
            break;
        }

        let Some(index) = columns.iter().enumerate().find_map(|(index, column)| {
            (widths[index] < desired[index] && widths[index] < column.max_width).then_some(index)
        }) else {
            break;
        };

        widths[index] += 1;
    }

    widths
}

fn desired_widths(columns: &[TableColumn], rows: &[Vec<TableCell>]) -> Vec<usize> {
    columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let cell_width = rows
                .iter()
                .filter_map(|row| row.get(index))
                .map(|cell| measure_text_width(&cell.styled_text()))
                .max()
                .unwrap_or(0);
            column.max_width.min(
                column
                    .min_width
                    .max(measure_text_width(column.header))
                    .max(cell_width),
            )
        })
        .collect()
}

fn widest_shrinkable_column(widths: &[usize]) -> Option<usize> {
    widths
        .iter()
        .enumerate()
        .filter(|(_, width)| **width > 4)
        .max_by_key(|(_, width)| **width)
        .map(|(index, _)| index)
}

fn table_display_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_else(|| usize::from(Term::stdout().size().1))
        .clamp(MIN_TABLE_WIDTH, MAX_TABLE_WIDTH)
}

fn row_count(count: usize) -> String {
    let label = if count == 1 { "row" } else { "rows" };
    format!("({count} {label})")
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        if measure_text_width(word) > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            lines.extend(split_long_word(word, width));
            continue;
        }

        let next_width = if current.is_empty() {
            measure_text_width(word)
        } else {
            measure_text_width(&current) + 1 + measure_text_width(word)
        };

        if next_width > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }

        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }

    if !current.is_empty() {
        lines.push(current);
    }

    lines
}

fn split_long_word(word: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();

    for ch in word.chars() {
        let next_width = measure_text_width(&current) + measure_text_width(&ch.to_string());
        if next_width > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        current.push(ch);
    }

    if !current.is_empty() {
        lines.push(current);
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_labels_keep_status_text() {
        assert!(status_label("installed").contains("installed"));
        assert!(status_label("missing").contains("missing"));
        assert!(status_label("not locked").contains("not locked"));
        assert!(status_label("orphaned").contains("orphaned"));
        assert_eq!(status_label("custom"), "custom");
    }

    #[test]
    fn test_message_helpers_keep_plain_text() {
        assert!(success_text("Installed docs").contains("Installed docs"));
        assert!(warning_text("Skipped docs").contains("Skipped docs"));
        assert!(error_text("Failed docs").contains("Failed docs"));
    }

    #[test]
    fn test_update_notice_text_points_to_self_update_only() {
        let notice = update_notice_text("0.3.0", "0.4.0");

        assert!(notice.contains("Update available"));
        assert!(notice.contains("0.3.0"));
        assert!(notice.contains("0.4.0"));
        assert!(notice.contains("kt self-update"));
        assert!(!notice.contains("cargo install"));
        assert!(!notice.contains("brew upgrade"));
        assert!(!notice.contains("github.com"));
    }

    #[test]
    fn test_render_table_respects_display_width() {
        let columns = [
            TableColumn::new("Name", 16, 30),
            TableColumn::new("Repo", 18, 64),
            TableColumn::new("Status", 12, 14),
        ];
        let rows = vec![vec![
            TableCell::skill("bmad-check-implementation-readiness"),
            TableCell::muted(
                "/Users/imagdy/dev/in1t.com/.agents/skills/bmad-check-implementation-readiness",
            ),
            TableCell::status("installed"),
        ]];

        for line in render_table("Skills", &columns, &rows, 60) {
            assert!(measure_text_width(&line) <= 60, "{line}");
        }
    }

    #[test]
    fn test_render_table_handles_alignment_and_missing_cells() {
        let columns = [
            TableColumn::new("Skill", 8, 20),
            TableColumn::new("Installs", 8, 9).right(),
            TableColumn::new("Install", 8, 24),
        ];
        let rows = vec![
            vec![
                TableCell::skill("docs"),
                TableCell::number("42"),
                TableCell::command("kt install example/docs"),
            ],
            vec![TableCell::plain("local")],
        ];

        let rendered = render_table("Search results", &columns, &rows, 80);

        assert!(rendered[0].contains("Search results"));
        assert!(rendered[0].contains("2 rows"));
        assert!(rendered.iter().any(|line| line.contains("42")));
        assert!(rendered.iter().any(|line| line.contains("local")));
    }

    #[test]
    fn test_print_helpers_do_not_panic() {
        let columns = [TableColumn::new("Name", 6, 12)];
        let rows = [vec![TableCell::plain("docs")]];
        print_table("Skills", &columns, &rows);
        update_notice("0.3.0", "0.4.0");
        print_diagnostics(
            "Errors",
            &["a diagnostic message that should wrap cleanly".to_string()],
            DiagnosticKind::Error,
        );
        print_diagnostics(
            "Warnings",
            &["a warning message that should wrap cleanly".to_string()],
            DiagnosticKind::Warning,
        );
    }

    #[test]
    fn test_table_width_helpers_handle_empty_and_constrained_tables() {
        assert!(table_widths(&[], &[], 80).is_empty());
        assert_eq!(widest_shrinkable_column(&[4, 4, 4]), None);

        let columns = [
            TableColumn::new("A", 10, 30),
            TableColumn::new("B", 10, 30),
            TableColumn::new("C", 10, 30),
        ];
        let rows = vec![vec![
            TableCell::plain("a very long first value"),
            TableCell::plain("a very long second value"),
            TableCell::plain("a very long third value"),
        ]];
        let widths = table_widths(&columns, &rows, 24);

        assert_eq!(widths.len(), columns.len());
        assert!(widths.iter().sum::<usize>() <= 20);
    }

    #[test]
    fn test_compact_source_keeps_useful_tail() {
        assert_eq!(
            compact_source("https://github.com/example/agent-docs.git"),
            "example/agent-docs"
        );
        assert_eq!(
            compact_source("git@github.com:example/agent-docs.git"),
            "example/agent-docs"
        );
        assert_eq!(
            compact_source("/Users/imagdy/dev/in1t.com/.agents/skills/docs"),
            "…/in1t.com/.agents/skills/docs"
        );
        assert_eq!(compact_source("~/dev/skm"), "~/dev/skm");
        assert_eq!(compact_source("local/repo"), "local/repo");
    }

    #[test]
    fn test_short_commit_prefers_short_hashes() {
        assert_eq!(
            short_commit("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"),
            "a1b2c3d4"
        );
        assert_eq!(short_commit("—"), "—");
        assert_eq!(short_commit("-"), "—");
        assert_eq!(short_commit("not-a-commit-ref"), "not-a-com…");
    }

    #[test]
    fn test_wrap_text_handles_empty_long_and_short_input() {
        assert_eq!(wrap_text("", 10), vec![String::new()]);
        assert_eq!(wrap_text("short text", 40), vec!["short text".to_string()]);
        assert_eq!(
            wrap_text("alpha beta gamma", 10),
            vec!["alpha beta", "gamma"]
        );
        assert_eq!(
            wrap_text("supercalifragilistic", 6),
            vec!["superc", "alifra", "gilist", "ic"]
        );
    }

    #[test]
    fn test_wrap_text_flushes_current_line_before_long_word() {
        let wrapped = wrap_text("short supercalifragilistic", 6);

        assert_eq!(wrapped.first().map(String::as_str), Some("short"));
        assert!(wrapped.len() > 2);
    }

    #[test]
    fn test_row_count_uses_singular_and_plural() {
        assert_eq!(row_count(1), "(1 row)");
        assert_eq!(row_count(2), "(2 rows)");
    }
}
