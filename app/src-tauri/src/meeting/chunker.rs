//! Cuts finished phrases out of a continuously growing audio buffer, for
//! meeting mode's never-ending recording. Push-to-talk records one bounded
//! utterance and can keep every sample; a meeting can run for an hour, so
//! this also drops audio that's already been transcribed, keeping only a
//! little behind the cut so the pause detector's noise estimate stays
//! stable.
//!
//! Free of app, device and model code so it can be unit-tested directly.

use crate::voice::segment;

/// Audio kept behind the cut, for `segment::silence_threshold`'s estimate
/// of the background noise level.
pub const CONTEXT_SAMPLES: usize = 10 * segment::TARGET_RATE as usize;

#[derive(Debug, Default)]
pub struct Chunker {
    /// Index into the buffer where untranscribed audio starts.
    pub cut: usize,
}

impl Chunker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes every finished phrase out of `buf` (16 kHz mono), skipping
    /// pieces that are only silence. With `force`, the unfinished tail
    /// counts as finished too: the user asked for an answer now, or the
    /// audio stopped arriving (WASAPI loopback delivers nothing at all
    /// while nothing is playing, so a pause never shows up as quiet
    /// samples). Afterwards, audio more than CONTEXT_SAMPLES behind the
    /// cut is dropped from the front of `buf`.
    pub fn take(&mut self, buf: &mut Vec<f32>, force: bool) -> Vec<Vec<f32>> {
        let mut pieces = Vec::new();
        self.cut = self.cut.min(buf.len());

        while let Some(next) = segment::find_cut(buf, self.cut) {
            let next = next.min(buf.len());
            if next <= self.cut {
                break;
            }
            let piece = &buf[self.cut..next];
            if !segment::is_silent(piece, buf) {
                pieces.push(piece.to_vec());
            }
            self.cut = next;
        }

        if force && self.cut < buf.len() {
            let tail = &buf[self.cut..];
            if !segment::is_silent(tail, buf) {
                pieces.push(tail.to_vec());
            }
            self.cut = buf.len();
        }

        if self.cut > CONTEXT_SAMPLES {
            let dropped = self.cut - CONTEXT_SAMPLES;
            buf.drain(..dropped);
            self.cut -= dropped;
        }
        pieces
    }

    /// For a fresh buffer (the audio device was reopened).
    pub fn reset(&mut self) {
        self.cut = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::segment::FRAME;

    fn tone(frames: usize, amp: f32) -> Vec<f32> {
        (0..frames * FRAME).map(|i| amp * ((i as f32) * 0.3).sin()).collect()
    }

    fn quiet(frames: usize) -> Vec<f32> {
        vec![0.0; frames * FRAME]
    }

    #[test]
    fn phrase_followed_by_pause_is_taken() {
        let mut c = Chunker::new();
        let mut buf = quiet(40);
        buf.extend(tone(60, 0.3));
        buf.extend(quiet(20));
        let pieces = c.take(&mut buf, false);
        assert_eq!(pieces.len(), 1);
        // Nothing new: nothing more taken.
        assert!(c.take(&mut buf, false).is_empty());
    }

    #[test]
    fn unfinished_phrase_waits_unless_forced() {
        let mut c = Chunker::new();
        let mut buf = quiet(40);
        buf.extend(tone(20, 0.3)); // still talking, no pause yet
        assert!(c.take(&mut buf, false).is_empty());
        let pieces = c.take(&mut buf, true);
        assert_eq!(pieces.len(), 1);
        assert_eq!(c.cut, buf.len());
    }

    #[test]
    fn forced_silence_yields_nothing() {
        let mut c = Chunker::new();
        let mut buf = quiet(100);
        assert!(c.take(&mut buf, true).is_empty());
        assert_eq!(c.cut, buf.len());
    }

    #[test]
    fn transcribed_audio_is_dropped_keeping_context() {
        let mut c = Chunker::new();
        let mut buf = Vec::new();
        // Many phrases' worth: speech, pause, repeated for ~60 s.
        for _ in 0..30 {
            buf.extend(tone(40, 0.3));
            buf.extend(quiet(25));
        }
        let before = buf.len();
        let pieces = c.take(&mut buf, true);
        assert_eq!(pieces.len(), 30);
        assert!(before > CONTEXT_SAMPLES * 2);
        assert_eq!(buf.len(), CONTEXT_SAMPLES, "only the context behind the cut is kept");
        assert_eq!(c.cut, buf.len());

        // New speech after the drain is still found.
        buf.extend(tone(40, 0.3));
        buf.extend(quiet(25));
        assert_eq!(c.take(&mut buf, false).len(), 1);
        assert!(buf.len() <= CONTEXT_SAMPLES + 65 * FRAME);
    }

    #[test]
    fn reset_after_new_buffer() {
        let mut c = Chunker::new();
        let mut buf = tone(60, 0.3);
        buf.extend(quiet(20));
        c.take(&mut buf, true);
        assert!(c.cut > 0);
        c.reset();
        let mut fresh = quiet(10);
        assert!(c.take(&mut fresh, false).is_empty());
        assert_eq!(c.cut, 0);
    }
}
