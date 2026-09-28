//! Pure audio helpers for push-to-talk: resampling to the 16 kHz mono the
//! speech model expects, and finding pauses so finished phrases can be
//! transcribed while the user is still holding the key. Kept free of any
//! device/model code so it can be unit-tested directly.

pub const TARGET_RATE: u32 = 16_000;

/// 30 ms frames at 16 kHz: the unit the pause detector works in.
pub const FRAME: usize = 480;

/// A phrase has to end in at least this much quiet before it's cut off and
/// transcribed early. Long enough not to split a word at a breath, short
/// enough that most of an utterance is already done by release.
pub const PAUSE_FRAMES: usize = 12; // 360 ms

/// Don't cut pieces shorter than this: very short clips transcribe worse
/// and the saving is negligible.
pub const MIN_SEGMENT_FRAMES: usize = 34; // ~1 s

/// Force a cut after this long even without a pause, so a single breathless
/// run-on sentence still gets transcribed progressively.
pub const MAX_SEGMENT_FRAMES: usize = 500; // 15 s

/// Mixes interleaved device audio down to mono.
pub fn to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Linear-interpolation resampler. Speech models are insensitive to the
/// small aliasing this allows, and it avoids another dependency.
pub fn resample(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = ((input.len() as f64) / ratio).floor() as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos.floor() as usize;
            let frac = (pos - idx as f64) as f32;
            let a = input[idx];
            let b = *input.get(idx + 1).unwrap_or(&a);
            a + (b - a) * frac
        })
        .collect()
}

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Loudness below which a frame counts as quiet. Adapts to the room: the
/// quietest fifth of frames so far estimates background noise, and speech
/// has to clear it by a margin. The absolute floor keeps a silent mic (all
/// zeros) from making every faint hiss look like speech.
pub fn silence_threshold(frame_levels: &[f32]) -> f32 {
    const ABSOLUTE_FLOOR: f32 = 0.008;
    if frame_levels.is_empty() {
        return ABSOLUTE_FLOOR;
    }
    let mut sorted = frame_levels.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let noise = sorted[sorted.len() / 5];
    (noise * 3.0).max(ABSOLUTE_FLOOR)
}

/// Where the next finished phrase ends, as a sample index into `audio`
/// (16 kHz mono), considering only audio after `from`. Returns the end of
/// the first pause that follows real speech, once the piece is long enough;
/// or a forced cut at MAX_SEGMENT_FRAMES. None means keep listening.
pub fn find_cut(audio: &[f32], from: usize) -> Option<usize> {
    let levels: Vec<f32> = audio.chunks_exact(FRAME).map(rms).collect();
    let threshold = silence_threshold(&levels);
    let start_frame = from / FRAME;
    let mut heard_speech = false;
    let mut quiet_run = 0;

    for (i, &level) in levels.iter().enumerate().skip(start_frame) {
        let frames_in = i + 1 - start_frame;
        if level > threshold {
            heard_speech = true;
            quiet_run = 0;
        } else {
            quiet_run += 1;
        }
        if heard_speech && quiet_run >= PAUSE_FRAMES && frames_in >= MIN_SEGMENT_FRAMES {
            return Some((i + 1) * FRAME);
        }
        if frames_in >= MAX_SEGMENT_FRAMES {
            return Some((i + 1) * FRAME);
        }
    }
    None
}

/// True when a piece of audio holds no speech worth sending to the model:
/// transcribing pure silence wastes time and can yield stray words.
pub fn is_silent(audio: &[f32], whole_recording: &[f32]) -> bool {
    let levels: Vec<f32> = whole_recording.chunks_exact(FRAME).map(rms).collect();
    let threshold = silence_threshold(&levels);
    let loud_frames = audio.chunks_exact(FRAME).filter(|f| rms(f) > threshold).count();
    loud_frames < 3 // under ~90 ms of anything above the noise floor
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frames: usize, amp: f32) -> Vec<f32> {
        (0..frames * FRAME)
            .map(|i| amp * (i as f32 * 0.07).sin())
            .collect()
    }

    fn quiet(frames: usize) -> Vec<f32> {
        vec![0.001; frames * FRAME]
    }

    #[test]
    fn mono_mix_averages_channels() {
        assert_eq!(to_mono(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(to_mono(&[0.2, 0.4], 1), vec![0.2, 0.4]);
    }

    #[test]
    fn resample_48k_to_16k_keeps_duration() {
        let one_second = vec![0.0; 48_000];
        assert_eq!(resample(&one_second, 48_000, 16_000).len(), 16_000);
        let same = vec![0.3; 100];
        assert_eq!(resample(&same, 16_000, 16_000), same);
    }

    #[test]
    fn cuts_after_speech_followed_by_a_pause() {
        let mut audio = quiet(10);
        audio.extend(tone(40, 0.3));
        audio.extend(quiet(20));
        let cut = find_cut(&audio, 0).expect("should cut at the pause");
        // Somewhere inside the trailing pause, after the speech ended.
        assert!(cut > 50 * FRAME && cut <= 70 * FRAME, "cut at frame {}", cut / FRAME);
    }

    #[test]
    fn keeps_listening_mid_sentence() {
        let mut audio = quiet(5);
        audio.extend(tone(40, 0.3));
        audio.extend(quiet(4)); // a breath, not a pause
        assert_eq!(find_cut(&audio, 0), None);
    }

    #[test]
    fn does_not_cut_a_too_short_piece() {
        let mut audio = tone(10, 0.3);
        audio.extend(quiet(15));
        assert_eq!(find_cut(&audio, 0), None);
    }

    #[test]
    fn silence_alone_never_cuts_before_the_cap() {
        assert_eq!(find_cut(&quiet(200), 0), None);
    }

    #[test]
    fn forces_a_cut_on_a_long_run_on() {
        let audio = tone(MAX_SEGMENT_FRAMES + 20, 0.3);
        assert_eq!(find_cut(&audio, 0), Some(MAX_SEGMENT_FRAMES * FRAME));
    }

    #[test]
    fn only_looks_after_the_previous_cut() {
        let mut audio = tone(40, 0.3);
        audio.extend(quiet(20));
        let first = find_cut(&audio, 0).unwrap();
        assert_eq!(find_cut(&audio, first), None);
    }

    #[test]
    fn silent_piece_detected() {
        let mut whole = quiet(20);
        whole.extend(tone(40, 0.3));
        assert!(is_silent(&quiet(20), &whole));
        assert!(!is_silent(&tone(40, 0.3), &whole));
    }
}
