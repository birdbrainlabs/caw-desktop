//! Microphone capture from the default input (PipeWire, PulseAudio or ALSA,
//! through ALSA). Asks for 16 kHz mono; if the device can't do that, takes
//! its own rate and the speech worker resamples. Samples arrive as mono f32
//! in -1..1 on cpal's thread; the stream lives on a thread of its own, so it
//! can be stopped from anywhere.

use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig};
use std::sync::mpsc;
use std::thread::JoinHandle;

pub struct Capture {
    /// The rate samples are delivered at.
    pub rate: u32,
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    /// Stops the microphone; once this returns, no more samples arrive.
    pub fn stop(mut self) {
        self.halt();
    }

    fn halt(&mut self) {
        let _ = self.stop.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.halt();
    }
}

/// Starts the default microphone; `on_audio` gets (rate, mono samples).
pub fn start(on_audio: impl FnMut(u32, Vec<f32>) + Send + 'static) -> Result<Capture> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<u32>>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let thread = std::thread::Builder::new().name("caw-mic".into()).spawn(move || {
        let stream = match open(on_audio) {
            Ok((stream, rate)) => {
                let _ = ready_tx.send(Ok(rate));
                stream
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
        };
        // Wait for stop (or for the Capture to be dropped), then close the stream.
        let _ = stop_rx.recv();
        drop(stream);
    })?;
    let rate = ready_rx.recv().map_err(|_| anyhow!("the microphone thread stopped"))??;
    Ok(Capture { rate, stop: stop_tx, thread: Some(thread) })
}

fn open(mut on_audio: impl FnMut(u32, Vec<f32>) + Send + 'static) -> Result<(cpal::Stream, u32)> {
    let host = cpal::default_host();
    let device = host.default_input_device().context("no microphone found")?;
    let wanted = SampleRate(super::recognizer::SAMPLE_RATE);
    let preferred = device.supported_input_configs().ok().and_then(|mut ranges| {
        ranges.find(|r| {
            r.channels() == 1 && r.min_sample_rate() <= wanted && r.max_sample_rate() >= wanted && matches!(r.sample_format(), SampleFormat::F32 | SampleFormat::I16)
        })
    });
    let supported = match preferred {
        Some(range) => range.with_sample_rate(wanted),
        None => device.default_input_config().context("reading the microphone's settings")?,
    };
    let format = supported.sample_format();
    let config: StreamConfig = supported.config();
    let rate = config.sample_rate.0;
    let channels = config.channels as usize;
    let on_error = |e| eprintln!("caw-desktop: microphone: {e}");

    fn mono<T: Copy>(data: &[T], channels: usize, to_f32: impl Fn(T) -> f32) -> Vec<f32> {
        if channels <= 1 {
            return data.iter().map(|&s| to_f32(s)).collect();
        }
        data.chunks(channels).map(|frame| frame.iter().map(|&s| to_f32(s)).sum::<f32>() / channels as f32).collect()
    }

    let stream = match format {
        SampleFormat::F32 => device.build_input_stream(&config, move |data: &[f32], _: &_| on_audio(rate, mono(data, channels, |s| s)), on_error, None),
        SampleFormat::I16 => device.build_input_stream(&config, move |data: &[i16], _: &_| on_audio(rate, mono(data, channels, |s| s as f32 / 32768.0)), on_error, None),
        SampleFormat::I32 => device.build_input_stream(&config, move |data: &[i32], _: &_| on_audio(rate, mono(data, channels, |s| s as f32 / 2147483648.0)), on_error, None),
        SampleFormat::U16 => device.build_input_stream(&config, move |data: &[u16], _: &_| on_audio(rate, mono(data, channels, |s| (s as f32 - 32768.0) / 32768.0)), on_error, None),
        other => bail!("the microphone sends {other} samples, which Caw can't read yet"),
    }
    .context("opening the microphone")?;
    stream.play().context("starting the microphone")?;
    Ok((stream, rate))
}
