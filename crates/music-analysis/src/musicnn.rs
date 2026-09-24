//! Fixed MusiCNN frame features shared by optional voice inference and model probes.
//! Framing, decoding and model-specific patch sizes belong to the caller.
//! Numerical reference provenance and regeneration live beside the test fixture.

use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

pub(crate) const SAMPLE_RATE: u32 = 16_000;
pub(crate) const FRAME_SIZE: usize = 512;
pub(crate) const MEL_BANDS: usize = 96;
const SPECTRUM_BINS: usize = FRAME_SIZE / 2 + 1;

pub(crate) struct MusicNnPreprocessor {
    fft: Arc<dyn Fft<f32>>,
    window: [f32; FRAME_SIZE],
    filters: Vec<[f32; SPECTRUM_BINS]>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl MusicNnPreprocessor {
    pub(crate) fn new() -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FRAME_SIZE);
        let window = std::array::from_fn(|index| {
            0.5 - 0.5 * (std::f32::consts::TAU * index as f32 / (FRAME_SIZE - 1) as f32).cos()
        });
        let scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()];
        Self {
            fft,
            window,
            filters: mel_filter_bank(),
            spectrum: vec![Complex::new(0.0, 0.0); FRAME_SIZE],
            scratch,
        }
    }

    pub(crate) fn transform(&mut self, frame: &[f32; FRAME_SIZE]) -> [f32; MEL_BANDS] {
        for (index, sample) in frame.iter().enumerate() {
            self.spectrum[index] = Complex::new(*sample * self.window[index], 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.spectrum, &mut self.scratch);
        let magnitudes =
            std::array::from_fn::<_, SPECTRUM_BINS, _>(|index| self.spectrum[index].norm());
        std::array::from_fn(|band| {
            let energy = magnitudes
                .iter()
                .zip(&self.filters[band])
                .map(|(magnitude, weight)| magnitude * magnitude * weight)
                .sum::<f32>();
            energy.mul_add(10_000.0, 1.0).log10()
        })
    }
}

fn mel_filter_bank() -> Vec<[f32; SPECTRUM_BINS]> {
    let edges = mel_edges();
    let bin_hz = SAMPLE_RATE as f32 / FRAME_SIZE as f32;
    let mut filters = Vec::with_capacity(MEL_BANDS);
    for band in 0..MEL_BANDS {
        let left = edges[band];
        let center = edges[band + 1];
        let right = edges[band + 2];
        let rising = center - left;
        let falling = right - center;
        let area = (rising + falling) / 2.0;
        let mut coefficients = [0.0; SPECTRUM_BINS];
        let first = (left / bin_hz).ceil().max(0.0) as usize;
        let last = (right / bin_hz).floor().max(0.0) as usize;
        for (index, coefficient) in coefficients
            .iter_mut()
            .enumerate()
            .take(last.min(SPECTRUM_BINS - 1).saturating_add(1))
            .skip(first)
        {
            let frequency = index as f32 * bin_hz;
            let triangle = if frequency < center {
                (frequency - left) / rising
            } else {
                (right - frequency) / falling
            };
            *coefficient = triangle / area;
        }
        filters.push(coefficients);
    }
    filters
}

fn mel_edges() -> [f32; MEL_BANDS + 2] {
    let low = hz_to_slaney_mel(0.0);
    let high = hz_to_slaney_mel(SAMPLE_RATE as f32 / 2.0);
    let increment = (high - low) / (MEL_BANDS + 1) as f32;
    let mut mel = low;
    std::array::from_fn(|_| {
        let frequency = slaney_mel_to_hz(mel);
        mel += increment;
        frequency
    })
}

fn hz_to_slaney_mel(frequency: f32) -> f32 {
    const MIN_LOG_HZ: f32 = 1_000.0;
    const LINEAR_SLOPE: f32 = 3.0 / 200.0;
    if frequency < MIN_LOG_HZ {
        frequency * LINEAR_SLOPE
    } else {
        const MIN_LOG_MEL: f32 = MIN_LOG_HZ * LINEAR_SLOPE;
        let log_step = 6.4_f32.ln() / 27.0;
        MIN_LOG_MEL + (frequency / MIN_LOG_HZ).ln() / log_step
    }
}

fn slaney_mel_to_hz(mel: f32) -> f32 {
    const MIN_LOG_HZ: f32 = 1_000.0;
    const LINEAR_SLOPE: f32 = 3.0 / 200.0;
    const MIN_LOG_MEL: f32 = MIN_LOG_HZ * LINEAR_SLOPE;
    if mel < MIN_LOG_MEL {
        mel / LINEAR_SLOPE
    } else {
        let log_step = 6.4_f32.ln() / 27.0;
        MIN_LOG_HZ * ((mel - MIN_LOG_MEL) * log_step).exp()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::f32::consts::TAU;

    use serde_json::Value;

    use super::*;

    #[test]
    fn slaney_scale_round_trips_and_edges_are_strictly_increasing() {
        for frequency in [0.0, 100.0, 999.0, 1_000.0, 4_000.0, 8_000.0] {
            let round_trip = slaney_mel_to_hz(hz_to_slaney_mel(frequency));
            assert!((round_trip - frequency).abs() < 0.01);
        }
        let edges = mel_edges();
        assert!((edges[0] - 0.0).abs() < f32::EPSILON);
        assert!((edges[MEL_BANDS + 1] - 8_000.0).abs() < 0.02);
        assert!(edges.windows(2).all(|edge| edge[0] < edge[1]));
    }

    #[test]
    fn preprocessing_maps_silence_to_zero_and_tone_to_its_filter() -> Result<(), Box<dyn Error>> {
        let mut preprocessor = MusicNnPreprocessor::new();
        let silence = preprocessor.transform(&[0.0; FRAME_SIZE]);
        assert_eq!(silence, [0.0; MEL_BANDS]);

        let tone =
            std::array::from_fn(|index| (TAU * 1_000.0 * index as f32 / SAMPLE_RATE as f32).sin());
        let bands = preprocessor.transform(&tone);
        let strongest = bands
            .iter()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(right.1))
            .map(|(index, _)| index)
            .ok_or("no strongest mel band")?;
        let edges = mel_edges();
        assert!(edges[strongest] <= 1_000.0);
        assert!(edges[strongest + 2] >= 1_000.0);
        Ok(())
    }

    #[test]
    fn preprocessing_matches_pinned_essentia_reference() -> Result<(), Box<dyn Error>> {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/musicnn-reference-v1.json"))?;
        assert_eq!(fixture["schema_version"], "musicnn-frame-reference/v1");
        assert_eq!(fixture["frame_size"], FRAME_SIZE);
        assert_eq!(fixture["mel_bands"], MEL_BANDS);
        assert_eq!(fixture["max_absolute_error"], 0.0001);
        assert_eq!(fixture["reference"]["package"], "essentia.js");
        assert_eq!(fixture["reference"]["package_version"], "0.1.3");
        assert_eq!(
            fixture["reference"]["artifact_sha256"],
            "7e0a2b5507199e8162c4ed090d38518de0c7faa070ce35d1593e7631b201014d"
        );
        let cases = fixture["cases"]
            .as_array()
            .ok_or("missing reference cases")?;
        assert_eq!(cases.len(), 12);
        let mut preprocessor = MusicNnPreprocessor::new();
        let mut failures = Vec::new();
        // Reusing the buffers in both orders must not leak previous-frame state.
        for case in cases.iter().chain(cases.iter().rev()) {
            let samples: Vec<f32> = serde_json::from_value(case["input"].clone())?;
            let frame: [f32; FRAME_SIZE] =
                samples.try_into().map_err(|_| "invalid reference frame")?;
            let expected: Vec<f32> = serde_json::from_value(case["bands"].clone())?;
            assert_eq!(expected.len(), MEL_BANDS);
            let actual = preprocessor.transform(&frame);
            let max_error = actual
                .iter()
                .zip(&expected)
                .map(|(actual, expected)| {
                    assert!(actual.is_finite() && expected.is_finite());
                    (actual - expected).abs()
                })
                .fold(0.0_f32, f32::max);
            println!("{}: maximum absolute error {max_error:.9}", case["name"]);
            if max_error > 0.000_1 {
                failures.push(format!("{}: {max_error}", case["name"]));
            }
        }
        assert!(
            failures.is_empty(),
            "reference mismatch: {}",
            failures.join(", ")
        );
        Ok(())
    }
}
