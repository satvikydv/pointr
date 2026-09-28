//! Microphone capture for push-to-talk. The mic is opened only while the
//! key is held and closed the moment it's released: nothing records in the
//! background.
//!
//! The cpal stream lives on its own thread for its whole life, because a
//! stream isn't guaranteed to be movable between threads on every backend.
//! Samples land in a shared buffer as 16 kHz mono f32 (converted in the
//! callback, so readers never deal with device formats or rates).

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use super::segment;

pub type SharedAudio = Arc<Mutex<Vec<f32>>>;

pub struct Recorder {
    stop_tx: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub audio: SharedAudio,
}

/// Hard cap on one utterance. Longer holds keep what was said so far.
pub const MAX_SECONDS: usize = 60;

impl Recorder {
    /// Opens the default input device and starts filling `audio`. Returns
    /// once the stream is actually running, or with a readable error
    /// (no mic, mic blocked in Windows privacy settings, etc.).
    pub fn start() -> Result<Self, String> {
        let audio: SharedAudio = Arc::new(Mutex::new(Vec::with_capacity(16_000 * 10)));
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let buf = audio.clone();

        let thread = std::thread::spawn(move || {
            let stream = match open_stream(buf) {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            if let Err(e) = stream.play() {
                let _ = ready_tx.send(Err(mic_error(&e.to_string())));
                return;
            }
            let _ = ready_tx.send(Ok(()));
            let _ = stop_rx.recv(); // until stop() or the Recorder is dropped
            drop(stream);
        });

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { stop_tx, thread: Some(thread), audio }),
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

fn open_stream(buf: SharedAudio) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "No microphone found. Connect one and try again.".to_string())?;
    let supported = device
        .default_input_config()
        .map_err(|e| mic_error(&e.to_string()))?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();

    match format {
        SampleFormat::F32 => build::<f32>(&device, config, buf),
        SampleFormat::I16 => build::<i16>(&device, config, buf),
        SampleFormat::I32 => build::<i32>(&device, config, buf),
        SampleFormat::U16 => build::<u16>(&device, config, buf),
        SampleFormat::U8 => build::<u8>(&device, config, buf),
        SampleFormat::I8 => build::<i8>(&device, config, buf),
        other => Err(format!("Unsupported microphone format: {:?}", other)),
    }
}

fn build<T>(device: &cpal::Device, config: cpal::StreamConfig, buf: SharedAudio) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    let rate = config.sample_rate;
    let cap = MAX_SECONDS * segment::TARGET_RATE as usize;

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
            |e| eprintln!("[voice] mic stream error: {}", e),
            None,
        )
        .map_err(|e| mic_error(&e.to_string()))
}
