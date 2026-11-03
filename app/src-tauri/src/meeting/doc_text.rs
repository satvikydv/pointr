//! Text handling for the documents a user adds as meeting context (resume,
//! project write-ups, notes): which files can be read, tidying, and
//! fitting them into what one answer request can carry.
//!
//! Free of app and network code so it can be unit-tested directly.

/// Most text kept from one document. A resume is ~5k characters; this
/// leaves room for long write-ups. Same as the backend's per-document cap.
pub const MAX_DOC_CHARS: usize = 60_000;

/// Most document text sent with one answer (~15k tokens). Matches the
/// backend's own ceiling (routes/meeting.py MAX_DOCUMENTS_CHARS); more would
/// only slow every answer down.
pub const SEND_BUDGET_CHARS: usize = 60_000;

const PLAIN_TEXT: [&str; 6] = ["txt", "md", "markdown", "text", "rst", "csv"];
const EXTRACTED: [&str; 2] = ["pdf", "docx"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Read as UTF-8 on this machine.
    PlainText,
    /// Text pulled out by the backend (PDF, Word).
    Extracted,
}

fn extension(name: &str) -> String {
    name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).unwrap_or_default()
}

/// How a file is read, or None if Pointr can't use it.
pub fn kind_of(name: &str) -> Option<Kind> {
    let ext = extension(name);
    if PLAIN_TEXT.contains(&ext.as_str()) {
        Some(Kind::PlainText)
    } else if EXTRACTED.contains(&ext.as_str()) {
        Some(Kind::Extracted)
    } else {
        None
    }
}

/// Bytes of a text file as a string: UTF-8 (byte-order mark dropped), with
/// anything invalid replaced rather than rejected.
pub fn decode_text(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

/// Line endings normalised, trailing spaces off every line, runs of blank
/// lines down to one, trimmed. Same as the backend's `_tidy`.
pub fn tidy(text: &str) -> String {
    let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(normalised.len());
    let mut blank_run = 0;
    for line in normalised.split('\n') {
        let line = line.trim_end();
        if line.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

/// At most `max` characters, cut at the last line break in the final 10% if
/// there is one, so a document doesn't end mid-sentence. Returns whether
/// anything was cut.
pub fn truncate_chars(text: &str, max: usize) -> (String, bool) {
    let Some((byte_end, _)) = text.char_indices().nth(max) else {
        return (text.to_string(), false);
    };
    let head = &text[..byte_end];
    let floor = head.char_indices().nth(max - max / 10).map(|(i, _)| i).unwrap_or(0);
    let cut = head[floor..].rfind('\n').map(|i| floor + i).unwrap_or(byte_end);
    (head[..cut].trim_end().to_string(), true)
}

/// Documents in order until `budget` characters are used: the one that
/// crosses it is cut short, the rest are left out.
pub fn fit_budget(docs: Vec<(String, String)>, budget: usize) -> Vec<(String, String)> {
    let mut left = budget;
    let mut out = Vec::new();
    for (name, text) in docs {
        if left == 0 {
            break;
        }
        let (text, cut) = truncate_chars(&text, left);
        left = if cut { 0 } else { left - text.chars().count() };
        if !text.is_empty() {
            out.push((name, text));
        }
    }
    out
}

pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(kind_of("Resume.PDF"), Some(Kind::Extracted));
        assert_eq!(kind_of("cv.docx"), Some(Kind::Extracted));
        assert_eq!(kind_of("notes.md"), Some(Kind::PlainText));
        assert_eq!(kind_of("a.b.txt"), Some(Kind::PlainText));
        assert_eq!(kind_of("photo.png"), None);
        assert_eq!(kind_of("old.doc"), None);
        assert_eq!(kind_of("README"), None);
    }

    #[test]
    fn decode_drops_bom_and_survives_bad_bytes() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFhello"), "hello");
        assert_eq!(decode_text(b"caf\xFF"), "caf\u{FFFD}");
    }

    #[test]
    fn tidy_collapses_blank_runs() {
        assert_eq!(tidy("  a  \r\n\r\n\r\n\r\nb\t \n\n"), "a\n\nb");
        assert_eq!(tidy("\n\n"), "");
    }

    #[test]
    fn truncate_prefers_a_line_break() {
        let text = format!("{}\n{}", "a".repeat(95), "b".repeat(50));
        let (out, cut) = truncate_chars(&text, 100);
        assert!(cut);
        assert_eq!(out, "a".repeat(95), "cut at the newline, not mid-line");

        let (out, cut) = truncate_chars(&"x".repeat(200), 100);
        assert!(cut);
        assert_eq!(out.len(), 100, "no newline nearby: hard cut");

        let (out, cut) = truncate_chars("short", 100);
        assert_eq!((out.as_str(), cut), ("short", false));
    }

    #[test]
    fn truncate_counts_characters_not_bytes() {
        let (out, cut) = truncate_chars(&"é".repeat(10), 4);
        assert!(cut);
        assert_eq!(out, "éééé");
    }

    #[test]
    fn budget_cuts_the_crossing_doc_and_drops_the_rest() {
        let docs = vec![
            ("one".to_string(), "a".repeat(60)),
            ("two".to_string(), "b".repeat(60)),
            ("three".to_string(), "c".repeat(10)),
        ];
        let out = fit_budget(docs, 100);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].1.len(), 60);
        assert_eq!(out[1].1.len(), 40);

        let fits = fit_budget(vec![("a".into(), "x".into()), ("b".into(), "y".into())], 100);
        assert_eq!(fits.len(), 2);
    }
}
