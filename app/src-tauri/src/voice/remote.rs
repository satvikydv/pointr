//! The "transcribe with my OpenAI key" option, for people who'd rather not
//! download the on-device model or whose machine is too slow for it.
//!
//! Same shape as the local path: each finished phrase is sent while the
//! key is still held, so release only waits on the last one. Audio goes to
//! OpenAI under the user's own key; Pointr's backend never sees it.

use std::time::Duration;

use super::segment::TARGET_RATE;

/// Cheapest of OpenAI's transcription models, and plenty for short spoken
/// commands.
pub const MODEL: &str = "gpt-4o-mini-transcribe";

/// 16-bit PCM mono WAV at 16 kHz, built in memory.
pub fn wav_bytes(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&TARGET_RATE.to_le_bytes());
    out.extend_from_slice(&(TARGET_RATE * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Blocking: called from the push-to-talk transcriber thread.
pub fn transcribe(api_key: &str, samples: &[f32]) -> Result<String, String> {
    let wav = wav_bytes(samples);
    let key = api_key.to_string();
    tauri::async_runtime::block_on(async move {
        let part = reqwest::multipart::Part::bytes(wav)
            .file_name("speech.wav")
            .mime_str("audio/wav")
            .map_err(|e| e.to_string())?;
        let form = reqwest::multipart::Form::new()
            .text("model", MODEL)
            .text("response_format", "json")
            .part("file", part);
        let res = reqwest::Client::new()
            .post("https://api.openai.com/v1/audio/transcriptions")
            .bearer_auth(&key)
            .timeout(Duration::from_secs(30))
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("OpenAI transcription failed: {}", e))?;
        let status = res.status();
        let body: serde_json::Value = res.json().await.unwrap_or_default();
        if !status.is_success() {
            let message = body["error"]["message"].as_str().unwrap_or("unknown error");
            return Err(format!("OpenAI transcription failed: {} {}", status.as_u16(), message));
        }
        Ok(body["text"].as_str().unwrap_or("").trim().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_valid_pcm16_mono_16k() {
        let wav = wav_bytes(&[0.0, 1.0, -1.0, 0.5]);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 8);
        assert_eq!(wav.len(), 44 + 8);
        assert_eq!(i16::from_le_bytes(wav[46..48].try_into().unwrap()), i16::MAX);
        assert_eq!(i16::from_le_bytes(wav[48..50].try_into().unwrap()), -i16::MAX);
    }

    /// Real call against OpenAI, phrase by phrase like push-to-talk.
    /// Needs POINTR_OPENAI_KEY and POINTR_VOICE_TEST_WAV (16 kHz mono):
    ///   cargo test openai_end_to_end -- --ignored --nocapture
    #[test]
    #[ignore]
    fn openai_end_to_end() {
        use crate::voice::segment;
        let key = std::env::var("POINTR_OPENAI_KEY").expect("set POINTR_OPENAI_KEY");
        let wav = std::env::var("POINTR_VOICE_TEST_WAV").expect("set POINTR_VOICE_TEST_WAV");
        let audio = transcribe_rs::audio::read_wav_samples(&std::path::PathBuf::from(wav)).unwrap();

        let mut cut = 0;
        let mut parts = Vec::new();
        while let Some(next) = segment::find_cut(&audio, cut) {
            if !segment::is_silent(&audio[cut..next], &audio) {
                let t = std::time::Instant::now();
                parts.push(transcribe(&key, &audio[cut..next]).expect("transcription failed"));
                println!("phrase ({:.1}s audio) in {:?}", (next - cut) as f32 / 16_000.0, t.elapsed());
            }
            cut = next;
        }
        let t = std::time::Instant::now();
        let tail = &audio[cut..];
        if !segment::is_silent(tail, &audio) {
            parts.push(transcribe(&key, tail).expect("transcription failed"));
        }
        println!("left after release: {:.1}s audio, done in {:?}", tail.len() as f32 / 16_000.0, t.elapsed());
        println!("  -> {}", parts.join(" "));
        assert!(!parts.join(" ").is_empty());

        // A bad key must come back as a readable error, not a panic.
        let err = transcribe("sk-invalid", &audio[..16_000]).unwrap_err();
        println!("bad key: {}", err);
        assert!(err.contains("401"));
    }
}
