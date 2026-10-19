//! Inputs for reactive effects (audio visualiser, screen sync).
//!
//! Capture threads write into [`SharedInputs`]; the engine only takes a short
//! lock once per frame to read the latest value.

use crate::color::Rgb;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};
use std::time::Instant;

/// Number of log-spaced frequency bands produced by the audio analyser.
pub const AUDIO_BANDS: usize = 32;

#[derive(Clone, Debug)]
pub struct AudioSpectrum {
    /// Normalised (0..1) band levels from low to high frequencies.
    pub bands: [f32; AUDIO_BANDS],
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    /// Overall level, 0..1.
    pub level: f32,
    /// `true` for the frame where a bass onset was detected.
    pub beat: bool,
    pub captured_at: Instant,
}

impl Default for AudioSpectrum {
    fn default() -> Self {
        Self {
            bands: [0.0; AUDIO_BANDS],
            bass: 0.0,
            mid: 0.0,
            treble: 0.0,
            level: 0.0,
            beat: false,
            captured_at: Instant::now(),
        }
    }
}

impl AudioSpectrum {
    /// Linear interpolation into the band array, `x` in `0..=1`.
    pub fn sample(&self, x: f32) -> f32 {
        let pos = x.clamp(0.0, 1.0) * (AUDIO_BANDS - 1) as f32;
        let i = pos.floor() as usize;
        let j = (i + 1).min(AUDIO_BANDS - 1);
        let t = pos - i as f32;
        self.bands[i] * (1.0 - t) + self.bands[j] * t
    }
}

/// A heavily downscaled screen frame (e.g. 64×36).
#[derive(Clone, Debug)]
pub struct ScreenFrame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<Rgb>,
    pub captured_at: Instant,
}

impl ScreenFrame {
    /// Nearest-neighbour sample, `x`/`y` in `0..=1`.
    pub fn sample(&self, x: f32, y: f32) -> Rgb {
        if self.width == 0 || self.height == 0 || self.pixels.is_empty() {
            return Rgb::BLACK;
        }
        let px =
            ((x.clamp(0.0, 1.0) * (self.width - 1) as f32).round() as usize).min(self.width - 1);
        let py =
            ((y.clamp(0.0, 1.0) * (self.height - 1) as f32).round() as usize).min(self.height - 1);
        self.pixels[py * self.width + px]
    }

    /// Average colour of a rectangular region (normalised coordinates).
    pub fn average(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Rgb {
        if self.width == 0 || self.height == 0 || self.pixels.is_empty() {
            return Rgb::BLACK;
        }
        let to_px = |v: f32, max: usize| ((v.clamp(0.0, 1.0) * max as f32) as usize).min(max - 1);
        let (ax, bx) = (to_px(x0.min(x1), self.width), to_px(x0.max(x1), self.width));
        let (ay, by) = (
            to_px(y0.min(y1), self.height),
            to_px(y0.max(y1), self.height),
        );
        let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
        for y in ay..=by {
            for x in ax..=bx {
                let c = self.pixels[y * self.width + x];
                r += c.r as u32;
                g += c.g as u32;
                b += c.b as u32;
                n += 1;
            }
        }
        let n = n.max(1);
        Rgb::new((r / n) as u8, (g / n) as u8, (b / n) as u8)
    }
}

impl ScreenFrame {
    /// Average colour of the area an LED covers: centre `(x, y)`, half
    /// extents `(rx, ry)`, all normalised. Never smaller than one cell.
    pub fn average_area(&self, x: f32, y: f32, rx: f32, ry: f32) -> Rgb {
        let rx = rx.max(0.5 / self.width.max(1) as f32);
        let ry = ry.max(0.5 / self.height.max(1) as f32);
        self.average(x - rx, y - ry, x + rx, y + ry)
    }
}

/// Hardware sensors for the "Moniteur matériel" effect (Hardware Sync).
/// Every value is optional: not every machine exposes every sensor.
#[derive(Clone, Debug)]
pub struct HardwareSample {
    /// CPU package temperature, °C.
    pub cpu_temp: Option<f32>,
    /// CPU load, `0..=1`.
    pub cpu_load: Option<f32>,
    /// GPU temperature, °C.
    pub gpu_temp: Option<f32>,
    /// GPU load, `0..=1`.
    pub gpu_load: Option<f32>,
    /// Memory in use, `0..=1`.
    pub mem_used: Option<f32>,
    pub captured_at: Instant,
}

impl Default for HardwareSample {
    fn default() -> Self {
        Self {
            cpu_temp: None,
            cpu_load: None,
            gpu_temp: None,
            gpu_load: None,
            mem_used: None,
            captured_at: Instant::now(),
        }
    }
}

/// Latest audio / screen / sensor data, shared between capture threads and
/// the engine.
///
/// Every access goes through the helpers below: they never fail on a
/// poisoned lock (a panic in one capture thread must not freeze the
/// reactive effects for the rest of the session), and the `try_set_*`
/// variants never wait, for the real-time PipeWire callbacks.
#[derive(Clone, Default)]
pub struct SharedInputs {
    pub audio: Arc<Mutex<Option<AudioSpectrum>>>,
    pub screen: Arc<Mutex<Option<Arc<ScreenFrame>>>>,
    /// Frame rendered by the GLSL shader renderer (same format as screen).
    pub shader: Arc<Mutex<Option<Arc<ScreenFrame>>>>,
    pub hardware: Arc<Mutex<Option<HardwareSample>>>,
}

/// Locks `m` even if a previous holder panicked (the data is plain values,
/// always consistent).
#[inline]
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Non-blocking variant of [`lock`]: `None` only while another thread holds
/// the lock right now.
#[inline]
pub fn try_lock<T>(m: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    match m.try_lock() {
        Ok(g) => Some(g),
        Err(TryLockError::Poisoned(p)) => Some(p.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

impl SharedInputs {
    pub fn latest_shader(&self) -> Option<Arc<ScreenFrame>> {
        lock(&self.shader).clone()
    }

    pub fn latest_hardware(&self) -> Option<HardwareSample> {
        lock(&self.hardware).clone()
    }

    pub fn latest_audio(&self) -> Option<AudioSpectrum> {
        lock(&self.audio).clone()
    }

    pub fn latest_screen(&self) -> Option<Arc<ScreenFrame>> {
        lock(&self.screen).clone()
    }

    pub fn set_hardware(&self, sample: Option<HardwareSample>) {
        *lock(&self.hardware) = sample;
    }

    pub fn set_audio(&self, spectrum: Option<AudioSpectrum>) {
        *lock(&self.audio) = spectrum;
    }

    /// Real-time safe: skips this update if the engine is reading the slot
    /// (the next spectrum comes a few milliseconds later).
    pub fn try_set_audio(&self, spectrum: AudioSpectrum) {
        if let Some(mut slot) = try_lock(&self.audio) {
            *slot = Some(spectrum);
        }
    }

    pub fn set_screen(&self, frame: Option<Arc<ScreenFrame>>) {
        *lock(&self.screen) = frame;
    }

    pub fn set_shader(&self, frame: Option<Arc<ScreenFrame>>) {
        *lock(&self.shader) = frame;
    }

    /// Publishes a new screen-like frame in `slot`. The pixel buffer comes
    /// from `spare` (the frame published before the current one) when nobody
    /// holds it any more, so a capture thread does not allocate a new buffer
    /// for every frame; the slot is never left empty in between (the engine
    /// always finds a frame). `fill` writes the pixels into an empty vector.
    pub fn publish_frame(
        slot: &Mutex<Option<Arc<ScreenFrame>>>,
        spare: &mut Option<Arc<ScreenFrame>>,
        width: usize,
        height: usize,
        fill: impl FnOnce(&mut Vec<Rgb>),
    ) {
        let mut pixels = spare
            .take()
            .and_then(|f| Arc::try_unwrap(f).ok())
            .map(|f| f.pixels)
            .unwrap_or_default();
        pixels.clear();
        fill(&mut pixels);
        let frame = Arc::new(ScreenFrame {
            width,
            height,
            pixels,
            captured_at: Instant::now(),
        });
        let previous = lock(slot).replace(frame);
        *spare = previous;
    }

    pub fn clear(&self) {
        *lock(&self.audio) = None;
        *lock(&self.screen) = None;
        *lock(&self.shader) = None;
        *lock(&self.hardware) = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poisoned_slots_keep_working() {
        let inputs = SharedInputs::default();
        let i = inputs.clone();
        let _ = std::thread::spawn(move || {
            let _g = i.audio.lock().unwrap();
            panic!("capture thread crashed while holding the lock");
        })
        .join();
        assert!(inputs.audio.is_poisoned());
        inputs.try_set_audio(AudioSpectrum::default());
        assert!(inputs.latest_audio().is_some());
        inputs.set_audio(None);
        assert!(inputs.latest_audio().is_none());
    }

    #[test]
    fn published_frames_reuse_their_buffer() {
        let inputs = SharedInputs::default();
        let mut spare = None;
        let publish = |spare: &mut Option<Arc<ScreenFrame>>, px: [Rgb; 2]| {
            SharedInputs::publish_frame(&inputs.screen, spare, 2, 1, |p| p.extend(px));
        };
        publish(&mut spare, [Rgb::WHITE, Rgb::BLACK]);
        let ptr = inputs.latest_screen().unwrap().pixels.as_ptr();
        publish(&mut spare, [Rgb::BLACK, Rgb::WHITE]);
        // The first buffer is now the spare: the third frame reuses it.
        publish(&mut spare, [Rgb::WHITE, Rgb::WHITE]);
        let third = inputs.latest_screen().unwrap();
        assert_eq!(third.pixels, vec![Rgb::WHITE, Rgb::WHITE]);
        assert_eq!(third.pixels.as_ptr(), ptr, "buffer reused");
        // A frame still held elsewhere (the engine) is never overwritten.
        let held = inputs.latest_screen().unwrap();
        publish(&mut spare, [Rgb::BLACK, Rgb::BLACK]);
        publish(&mut spare, [Rgb::WHITE, Rgb::BLACK]);
        assert_eq!(held.pixels, vec![Rgb::WHITE, Rgb::WHITE]);
        assert_eq!(
            inputs.latest_screen().unwrap().pixels,
            vec![Rgb::WHITE, Rgb::BLACK]
        );
    }
}
