//! The on-device speech model: where it lives, whether it's installed, and
//! the one-time download.
//!
//! Parakeet TDT 0.6B v3 (NVIDIA, CC-BY-4.0), int8 ONNX export by istupakov
//! on Hugging Face. Downloaded on first use rather than bundled, so the
//! installer stays small for people who never use voice. The revision is
//! pinned and every file is checked against its SHA-256 before it's used,
//! so a changed or corrupted upstream file is rejected, not loaded.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager};

const REPO: &str = "istupakov/parakeet-tdt-0.6b-v3-onnx";
const REVISION: &str = "8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce";
pub const MODEL_DIR_NAME: &str = "parakeet-tdt-0.6b-v3-int8";

pub struct ModelFile {
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

pub const FILES: [ModelFile; 4] = [
    ModelFile {
        name: "encoder-model.int8.onnx",
        size: 652_183_999,
        sha256: "6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09",
    },
    ModelFile {
        name: "decoder_joint-model.int8.onnx",
        size: 18_202_004,
        sha256: "eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70",
    },
    ModelFile {
        name: "nemo128.onnx",
        size: 139_764,
        sha256: "a9fde1486ebfcc08f328d75ad4610c67835fea58c73ba57e3209a6f6cf019e9f",
    },
    ModelFile {
        name: "vocab.txt",
        size: 93_939,
        sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
    },
];

pub fn total_size() -> u64 {
    FILES.iter().map(|f| f.size).sum()
}

/// Local app data, not the roaming config dir settings.json lives in: 670 MB
/// has no business syncing between machines with a roaming profile.
pub fn model_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let base = app
        .path()
        .app_local_data_dir()
        .map_err(|e| format!("Failed to resolve data dir: {}", e))?;
    Ok(base.join("models").join(MODEL_DIR_NAME))
}

/// Installed means every file is present at its exact expected size. The
/// hash is checked once, at download time; re-hashing 650 MB on every
/// launch would add seconds for no real gain.
pub fn is_installed(dir: &Path) -> bool {
    FILES.iter().all(|f| {
        std::fs::metadata(dir.join(f.name))
            .map(|m| m.len() == f.size)
            .unwrap_or(false)
    })
}

static DOWNLOADING: AtomicBool = AtomicBool::new(false);

pub fn is_downloading() -> bool {
    DOWNLOADING.load(Ordering::SeqCst)
}

#[derive(Clone, serde::Serialize)]
struct Progress {
    downloaded: u64,
    total: u64,
}

/// Downloads any missing files, emitting `voice-model-progress` as it goes.
/// Each file streams to `<name>.part`, is hashed while it downloads, and is
/// renamed into place only if the hash matches, so an interrupted or
/// tampered download never looks installed.
pub async fn download(app: AppHandle) -> Result<(), String> {
    if DOWNLOADING.swap(true, Ordering::SeqCst) {
        return Err("The voice model is already downloading.".into());
    }
    let result = match model_dir(&app) {
        Ok(dir) => {
            let emitter = app.clone();
            download_to(&dir, move |downloaded, total| {
                let _ = emitter.emit("voice-model-progress", Progress { downloaded, total });
            })
            .await
        }
        Err(e) => Err(e),
    };
    DOWNLOADING.store(false, Ordering::SeqCst);
    result
}

/// The download itself, independent of the app so it can be exercised
/// directly. `progress(downloaded, total)` is called about 5 times a second.
pub async fn download_to(dir: &Path, progress: impl Fn(u64, u64)) -> Result<(), String> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| format!("Failed to create model folder: {}", e))?;

    let total = total_size();
    // Files already in place count as done, so a retry after a failure
    // only fetches what's missing.
    let mut done: u64 = FILES
        .iter()
        .filter(|f| std::fs::metadata(dir.join(f.name)).map(|m| m.len() == f.size).unwrap_or(false))
        .map(|f| f.size)
        .sum();
    progress(done, total);

    let client = reqwest::Client::new();
    for file in FILES.iter() {
        let dest = dir.join(file.name);
        if std::fs::metadata(&dest).map(|m| m.len() == file.size).unwrap_or(false) {
            continue;
        }
        let url = format!("https://huggingface.co/{}/resolve/{}/{}", REPO, REVISION, file.name);
        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Download failed ({}): {}", file.name, e))?;
        if !resp.status().is_success() {
            return Err(format!("Download failed ({}): HTTP {}", file.name, resp.status()));
        }

        let part = dir.join(format!("{}.part", file.name));
        let mut out = tokio::fs::File::create(&part)
            .await
            .map_err(|e| format!("Failed to write {}: {}", file.name, e))?;
        let mut hasher = Sha256::new();
        let mut stream = resp.bytes_stream();
        let mut last_emit = std::time::Instant::now();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("Download interrupted ({}): {}", file.name, e))?;
            hasher.update(&chunk);
            out.write_all(&chunk)
                .await
                .map_err(|e| format!("Failed to write {}: {}", file.name, e))?;
            done += chunk.len() as u64;
            if last_emit.elapsed() >= std::time::Duration::from_millis(200) {
                progress(done, total);
                last_emit = std::time::Instant::now();
            }
        }
        out.flush().await.map_err(|e| e.to_string())?;
        drop(out);

        let digest = hex(&hasher.finalize());
        if digest != file.sha256 {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(format!(
                "{} failed its integrity check, so it was deleted rather than used. Try again.",
                file.name
            ));
        }
        tokio::fs::rename(&part, &dest)
            .await
            .map_err(|e| format!("Failed to finish {}: {}", file.name, e))?;
    }

    progress(total, total);
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_consistent() {
        for f in FILES.iter() {
            assert_eq!(f.sha256.len(), 64, "{} hash must be a full SHA-256", f.name);
            assert!(f.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
            assert!(f.size > 0);
        }
        // ~670 MB: the figure quoted to users in Settings and the setup guide.
        assert!((600_000_000..750_000_000).contains(&total_size()));
    }

    #[test]
    fn hex_matches_known_digest() {
        let d = Sha256::digest(b"abc");
        assert_eq!(hex(&d), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn missing_dir_is_not_installed() {
        assert!(!is_installed(Path::new("Z:\\definitely\\not\\here")));
    }

    /// Real end-to-end check against the network and the real model: runs
    /// the actual downloader into the same folder the app uses, then
    /// transcribes real speech (a WAV path in POINTR_VOICE_TEST_WAV, 16 kHz
    /// mono) two ways: whole, and the way push-to-talk does it, phrase by
    /// phrase, timing only what's left after "release". Slow and ~670 MB,
    /// so ignored by default:
    ///   cargo test voice_end_to_end -- --ignored --nocapture
    #[test]
    #[ignore]
    fn voice_end_to_end() {
        use crate::voice::segment;
        use transcribe_rs::onnx::parakeet::{ParakeetModel, ParakeetParams};
        use transcribe_rs::onnx::Quantization;

        let dir = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap())
            .join("dev.pointr.app")
            .join("models")
            .join(MODEL_DIR_NAME);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let started = std::time::Instant::now();
        rt.block_on(download_to(&dir, |d, t| {
            if d == t || d % 100_000_000 < 3_000_000 {
                println!("  download {} / {} MB", d / 1_000_000, t / 1_000_000);
            }
        }))
        .expect("download failed");
        assert!(is_installed(&dir));
        println!("download/verify: {:?}", started.elapsed());

        let t = std::time::Instant::now();
        let mut model = ParakeetModel::load(&dir, &Quantization::Int8).expect("load failed");
        println!("model load: {:?}", t.elapsed());

        let wav = std::env::var("POINTR_VOICE_TEST_WAV").expect("set POINTR_VOICE_TEST_WAV");
        let audio = transcribe_rs::audio::read_wav_samples(&PathBuf::from(wav)).expect("bad wav");
        println!("audio: {:.1}s", audio.len() as f32 / 16_000.0);

        let t = std::time::Instant::now();
        let whole = model.transcribe_with(&audio, &ParakeetParams::default()).unwrap().text;
        println!("whole-clip transcription: {:?}\n  -> {}", t.elapsed(), whole.trim());

        // Push-to-talk path: cut at pauses as audio "arrives", transcribing
        // each finished phrase; then time only the tail after release.
        let mut cut = 0;
        let mut parts = Vec::new();
        while let Some(next) = segment::find_cut(&audio, cut) {
            if !segment::is_silent(&audio[cut..next], &audio) {
                let piece = model.transcribe_with(&audio[cut..next], &ParakeetParams::default()).unwrap();
                parts.push(piece.text.trim().to_string());
            }
            cut = next;
        }
        println!("phrases done while held: {} ({:.1}s of audio)", parts.len(), cut as f32 / 16_000.0);
        let t = std::time::Instant::now();
        let tail = &audio[cut..];
        if !segment::is_silent(tail, &audio) {
            parts.push(model.transcribe_with(tail, &ParakeetParams::default()).unwrap().text.trim().to_string());
        }
        println!("left after release: {:.1}s of audio, transcribed in {:?}", tail.len() as f32 / 16_000.0, t.elapsed());
        println!("  -> {}", parts.join(" "));
        assert!(!parts.join(" ").trim().is_empty());
    }
}
