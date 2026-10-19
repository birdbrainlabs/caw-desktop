//! Cuts one hold's audio into segments at pauses, so an offline recognizer
//! (Moonshine) can decode each finished segment while the hold goes on. Works
//! on 100 ms chunks and their RMS; the caller keeps the audio itself, in a
//! queue parallel to this one, and applies each [`Step`]. Same rules as Caw
//! for Android:
//!
//! - A chunk is quiet when its RMS is below max([`FLOOR`], [`RELATIVE`] x the
//!   average RMS of the loud chunks in this segment so far); otherwise loud.
//! - Speech = the stretch from the segment's first loud chunk to its last one.
//!   Once that is at least 1.0 s long and 0.8 s of quiet has followed it, the
//!   segment ends: it keeps 0.2 s of the quiet after the speech, the next
//!   segment starts with the last 0.2 s of the quiet, and the quiet between is
//!   dropped. Pauses of any length are fine; they only end a segment.
//! - A segment that reaches 25 s is cut at the quietest 0.4 s in its last 5 s:
//!   the first 0.2 s of that stretch ends this segment, the second 0.2 s starts
//!   the next one.
//! - Before a segment has any loud chunk, only its last 0.2 s is kept.

/// 100 ms at 16 kHz.
pub const CHUNK_SAMPLES: usize = 1_600;
pub const FLOOR: f32 = 0.004;
pub const RELATIVE: f32 = 0.15;
pub const MIN_SPEECH_CHUNKS: usize = 10;
pub const PAUSE_CHUNKS: usize = 8;
pub const PAD_CHUNKS: usize = 2;
pub const MAX_CHUNKS: usize = 250;
pub const FORCE_LOOKBACK_CHUNKS: usize = 50;

/// What the caller does with its buffered chunks after a push: hand the first
/// `emit` chunks to the recognizer as one finished segment (none if 0), then
/// throw away the first `drop` chunks (`drop >= emit`). The chunks left over
/// begin the next segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Step {
    pub emit: usize,
    pub drop: usize,
}

impl Step {
    pub fn is_nothing(&self) -> bool {
        self.emit == 0 && self.drop == 0
    }
}

#[derive(Default)]
pub struct PauseSegmenter {
    /// RMS of every chunk buffered for the current segment, oldest first.
    levels: Vec<f32>,
    loud_count: usize,
    loud_sum: f64,
    first_loud: Option<usize>,
    last_loud: Option<usize>,
}

impl PauseSegmenter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Chunks buffered for the current segment.
    pub fn len(&self) -> usize {
        self.levels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.levels.is_empty()
    }

    /// True if the buffered segment has any chunk above the silence floor (worth decoding).
    pub fn has_sound(&self) -> bool {
        self.levels.iter().any(|&l| l >= FLOOR)
    }

    pub fn reset(&mut self) {
        self.levels.clear();
        self.clear_stats();
    }

    /// Adds one 100 ms chunk's RMS; returns what to do with the buffered chunks.
    pub fn push(&mut self, rms: f32) -> Step {
        self.add(rms);
        let n = self.levels.len();

        let (Some(first), Some(last)) = (self.first_loud, self.last_loud) else {
            // No speech yet: keep only the last 0.2 s.
            if n > PAD_CHUNKS {
                let drop = n - PAD_CHUNKS;
                self.levels.drain(..drop);
                return Step { emit: 0, drop };
            }
            return Step::default();
        };

        let speech = last - first + 1;
        let quiet_after = n - 1 - last;
        if speech >= MIN_SPEECH_CHUNKS && quiet_after >= PAUSE_CHUNKS {
            return self.cut(last + 1 + PAD_CHUNKS, n - PAD_CHUNKS);
        }

        if n >= MAX_CHUNKS {
            // Quietest 0.4 s (4 chunks) whose start lies in the last 5 s.
            let window = PAD_CHUNKS * 2;
            let mut best = n - window;
            let mut best_sum = f64::MAX;
            for start in n.saturating_sub(FORCE_LOOKBACK_CHUNKS)..=(n - window) {
                let sum: f64 = self.levels[start..start + window].iter().map(|&x| x as f64).sum();
                if sum < best_sum {
                    // The first (earliest) quietest stretch wins ties.
                    best_sum = sum;
                    best = start;
                }
            }
            let boundary = best + PAD_CHUNKS;
            return self.cut(boundary, boundary);
        }
        Step::default()
    }

    fn cut(&mut self, emit: usize, drop: usize) -> Step {
        let rest = self.levels.split_off(drop);
        self.levels.clear();
        self.clear_stats();
        // The carried chunks begin the next segment; re-measure them under its own fresh threshold.
        for r in rest {
            self.add(r);
        }
        Step { emit, drop }
    }

    fn add(&mut self, rms: f32) {
        let average = if self.loud_count == 0 { 0.0 } else { RELATIVE * (self.loud_sum / self.loud_count as f64) as f32 };
        let threshold = FLOOR.max(average);
        self.levels.push(rms);
        if rms >= threshold {
            self.loud_count += 1;
            self.loud_sum += rms as f64;
            let at = self.levels.len() - 1;
            self.first_loud.get_or_insert(at);
            self.last_loud = Some(at);
        }
    }

    fn clear_stats(&mut self) {
        self.loud_count = 0;
        self.loud_sum = 0.0;
        self.first_loud = None;
        self.last_loud = None;
    }
}

/// Root mean square of a chunk of samples in -1..1.
pub fn rms(chunk: &[f32]) -> f32 {
    if chunk.is_empty() {
        return 0.0;
    }
    let ss: f64 = chunk.iter().map(|&x| (x as f64) * (x as f64)).sum();
    (ss / chunk.len() as f64).sqrt() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    const SPEECH: f32 = 0.05;
    const QUIET: f32 = 0.001;

    /// Drives the segmenter the way the recognizer does: a parallel queue of
    /// chunk ids stands in for the audio, and every step is applied to it.
    #[derive(Default)]
    struct Driver {
        seg: PauseSegmenter,
        buffer: VecDeque<usize>,
        segments: Vec<Vec<usize>>,
        next: usize,
    }

    impl Driver {
        fn push(&mut self, level: f32, times: usize) {
            for _ in 0..times {
                self.buffer.push_back(self.next);
                self.next += 1;
                let step = self.seg.push(level);
                if step.emit > 0 {
                    self.segments.push(self.buffer.iter().take(step.emit).copied().collect());
                }
                self.buffer.drain(..step.drop);
                assert_eq!(self.seg.len(), self.buffer.len());
            }
        }

        fn buffered(&self) -> Vec<usize> {
            self.buffer.iter().copied().collect()
        }
    }

    #[test]
    fn leading_silence_keeps_only_the_last_point_two_seconds() {
        let mut d = Driver::default();
        d.push(QUIET, 30);
        assert_eq!(d.buffered(), vec![28, 29]);
        assert!(!d.seg.has_sound());
        assert!(d.segments.is_empty());
    }

    #[test]
    fn pause_after_one_second_of_speech_cuts_with_point_two_seconds_each_side() {
        let mut d = Driver::default();
        d.push(QUIET, 5);
        d.push(SPEECH, 12);
        d.push(QUIET, 7);
        assert!(d.segments.is_empty());
        d.push(QUIET, 1);
        assert_eq!(d.segments.len(), 1);
        assert_eq!(d.segments[0], (3..=18).collect::<Vec<_>>());
        assert_eq!(d.buffered(), vec![23, 24]);
    }

    #[test]
    fn short_utterance_does_not_cut() {
        let mut d = Driver::default();
        d.push(SPEECH, 6);
        d.push(QUIET, 20);
        assert!(d.segments.is_empty());
        assert!(d.seg.has_sound());
        d.push(SPEECH, 6);
        d.push(QUIET, 8);
        assert_eq!(d.segments.len(), 1);
        assert_eq!(d.segments[0][0], 0);
    }

    #[test]
    fn short_gaps_inside_speech_do_not_cut() {
        let mut d = Driver::default();
        for _ in 0..5 {
            d.push(SPEECH, 8);
            d.push(QUIET, 7);
        }
        assert!(d.segments.is_empty());
    }

    #[test]
    fn long_pauses_only_end_a_segment() {
        // A 30 s pause mid-hold drops the quiet, not the speech on either side.
        let mut d = Driver::default();
        d.push(SPEECH, 15);
        d.push(QUIET, 300);
        d.push(SPEECH, 15);
        d.push(QUIET, 8);
        assert_eq!(d.segments.len(), 2);
        assert!(d.segments[0].contains(&0) && d.segments[0].contains(&14));
        assert!(d.segments[1].contains(&315) && d.segments[1].contains(&329));
    }

    #[test]
    fn threshold_follows_the_loudness_of_the_segment() {
        let mut d = Driver::default();
        d.push(0.2, 15);
        d.push(0.02, 8);
        assert_eq!(d.segments.len(), 1);
        let mut soft = Driver::default();
        soft.push(0.02, 15);
        soft.push(0.02, 8);
        assert!(soft.segments.is_empty());
    }

    #[test]
    fn floor_applies_when_everything_is_quiet() {
        let mut d = Driver::default();
        d.push(0.003, 40);
        assert!(d.segments.is_empty());
        assert_eq!(d.buffer.len(), 2);
    }

    #[test]
    fn force_cut_at_twenty_five_seconds_at_the_quietest_spot() {
        let mut d = Driver::default();
        for i in 0..250 {
            d.push(if (220..=223).contains(&i) { 0.01 } else { SPEECH }, 1);
        }
        assert_eq!(d.segments.len(), 1);
        assert_eq!(d.segments[0], (0..=221).collect::<Vec<_>>());
        assert_eq!(d.buffered(), (222..=249).collect::<Vec<_>>());
    }

    #[test]
    fn force_cut_never_looks_before_the_last_five_seconds() {
        let mut d = Driver::default();
        for i in 0..250 {
            d.push(if (100..=103).contains(&i) { 0.006 } else { SPEECH }, 1);
        }
        assert_eq!(d.segments.len(), 1);
        assert!(d.segments[0].len() >= 200 && d.segments[0].len() <= 248);
    }

    #[test]
    fn segment_after_a_force_cut_can_still_end_at_a_pause() {
        let mut d = Driver::default();
        d.push(SPEECH, 260);
        assert_eq!(d.segments.len(), 1);
        d.push(QUIET, 8);
        assert_eq!(d.segments.len(), 2);
        let all: Vec<usize> = d.segments.concat();
        let unique: std::collections::BTreeSet<_> = all.iter().copied().collect();
        assert_eq!(all.len(), unique.len());
        assert!((0..260).all(|i| unique.contains(&i)));
    }

    #[test]
    fn reset_forgets_everything() {
        let mut d = Driver::default();
        d.push(SPEECH, 5);
        d.seg.reset();
        assert_eq!(d.seg.len(), 0);
        assert!(!d.seg.has_sound());
    }
}
