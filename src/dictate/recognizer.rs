//! Moonshine base (English, int8) through sherpa-onnx's offline recognizer, as
//! in Caw for Android. Moonshine is not streaming, so a hold's audio is cut at
//! pauses (caw_desktop::speech::segmenter) and each finished segment is decoded
//! while the key is still held; its text is banked. `finish` decodes what is
//! left and joins the banked segments as sentences.

use anyhow::{bail, Context, Result};
use caw_desktop::speech::segmenter::{self, PauseSegmenter, CHUNK_SAMPLES, FLOOR};
use caw_desktop::speech::text::join_sentences;
use sherpa_onnx::{OfflineMoonshineModelConfig, OfflineRecognizer, OfflineRecognizerConfig};
use std::collections::VecDeque;
use std::path::Path;
use std::time::Instant;

pub const SAMPLE_RATE: u32 = 16_000;

const PREPROCESSOR: &str = "preprocess.onnx";
const ENCODER: &str = "encode.int8.onnx";
const UNCACHED_DECODER: &str = "uncached_decode.int8.onnx";
const CACHED_DECODER: &str = "cached_decode.int8.onnx";
const TOKENS: &str = "tokens.txt";

pub struct Moonshine {
    engine: OfflineRecognizer,
}

impl Moonshine {
    pub fn load(dir: &Path, threads: i32) -> Result<Self> {
        // A missing model file makes the native code abort the whole process, so check first.
        for name in [PREPROCESSOR, ENCODER, UNCACHED_DECODER, CACHED_DECODER, TOKENS] {
            if !dir.join(name).is_file() {
                bail!("the speech model file {} is missing", dir.join(name).display());
            }
        }
        let path = |name: &str| Some(dir.join(name).to_string_lossy().into_owned());
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.moonshine = OfflineMoonshineModelConfig {
            preprocessor: path(PREPROCESSOR),
            encoder: path(ENCODER),
            uncached_decoder: path(UNCACHED_DECODER),
            cached_decoder: path(CACHED_DECODER),
            merged_decoder: None,
        };
        config.model_config.tokens = path(TOKENS);
        config.model_config.num_threads = threads;
        config.model_config.provider = Some("cpu".into());
        let engine = OfflineRecognizer::create(&config).context("the speech model did not load")?;
        Ok(Self { engine })
    }

    /// Decodes one segment of 16 kHz mono audio.
    pub fn decode(&self, audio: &[f32]) -> String {
        let stream = self.engine.create_stream();
        stream.accept_waveform(SAMPLE_RATE as i32, audio);
        self.engine.decode(&stream);
        stream.get_result().map(|r| r.text.trim().to_string()).unwrap_or_default()
    }
}

/// How long one segment took to decode.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub audio_secs: f32,
    pub decode_ms: u128,
}

/// One hold's recognition: feed 16 kHz audio as it comes, then `finish`.
#[derive(Default)]
pub struct Segmented {
    segmenter: PauseSegmenter,
    /// The current segment's 100 ms chunks, parallel to the segmenter's levels.
    chunks: VecDeque<Vec<f32>>,
    pending: Vec<f32>,
    texts: Vec<String>,
    pub timings: Vec<Timing>,
}

impl Segmented {
    pub fn feed(&mut self, model: &Moonshine, mut samples: &[f32]) {
        while !samples.is_empty() {
            let n = (CHUNK_SAMPLES - self.pending.len()).min(samples.len());
            self.pending.extend_from_slice(&samples[..n]);
            samples = &samples[n..];
            if self.pending.len() == CHUNK_SAMPLES {
                let chunk = std::mem::replace(&mut self.pending, Vec::with_capacity(CHUNK_SAMPLES));
                self.add_chunk(model, chunk);
            }
        }
    }

    /// No more audio: decodes what is left and returns the whole text.
    pub fn finish(&mut self, model: &Moonshine) -> String {
        if !self.pending.is_empty() {
            // The last part-chunk joins the segment; it is too short to start or end one by itself.
            self.chunks.push_back(std::mem::take(&mut self.pending));
        }
        if !self.chunks.is_empty() && self.has_sound() {
            self.decode(model, self.chunks.len());
        }
        self.chunks.clear();
        self.segmenter.reset();
        join_sentences(&self.texts)
    }

    fn add_chunk(&mut self, model: &Moonshine, chunk: Vec<f32>) {
        let level = segmenter::rms(&chunk);
        self.chunks.push_back(chunk);
        let step = self.segmenter.push(level);
        if step.is_nothing() {
            return;
        }
        if step.emit > 0 {
            self.decode(model, step.emit);
        }
        self.chunks.drain(..step.drop - step.emit);
    }

    fn has_sound(&self) -> bool {
        self.segmenter.has_sound() || self.chunks.iter().any(|c| segmenter::rms(c) >= FLOOR)
    }

    /// Decodes the first `count` chunks as one segment, removes them, and banks the text.
    fn decode(&mut self, model: &Moonshine, count: usize) {
        let audio: Vec<f32> = self.chunks.drain(..count).flatten().collect();
        let started = Instant::now();
        let text = model.decode(&audio);
        self.timings.push(Timing { audio_secs: audio.len() as f32 / SAMPLE_RATE as f32, decode_ms: started.elapsed().as_millis() });
        if !text.is_empty() {
            self.texts.push(text);
        }
    }
}
