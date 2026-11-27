//! Audio spectrum analyser.

use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;
use std::time::Instant;
use tuxrgb_core::signal::{AUDIO_BANDS, AudioSpectrum};

pub const FFT_SIZE: usize = 2048;

pub struct Analyzer {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    ring: Vec<f32>,
    write: usize,
    since_last: usize,
    hop: usize,
    rate: u32,
    input: Vec<f32>,
    spectrum: Vec<realfft::num_complex::Complex<f32>>,
    scratch: Vec<realfft::num_complex::Complex<f32>>,
    edges: [usize; AUDIO_BANDS + 1],
    /// Automatic gain: slowly tracks the loudest band.
    peak: f32,
    smoothed: [f32; AUDIO_BANDS],
    bass_avg: f32,
    last_beat: Instant,
}

impl Analyzer {
    /// `rate` in Hz; `updates_per_sec` bounds the FFT rate (CPU budget).
    pub fn new(rate: u32, updates_per_sec: u32) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let window = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (FFT_SIZE - 1) as f32).cos())
            .collect();
        let spectrum = fft.make_output_vec();
        let scratch = fft.make_scratch_vec();
        let mut a = Self {
            fft,
            window,
            ring: vec![0.0; FFT_SIZE],
            write: 0,
            since_last: 0,
            hop: (rate / updates_per_sec.max(1)).max(128) as usize,
            rate,
            input: vec![0.0; FFT_SIZE],
            spectrum,
            scratch,
            edges: [0; AUDIO_BANDS + 1],
            peak: 0.05,
            smoothed: [0.0; AUDIO_BANDS],
            bass_avg: 0.0,
            last_beat: Instant::now(),
        };
        a.compute_edges();
        a
    }

    pub fn set_rate(&mut self, rate: u32, updates_per_sec: u32) {
        if rate != self.rate && rate > 0 {
            self.rate = rate;
            self.hop = (rate / updates_per_sec.max(1)).max(128) as usize;
            self.compute_edges();
        }
    }

    /// Log-spaced band edges between 30 Hz and 16 kHz.
    fn compute_edges(&mut self) {
        let bins = FFT_SIZE / 2;
        let hz_per_bin = self.rate as f32 / FFT_SIZE as f32;
        let (lo, hi) = (30.0f32, 16_000.0f32.min(self.rate as f32 / 2.0));
        for i in 0..=AUDIO_BANDS {
            let f = lo * (hi / lo).powf(i as f32 / AUDIO_BANDS as f32);
            self.edges[i] = ((f / hz_per_bin).round() as usize).clamp(1, bins);
        }
        // Guarantee at least one bin per band.
        for i in 1..=AUDIO_BANDS {
            if self.edges[i] <= self.edges[i - 1] {
                self.edges[i] = (self.edges[i - 1] + 1).min(bins);
            }
        }
    }

    /// Feeds mono samples; returns a new spectrum when one is due.
    pub fn push(&mut self, samples: &[f32]) -> Option<AudioSpectrum> {
        let mut out = None;
        for &s in samples {
            self.ring[self.write] = s;
            self.write = (self.write + 1) % FFT_SIZE;
            self.since_last += 1;
            if self.since_last >= self.hop {
                self.since_last = 0;
                out = Some(self.analyze());
            }
        }
        out
    }

    fn analyze(&mut self) -> AudioSpectrum {
        for i in 0..FFT_SIZE {
            let s = self.ring[(self.write + i) % FFT_SIZE];
            self.input[i] = s * self.window[i];
        }
        if self
            .fft
            .process_with_scratch(&mut self.input, &mut self.spectrum, &mut self.scratch)
            .is_err()
        {
            return AudioSpectrum::default();
        }
        let mut bands = [0.0f32; AUDIO_BANDS];
        let mut frame_peak = 0.0f32;
        for (b, band) in bands.iter_mut().enumerate() {
            let (a, z) = (self.edges[b], self.edges[b + 1].max(self.edges[b] + 1));
            let mut sum = 0.0;
            for bin in a..z.min(self.spectrum.len()) {
                sum += self.spectrum[bin].norm();
            }
            // Pink-noise tilt so highs are not invisible.
            let tilt = 1.0 + b as f32 / AUDIO_BANDS as f32 * 2.5;
            let mag = (sum / (z - a) as f32) * tilt / (FFT_SIZE as f32 * 0.25);
            *band = (1.0 + mag * 60.0).ln();
            frame_peak = frame_peak.max(*band);
        }
        // Time constants independent from the update rate.
        let per_sec = (self.rate as f32 / self.hop as f32).max(1.0);
        let peak_decay = 0.74f32.powf(1.0 / per_sec); // AGC: ~3 s release
        let fall = 0.85f32.powf(60.0 / per_sec); // bands fall smoothly
        self.peak = if frame_peak > self.peak {
            frame_peak
        } else {
            (self.peak * peak_decay).max(0.05)
        };
        for (b, band) in bands.iter_mut().enumerate() {
            let v = (*band / self.peak).clamp(0.0, 1.0);
            let s = &mut self.smoothed[b];
            *s = if v > *s {
                v
            } else {
                *s * fall + v * (1.0 - fall)
            };
            *band = *s;
        }
        let avg = |r: std::ops::Range<usize>| {
            let n = r.len().max(1) as f32;
            bands[r].iter().sum::<f32>() / n
        };
        let bass = avg(0..6);
        let mid = avg(6..20);
        let treble = avg(20..AUDIO_BANDS);
        let level = avg(0..AUDIO_BANDS);
        // Beat: bass well above its running average, debounced.
        let avg_k = 1.0 - 0.92f32.powf(60.0 / per_sec);
        self.bass_avg = self.bass_avg * (1.0 - avg_k) + bass * avg_k;
        let beat = bass > self.bass_avg * 1.35 + 0.05 && self.last_beat.elapsed().as_millis() > 180;
        if beat {
            self.last_beat = Instant::now();
        }
        AudioSpectrum {
            bands,
            bass,
            mid,
            treble,
            level,
            beat,
            captured_at: Instant::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_tone_lights_low_bands() {
        let rate = 48_000;
        let mut a = Analyzer::new(rate, 60);
        let tone: Vec<f32> = (0..rate as usize / 2)
            .map(|i| (std::f32::consts::TAU * 60.0 * i as f32 / rate as f32).sin() * 0.8)
            .collect();
        let mut last = None;
        for chunk in tone.chunks(512) {
            if let Some(s) = a.push(chunk) {
                last = Some(s);
            }
        }
        let s = last.expect("spectrum produced");
        let low = s.bands[..6].iter().cloned().fold(0.0, f32::max);
        let high = s.bands[24..].iter().cloned().fold(0.0, f32::max);
        assert!(low > high, "low={low} high={high}");
        assert!(s.bass > s.treble);
    }

    #[test]
    fn silence_is_quiet() {
        let mut a = Analyzer::new(48_000, 60);
        let mut last = None;
        for _ in 0..50 {
            if let Some(s) = a.push(&[0.0; 512]) {
                last = Some(s);
            }
        }
        assert!(last.unwrap().level < 0.05);
    }
}
