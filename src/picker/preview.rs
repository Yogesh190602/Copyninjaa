//! Text shown in the picker rows: previews, search highlighting, labels.

use std::ops::Range;

/// Characters of text kept for a row's preview (two lines show ~90).
const PREVIEW_CHARS: usize = 300;
/// A match this far into the preview is still on screen.
const VISIBLE_CHARS: usize = 60;
/// Context kept before a match when the preview jumps to it.
const LEAD_BYTES: usize = 30;

/// Theme colors used inside Pango markup, which can't refer to CSS colors.
#[derive(Debug, Clone)]
pub struct MarkupStyle {
    newline_mark: String,
    highlight_open: String,
}

impl MarkupStyle {
    /// `muted` and `accent` must be "#rrggbb" (theme colors are validated).
    pub fn new(muted: &str, accent: &str) -> Self {
        Self {
            newline_mark: format!("<span foreground=\"{}\">↵</span>", muted),
            highlight_open: format!("<span background=\"{}\" bgalpha=\"35%\">", accent),
        }
    }
}

/// What a text clip is, to choose how its row shows it.
#[derive(Debug, PartialEq)]
pub enum Kind {
    Text,
    /// A single web link, titled by its host.
    Link {
        host: String,
    },
    /// A color code such as #cba6f7, shown with a swatch.
    Color {
        rgb: (u8, u8, u8),
    },
    /// One or more file paths: their names and the first one's folder.
    Files {
        names: Vec<String>,
        folder: String,
    },
}

pub fn classify(text: &str) -> Kind {
    let t = text.trim();
    if t.is_empty() || t.len() > 8192 {
        return Kind::Text;
    }
    if let Some(color) = crate::theme::parse_color(t) {
        return Kind::Color {
            rgb: crate::theme::rgb(&color),
        };
    }
    if !t.contains(char::is_whitespace) {
        if let Some(rest) = t
            .strip_prefix("https://")
            .or_else(|| t.strip_prefix("http://"))
        {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
            let host = authority.rsplit('@').next().unwrap_or_default();
            let host = host.strip_prefix("www.").unwrap_or(host);
            if !host.is_empty() {
                return Kind::Link {
                    host: host.to_string(),
                };
            }
        }
    }
    let lines: Vec<&str> = t.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.len() <= 100 {
        let paths: Option<Vec<String>> = lines.iter().map(|l| as_path(l)).collect();
        if let Some(paths) = paths {
            return Kind::Files {
                names: paths.iter().map(|p| file_name(p)).collect(),
                folder: folder_of(&paths[0]),
            };
        }
    }
    Kind::Text
}

/// The local path on a line ("/…", "~/…" or "file:///…"), if that's all it is.
fn as_path(line: &str) -> Option<String> {
    let path = match line.strip_prefix("file://") {
        Some(rest) => crate::daemon::percent_decode(rest),
        None => line.to_string(),
    };
    let rooted = path.starts_with('/') || path.starts_with("~/");
    // Commands that start with a path ("/usr/bin/ls -la") aren't paths.
    let command_like =
        path.contains(" -") || path.contains(['|', ';', '&', '>', '<', '$', '`', '*', '"', '\'']);
    (rooted && path.trim_end_matches('/').len() > 1 && !command_like).then_some(path)
}

fn file_name(path: &str) -> String {
    let path = path.trim_end_matches('/');
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// The folder containing `path`, with the home directory shown as ~.
fn folder_of(path: &str) -> String {
    let path = path.trim_end_matches('/');
    let folder = match path.rfind('/') {
        Some(0) => "/",
        Some(i) => &path[..i],
        None => "",
    };
    if let Some(home) = dirs::home_dir().and_then(|h| h.to_str().map(str::to_string)) {
        if let Some(rest) = folder.strip_prefix(&home) {
            if rest.is_empty() || rest.starts_with('/') {
                return format!("~{}", rest);
            }
        }
    }
    folder.to_string()
}

/// Make text readable on one or two lines: blank lines dropped, whitespace
/// runs collapsed, line breaks shown as ↵. Stops after about `max_chars`.
pub fn display_text(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut chars = 0;
    for line in text.lines() {
        for (i, word) in line.split_whitespace().enumerate() {
            let sep = match (i, out.is_empty()) {
                (_, true) => "",
                (0, false) => " ↵ ",
                _ => " ",
            };
            out.push_str(sep);
            chars += sep.chars().count();
            for c in word.chars() {
                if chars >= max_chars {
                    return out;
                }
                out.push(c);
                chars += 1;
            }
        }
    }
    out
}

/// Pango markup for a text clip's preview, with the search terms highlighted.
/// When the first term only appears further in, the preview starts just
/// before that match.
pub fn markup(text: &str, terms: &[String], style: &MarkupStyle) -> String {
    highlight(&snippet(text, terms), terms, style)
}

/// Pango markup for `display` with the search terms highlighted.
pub fn highlight(display: &str, terms: &[String], style: &MarkupStyle) -> String {
    let mut ranges: Vec<Range<usize>> = terms
        .iter()
        .flat_map(|t| find_ci(display, t, usize::MAX))
        .collect();
    ranges.sort_by_key(|r| r.start);
    // Merge overlapping and touching matches into one highlight.
    let mut merged: Vec<Range<usize>> = Vec::new();
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }

    let mut out = String::new();
    let mut pos = 0;
    for r in merged {
        out.push_str(&plain(&display[pos..r.start], style));
        out.push_str(&style.highlight_open);
        out.push_str(&escape(&display[r.clone()]));
        out.push_str("</span>");
        pos = r.end;
    }
    out.push_str(&plain(&display[pos..], style));
    out
}

fn snippet(text: &str, terms: &[String]) -> String {
    let head = display_text(text, PREVIEW_CHARS);
    let Some(first) = terms.first() else {
        return head;
    };
    if let Some(r) = find_ci(&head, first, 1).first() {
        if head[..r.start].chars().count() <= VISIBLE_CHARS {
            return head;
        }
    }
    let Some(r) = find_ci(text, first, 1).into_iter().next() else {
        return head;
    };
    let mut lead = r.start.saturating_sub(LEAD_BYTES);
    while !text.is_char_boundary(lead) {
        lead -= 1;
    }
    if lead == 0 {
        return head;
    }
    // Start at a word boundary inside the lead-in, or at the match itself.
    let start = text[lead..r.start]
        .find(char::is_whitespace)
        .map_or(r.start, |i| lead + i);
    format!("… {}", display_text(&text[start..], PREVIEW_CHARS))
}

/// Byte ranges of case-insensitive matches of `needle` (already lowercase),
/// at most `limit` of them.
fn find_ci(haystack: &str, needle: &str, limit: usize) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    if needle.is_empty() {
        return found;
    }
    let mut i = 0;
    while i < haystack.len() && found.len() < limit {
        match match_at(haystack, i, needle) {
            Some(end) => {
                found.push(i..end);
                i = end;
            }
            None => i += haystack[i..].chars().next().map_or(1, char::len_utf8),
        }
    }
    found
}

fn match_at(haystack: &str, start: usize, needle: &str) -> Option<usize> {
    let mut needle = needle.chars().peekable();
    for (offset, c) in haystack[start..].char_indices() {
        for lower in c.to_lowercase() {
            if needle.next() != Some(lower) {
                return None;
            }
        }
        if needle.peek().is_none() {
            return Some(start + offset + c.len_utf8());
        }
    }
    None
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn plain(s: &str, style: &MarkupStyle) -> String {
    escape(s).replace('↵', &style.newline_mark)
}

/// Extra detail for long text clips, or None when the preview shows it all.
pub fn text_meta(text: &str) -> Option<String> {
    let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
    if lines >= 3 {
        return Some(format!("{} lines", group_digits(lines)));
    }
    let chars = text.chars().count();
    (chars > 160).then(|| format!("{} characters", group_digits(chars)))
}

/// "now", "4m", "2h", "3d", "5w", "4mo", "2y".
pub fn relative_time(timestamp: f64, now: f64) -> String {
    let secs = (now - timestamp).max(0.0) as u64;
    const DAY: u64 = 86_400;
    match secs {
        0..=59 => "now".to_string(),
        60..=3_599 => format!("{}m", secs / 60),
        3_600..=86_399 => format!("{}h", secs / 3_600),
        s if s < 7 * DAY => format!("{}d", s / DAY),
        s if s < 30 * DAY => format!("{}w", s / (7 * DAY)),
        s if s < 365 * DAY => format!("{}mo", s / (30 * DAY)),
        s => format!("{}y", s / (365 * DAY)),
    }
}

/// "512 bytes", "240 KB", "1.4 MB".
pub fn human_size(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{} bytes", bytes),
        1024..=1_048_575 => format!("{} KB", bytes / 1024),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

fn group_digits(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(q: &str) -> Vec<String> {
        q.split_whitespace().map(str::to_lowercase).collect()
    }

    fn style() -> MarkupStyle {
        MarkupStyle::new("#7f849c", "#cba6f7")
    }

    fn markup(text: &str, terms: &[String]) -> String {
        super::markup(text, terms, &style())
    }

    const HIGHLIGHT_OPEN: &str = "<span background=\"#cba6f7\" bgalpha=\"35%\">";
    const NEWLINE_MARK: &str = "<span foreground=\"#7f849c\">↵</span>";

    #[test]
    fn clips_are_classified() {
        assert_eq!(classify("plain words"), Kind::Text);
        assert_eq!(
            classify(" #CBA6F7\n"),
            Kind::Color {
                rgb: (203, 166, 247)
            }
        );
        assert_eq!(
            classify("https://www.github.com/a/b?c=d"),
            Kind::Link {
                host: "github.com".into()
            }
        );
        assert_eq!(classify("https://example.com and more"), Kind::Text);
        assert_eq!(
            classify("/etc/hosts\nfile:///tmp/My%20File.txt\n"),
            Kind::Files {
                names: vec!["hosts".into(), "My File.txt".into()],
                folder: "/etc".into()
            }
        );
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            classify(&format!("{}/Downloads/a.pdf", home.display())),
            Kind::Files {
                names: vec!["a.pdf".into()],
                folder: "~/Downloads".into()
            }
        );
        assert_eq!(
            classify("/usr/bin/ls -la"),
            Kind::Text,
            "a command, not a path"
        );
        assert_eq!(classify("/"), Kind::Text);
    }

    #[test]
    fn display_text_marks_line_breaks() {
        assert_eq!(
            display_text("  fn main() {\n\n    println!();\n}\n", 100),
            "fn main() { ↵ println!(); ↵ }"
        );
        assert_eq!(display_text("abcdef", 3), "abc");
        assert_eq!(display_text(&"word ".repeat(1000), 20).chars().count(), 20);
    }

    #[test]
    fn markup_escapes_and_highlights() {
        let m = markup("a <b> & Rust", &terms("rust"));
        assert_eq!(
            m,
            format!("a &lt;b&gt; &amp; {}Rust</span>", HIGHLIGHT_OPEN)
        );
        assert!(markup("x\ny", &[]).contains(NEWLINE_MARK));
    }

    #[test]
    fn markup_highlights_every_term_once() {
        let m = markup("git push origin", &terms("push or"));
        assert_eq!(m.matches(HIGHLIGHT_OPEN).count(), 2, "{}", m);
        let overlapping = markup("aaaa", &terms("aa aaa"));
        assert_eq!(overlapping, format!("{}aaaa</span>", HIGHLIGHT_OPEN));
    }

    #[test]
    fn preview_jumps_to_a_far_match() {
        let text = format!("{} needle here", "filler ".repeat(100));
        let m = markup(&text, &terms("needle"));
        assert!(m.starts_with("… filler "), "starts on a whole word: {}", m);
        assert!(m.contains(&format!("{}needle</span>", HIGHLIGHT_OPEN)));
        assert!(!markup("needle first", &terms("needle")).starts_with('…'));
    }

    #[test]
    fn case_insensitive_unicode_matching() {
        assert_eq!(find_ci("Größe GRÖSSE", "größe", 10), vec![0..7]);
        assert_eq!(find_ci("ÉCOLE école", "école", 10).len(), 2);
    }

    #[test]
    fn meta_and_time_labels() {
        assert_eq!(text_meta("short"), None);
        assert_eq!(text_meta("a\nb\nc\n"), Some("3 lines".to_string()));
        assert_eq!(
            text_meta(&"x".repeat(1234)),
            Some("1,234 characters".to_string())
        );
        assert_eq!(relative_time(100.0, 130.0), "now");
        assert_eq!(relative_time(0.0, 7_200.0), "2h");
        assert_eq!(relative_time(0.0, 3.0 * 86_400.0), "3d");
        assert_eq!(relative_time(10.0, 0.0), "now", "clock skew");
        assert_eq!(human_size(2_500), "2 KB");
        assert_eq!(human_size(3_145_728), "3.0 MB");
    }
}
