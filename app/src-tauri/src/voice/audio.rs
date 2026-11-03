//! Audio capture. Push-to-talk records the microphone, opened only while
//! the key is held and closed the moment it's released. Meeting mode
//! records system audio instead (WASAPI loopback of the default output
//! device: what the other people in the call say), only while a meeting
//! session is running.
//!
//! The cpal stream lives on its own thread for its whole life, because a
//! stream isn't guaranteed to be movable between threads on every backend.
//! Samples land in a shared buffer as 16 kHz mono f32 (converted in the
//! callback, so readers never deal with device formats or rates).

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use super::segment;

pub type SharedAudio = Arc<Mutex<Vec<f32>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The default microphone.
    Mic,
    /// Whatever the default output device is playing (loopback).
    System,
}

pub struct Recorder {
    stop_tx: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub audio: SharedAudio,
    /// Set when the stream reports an error it can't recover from, e.g. the
    /// device was unplugged (see `is_fatal`). The stream is dead from then
    /// on; a long-running caller should start a new one.
    pub failed: Arc<AtomicBool>,
}

/// Hard cap on one utterance. Longer holds keep what was said so far.
pub const MAX_SECONDS: usize = 60;

impl Recorder {
    /// Push-to-talk: the default mic, capped at MAX_SECONDS.
    pub fn start() -> Result<Self, String> {
        Self::start_from(Source::Mic, MAX_SECONDS)
    }

    /// Opens `source` and starts filling `audio` (16 kHz mono), keeping at
    /// most `cap_seconds` in the buffer: a caller that drains the buffer as
    /// it goes can record indefinitely. Returns once the stream is actually
    /// running, or with a readable error (no mic, mic blocked in Windows
    /// privacy settings, no output device, etc.).
    pub fn start_from(source: Source, cap_seconds: usize) -> Result<Self, String> {
        let audio: SharedAudio = Arc::new(Mutex::new(Vec::with_capacity(16_000 * 10)));
        let failed = Arc::new(AtomicBool::new(false));
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let buf = audio.clone();
        let failed_flag = failed.clone();
        let cap = cap_seconds * segment::TARGET_RATE as usize;

        let thread = std::thread::spawn(move || {
            let stream = match open_stream(source, buf, cap, failed_flag) {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            if let Err(e) = stream.play() {
                let error = if source == Source::Mic { mic_error(&e.to_string()) } else { system_audio_error(&e.to_string()) };
                let _ = ready_tx.send(Err(error));
                return;
            }
            let _ = ready_tx.send(Ok(()));
            let _ = stop_rx.recv(); // until stop() or the Recorder is dropped
            drop(stream);
        });

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { stop_tx, thread: Some(thread), audio, failed }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err("The microphone thread stopped unexpectedly.".into()),
        }
    }

    /// Closes the mic. What was recorded stays in `audio` for the
    /// transcriber's final pass.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let _ = self.stop_tx.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn mic_error(detail: &str) -> String {
    format!(
        "Couldn't use the microphone. Check that one is connected and that \
         Settings > Privacy & security > Microphone allows desktop apps. ({})",
        detail
    )
}

fn system_audio_error(detail: &str) -> String {
    format!("Couldn't listen to this computer's audio output. ({})", detail)
}

fn open_stream(source: Source, buf: SharedAudio, cap: usize, failed: Arc<AtomicBool>) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let (device, supported, error): (cpal::Device, cpal::SupportedStreamConfig, fn(&str) -> String) = match source {
        Source::Mic => {
            let device = host
                .default_input_device()
                .ok_or_else(|| "No microphone found. Connect one and try again.".to_string())?;
            let supported = device.default_input_config().map_err(|e| mic_error(&e.to_string()))?;
            (device, supported, mic_error)
        }
        Source::System => {
            // On WASAPI, an input stream built on an output device is a
            // loopback capture of that device, in its own mix format.
            let device = host
                .default_output_device()
                .ok_or_else(|| "No audio output device found, so there's no meeting audio to listen to.".to_string())?;
            let supported = device.default_output_config().map_err(|e| system_audio_error(&e.to_string()))?;
            (device, supported, system_audio_error)
        }
    };
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();

    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, config, buf, cap, failed),
        SampleFormat::I16 => build::<i16>(&device, config, buf, cap, failed),
        SampleFormat::I32 => build::<i32>(&device, config, buf, cap, failed),
        SampleFormat::U16 => build::<u16>(&device, config, buf, cap, failed),
        SampleFormat::U8 => build::<u8>(&device, config, buf, cap, failed),
        SampleFormat::I8 => build::<i8>(&device, config, buf, cap, failed),
        other => return Err(format!("Unsupported audio format: {:?}", other)),
    };
    stream.map_err(|e| error(&e))
}

/// Whether a stream error means the stream is dead and has to be reopened.
/// The rest are reported while the stream carries on:
/// - Xrun: a gap in the data. WASAPI flags one on loopback every time
///   playback starts or stops, so it's routine during a meeting, and at
///   worst one phrase loses a few milliseconds.
/// - DeviceChanged: the default device changed and cpal already moved the
///   stream over to the new one.
/// - RealtimeDenied: audio still flows, just without a priority boost.
fn is_fatal(kind: cpal::ErrorKind) -> bool {
    !matches!(
        kind,
        cpal::ErrorKind::Xrun | cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied
    )
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    buf: SharedAudio,
    cap: usize,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    let rate = config.sample_rate;

    device
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                let as_f32: Vec<f32> = data.iter().map(|&s| s.to_sample::<f32>()).collect();
                let mono = segment::to_mono(&as_f32, channels);
                // Resampling each callback's block independently is fine at
                // these block sizes: the only artifact is a one-sample seam.
                let resampled = segment::resample(&mono, rate, segment::TARGET_RATE);
                let mut b = buf.lock().unwrap();
                let room = cap.saturating_sub(b.len());
                b.extend_from_slice(&resampled[..resampled.len().min(room)]);
            },
            move |e| {
                if is_fatal(e.kind()) {
                    eprintln!("[voice] audio stream stopped: {}", e);
                    failed.store(true, Ordering::SeqCst);
                } else if cfg!(debug_assertions) && e.kind() != cpal::ErrorKind::Xrun {
                    eprintln!("[voice] audio stream notice: {}", e);
                }
            },
            None,
        )
        .map_err(|e| e.to_string())
}
