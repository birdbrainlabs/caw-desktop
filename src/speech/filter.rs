//! An 80 Hz high-pass filter for microphone audio. Some laptop microphones
//! start with a large, slowly fading DC offset each time they wake up (about
//! 0.7 s, near full scale), which would otherwise sound like loud speech to
//! the pause segmenter and the model. Speech has almost nothing below 80 Hz.

/// Two cascaded Butterworth biquads (4th order), 80 Hz at 16 kHz.
#[derive(Default, Clone)]
pub struct HighPass {
    stages: [Biquad; 2],
}

#[derive(Default, Clone, Copy)]
struct Biquad {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

const CUTOFF_HZ: f32 = 80.0;
const SAMPLE_RATE: f32 = 16_000.0;

struct Coefficients {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

fn coefficients() -> Coefficients {
    // RBJ cookbook high-pass, Q = 1/sqrt(2).
    let w0 = 2.0 * std::f32::consts::PI * CUTOFF_HZ / SAMPLE_RATE;
    let alpha = w0.sin() / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
    let cos = w0.cos();
    let a0 = 1.0 + alpha;
    Coefficients { b0: (1.0 + cos) / 2.0 / a0, b1: -(1.0 + cos) / a0, b2: (1.0 + cos) / 2.0 / a0, a1: -2.0 * cos / a0, a2: (1.0 - alpha) / a0 }
}

impl HighPass {
    pub fn new() -> Self {
        Self::default()
    }

    /// Filters 16 kHz samples in place.
    pub fn process(&mut self, samples: &mut [f32]) {
        let c = coefficients();
        for s in samples.iter_mut() {
            let mut v = *s;
            for st in self.stages.iter_mut() {
                let y = c.b0 * v + c.b1 * st.x1 + c.b2 * st.x2 - c.a1 * st.y1 - c.a2 * st.y2;
                st.x2 = st.x1;
                st.x1 = v;
                st.y2 = st.y1;
                st.y1 = y;
                v = y;
            }
            *s = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::segmenter::rms;

    #[test]
    fn removes_a_fading_offset_and_keeps_speech_frequencies() {
        // A -0.45 offset fading over 0.6 s, plus a 300 Hz tone at 0.1.
        let n = 16_000;
        let tone = |i: usize| 0.1 * (2.0 * std::f32::consts::PI * 300.0 * i as f32 / 16_000.0).sin();
        let mut x: Vec<f32> = (0..n).map(|i| -0.45 * (-(i as f32) / 3_000.0).exp() + tone(i)).collect();
        HighPass::new().process(&mut x);
        // After the first 0.1 s, only the tone is left.
        let want: Vec<f32> = (1_600..n).map(tone).collect();
        let got = &x[1_600..];
        assert!((rms(got) - rms(&want)).abs() < 0.01, "{} vs {}", rms(got), rms(&want));
        let mean: f32 = got.iter().sum::<f32>() / got.len() as f32;
        assert!(mean.abs() < 0.002, "mean {mean}");
    }

    #[test]
    fn works_across_calls() {
        let mut whole: Vec<f32> = (0..3_200).map(|i| ((i * 7) % 13) as f32 / 13.0 - 0.3).collect();
        let mut parts = whole.clone();
        HighPass::new().process(&mut whole);
        let mut f = HighPass::new();
        let (a, b) = parts.split_at_mut(1_000);
        f.process(a);
        f.process(b);
        assert_eq!(whole, parts);
    }
}
