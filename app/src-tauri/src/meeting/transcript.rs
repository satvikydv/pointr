//! The running meeting transcript: what the other people in the call said,
//! phrase by phrase, kept in memory only and dropped when the meeting ends.
//! Nothing here touches disk.
//!
//! Free of app, device and model code so it can be unit-tested directly.

use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// Seconds since the meeting session started, at the end of the phrase.
    pub t: f64,
    pub text: String,
}

/// How much history is kept at all. Answers only ever look at the last few
/// minutes (see `recent`); this bounds memory over a long call.
pub const KEEP_SECS: f64 = 15.0 * 60.0;

/// Meeting mode listens to every output device at once, so if Windows plays
/// the same audio on two of them, the same phrase arrives twice. A phrase
/// matching one already heard within this many seconds is that echo.
pub const DUPLICATE_SECS: f64 = 4.0;

/// Case, punctuation and spacing ignored, so two transcriptions of the same
/// audio compare equal.
fn comparable(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Default)]
pub struct Transcript {
    lines: VecDeque<Line>,
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a phrase. Blank text (silence the model returned nothing
    /// for) and an echo of a phrase just heard on another device (see
    /// DUPLICATE_SECS) are ignored. Returns whether a line was added.
    pub fn push(&mut self, t: f64, text: &str) -> bool {
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            return false;
        }
        let key = comparable(&text);
        let echo = self
            .lines
            .iter()
            .rev()
            .take_while(|l| (t - l.t).abs() <= DUPLICATE_SECS)
            .any(|l| comparable(&l.text) == key);
        if echo {
            return false;
        }
        self.lines.push_back(Line { t, text });
        while self.lines.front().is_some_and(|l| l.t < t - KEEP_SECS) {
            self.lines.pop_front();
        }
        true
    }

    /// The newest lines, oldest first: everything from the last `max_secs`
    /// before the newest line, cut down further (oldest first) to fit
    /// `max_chars`. The newest line is always included, even alone over
    /// budget, since it's usually the question.
    pub fn recent(&self, max_secs: f64, max_chars: usize) -> Vec<Line> {
        let Some(newest) = self.lines.back() else { return Vec::new() };
        let since = newest.t - max_secs;
        let mut out = Vec::new();
        let mut chars = 0;
        for line in self.lines.iter().rev() {
            if line.t < since {
                break;
            }
            chars += line.text.chars().count();
            if !out.is_empty() && chars > max_chars {
                break;
            }
            out.push(line.clone());
        }
        out.reverse();
        out
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_normalises_and_skips_blank() {
        let mut t = Transcript::new();
        assert!(!t.push(1.0, "   "));
        assert!(!t.push(1.0, ""));
        assert!(t.push(2.0, "  so   what's \n your  plan? "));
        assert_eq!(t.recent(60.0, 1000), vec![Line { t: 2.0, text: "so what's your plan?".into() }]);
    }

    #[test]
    fn old_lines_are_pruned() {
        let mut t = Transcript::new();
        t.push(0.0, "first");
        t.push(KEEP_SECS - 1.0, "second");
        assert_eq!(t.len(), 2);
        t.push(KEEP_SECS + 10.0, "third");
        assert_eq!(t.len(), 2, "the line from t=0 is now older than KEEP_SECS");
        assert_eq!(t.recent(KEEP_SECS * 2.0, 1000)[0].text, "second");
    }

    #[test]
    fn recent_window_by_time() {
        let mut t = Transcript::new();
        t.push(10.0, "a");
        t.push(200.0, "b");
        t.push(250.0, "c");
        let r: Vec<_> = t.recent(60.0, 1000).into_iter().map(|l| l.text).collect();
        assert_eq!(r, vec!["b", "c"]);
    }

    #[test]
    fn recent_window_by_chars_keeps_newest() {
        let mut t = Transcript::new();
        t.push(1.0, "aaaaaaaaaa");
        t.push(2.0, "bbbbbbbbbb");
        t.push(3.0, "cccccccccc");
        let r: Vec<_> = t.recent(60.0, 25).into_iter().map(|l| l.text).collect();
        assert_eq!(r, vec!["bbbbbbbbbb", "cccccccccc"]);
        // Newest is kept even when it alone is over budget.
        let r = t.recent(60.0, 3);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].text, "cccccccccc");
    }

    #[test]
    fn echo_from_a_second_device_is_dropped() {
        let mut t = Transcript::new();
        assert!(t.push(10.0, "So, what's your plan?"));
        assert!(!t.push(10.4, "so what's your plan"), "same words, moments apart: an echo");
        assert!(t.push(10.5, "And the timeline?"));
        assert!(t.push(10.0 + DUPLICATE_SECS + 1.0, "So, what's your plan?"), "asked again later: kept");
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn empty_transcript() {
        let t = Transcript::new();
        assert!(t.is_empty());
        assert!(t.recent(60.0, 100).is_empty());
    }
}
