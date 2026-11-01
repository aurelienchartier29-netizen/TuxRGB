//! WASAPI loopback capture of the default output ("what you hear") – the
//! Windows counterpart of the PipeWire monitor capture (Windows build).
//!
//! The same analysis as on Linux runs on the samples: real FFT, log bands,
//! AGC and beat detection ([`Analyzer`]). The capture follows the default
//! output device: a change of headset / speakers in Windows reopens it, and
//! a device that disappears is retried until it comes back.

use crate::CaptureError;
use crate::analyzer::Analyzer;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tuxrgb_core::signal::SharedInputs;
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE, eCapture, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};

/// Spectra per second, as on Linux.
const UPDATES_PER_SEC: u32 = 100;
/// How often the capture thread drains WASAPI (loopback streams are not
/// event driven on every Windows version: polling is the portable way).
const POLL: Duration = Duration::from_millis(5);
/// The default output device is checked this often.
const DEVICE_CHECK: Duration = Duration::from_secs(2);
/// Shared-mode buffer asked from WASAPI (100 ns units): 50 ms.
const BUFFER_HNS: i64 = 500_000;

/// Developer check (`TUXRGB_DEV_WASAPI_INPUT=1`): reads the default
/// *input* device instead of the output loopback – Wine has no loopback,
/// this exercises the same capture and analysis code there.
fn endpoint() -> (windows::Win32::Media::Audio::EDataFlow, u32) {
    if std::env::var_os("TUXRGB_DEV_WASAPI_INPUT").is_some() {
        (eCapture, 0)
    } else {
        (eRender, AUDCLNT_STREAMFLAGS_LOOPBACK)
    }
}

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// Running audio capture; dropping it stops the WASAPI thread.
pub struct AudioCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AudioCapture {
    /// Starts capturing. Returns once the WASAPI stream is open (or failed).
    pub fn start(inputs: SharedInputs) -> Result<Self, CaptureError> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std_mpsc::channel::<Result<(), String>>();
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("tuxrgb-audio".into())
            .spawn(move || {
                if let Err(e) = run(inputs, &flag, &ready_tx) {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| CaptureError::PipeWire(e.to_string()))?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                stop,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                tracing::warn!("audio capture (WASAPI) unavailable: {e}");
                Err(CaptureError::PipeWire(e))
            }
            Err(_) => {
                tracing::warn!("audio capture (WASAPI) did not start in time");
                stop.store(true, Ordering::Relaxed);
                Err(CaptureError::PipeWire(
                    "le service audio de Windows ne répond pas".into(),
                ))
            }
        }
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

/// COM for this thread, released when the thread ends.
struct Com;

impl Com {
    fn init() -> Result<Self, String> {
        // SAFETY: plain COM initialisation of the capture thread.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|e| e.to_string())?;
        Ok(Self)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: balances the successful CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SampleFormat {
    F32,
    I16,
    I24,
    I32,
}

impl SampleFormat {
    fn bytes(self) -> usize {
        match self {
            Self::F32 | Self::I32 => 4,
            Self::I16 => 2,
            Self::I24 => 3,
        }
    }

    /// One sample at the start of `b`, in `-1.0..=1.0`.
    fn read(self, b: &[u8]) -> f32 {
        match self {
            Self::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            Self::I16 => i16::from_le_bytes([b[0], b[1]]) as f32 / 32_768.0,
            Self::I24 => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
            Self::I32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
        }
    }
}

/// Reads the mix format WASAPI hands out (`WAVEFORMATEX`, often extended).
///
/// # Safety
/// `fmt` must point to a valid format returned by `GetMixFormat`.
unsafe fn sample_format(fmt: *const WAVEFORMATEX) -> Option<(SampleFormat, usize, u32)> {
    // SAFETY: the structure is packed: unaligned reads only.
    let base = unsafe { std::ptr::read_unaligned(fmt) };
    let channels = base.nChannels.max(1) as usize;
    let rate = base.nSamplesPerSec;
    let bits = base.wBitsPerSample;
    let tag = if base.wFormatTag == WAVE_FORMAT_EXTENSIBLE && base.cbSize >= 22 {
        // SAFETY: cbSize says the extensible part is there.
        let ext = unsafe { std::ptr::read_unaligned(fmt as *const WAVEFORMATEXTENSIBLE) };
        // KSDATAFORMAT_SUBTYPE_PCM / _IEEE_FLOAT share their tail: the
        // first field is the classic format tag.
        ext.SubFormat.data1 as u16
    } else {
        base.wFormatTag
    };
    let format = match (tag, bits) {
        (WAVE_FORMAT_IEEE_FLOAT, 32) => SampleFormat::F32,
        (WAVE_FORMAT_PCM, 16) => SampleFormat::I16,
        (WAVE_FORMAT_PCM, 24) => SampleFormat::I24,
        (WAVE_FORMAT_PCM, 32) => SampleFormat::I32,
        _ => return None,
    };
    Some((format, channels, rate))
}

/// One open loopback stream on the default output device.
struct Stream {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    device_id: String,
    format: SampleFormat,
    channels: usize,
    rate: u32,
}

impl Stream {
    fn default_device_id(enumerator: &IMMDeviceEnumerator) -> Option<String> {
        // SAFETY: COM calls on a valid enumerator; the id string is freed.
        unsafe {
            let device = enumerator
                .GetDefaultAudioEndpoint(endpoint().0, eConsole)
                .ok()?;
            let id = device.GetId().ok()?;
            let s = id.to_string().ok();
            CoTaskMemFree(Some(id.0 as *const _));
            s
        }
    }

    fn open(enumerator: &IMMDeviceEnumerator) -> Result<Self, String> {
        // SAFETY: standard WASAPI loopback set-up; the mix format is freed
        // once the client is initialised.
        unsafe {
            let device = enumerator
                .GetDefaultAudioEndpoint(endpoint().0, eConsole)
                .map_err(|_| "aucune sortie audio".to_owned())?;
            let device_id = device
                .GetId()
                .ok()
                .map(|id| {
                    let s = id.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(id.0 as *const _));
                    s
                })
                .unwrap_or_default();
            let client: IAudioClient = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| e.to_string())?;
            let fmt = client.GetMixFormat().map_err(|e| e.to_string())?;
            let parsed = sample_format(fmt);
            let init = client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                endpoint().1,
                BUFFER_HNS,
                0,
                fmt,
                None,
            );
            CoTaskMemFree(Some(fmt as *const _));
            init.map_err(|e| e.to_string())?;
            let (format, channels, rate) =
                parsed.ok_or_else(|| "format audio non pris en charge".to_owned())?;
            let capture: IAudioCaptureClient = client.GetService().map_err(|e| e.to_string())?;
            client.Start().map_err(|e| e.to_string())?;
            tracing::info!(rate, channels, ?format, "WASAPI loopback capture running");
            Ok(Self {
                client,
                capture,
                device_id,
                format,
                channels,
                rate,
            })
        }
    }

    /// Appends every pending frame (mixed down to mono) to `mono`.
    fn drain(&self, mono: &mut Vec<f32>) -> windows::core::Result<()> {
        let frame_bytes = self.format.bytes() * self.channels;
        // SAFETY: GetBuffer hands out `frames` frames of `frame_bytes`
        // bytes, valid until ReleaseBuffer.
        unsafe {
            loop {
                let packet = self.capture.GetNextPacketSize()?;
                if packet == 0 {
                    return Ok(());
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let n = frames as usize;
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    mono.extend(std::iter::repeat_n(0.0, n));
                } else {
                    let bytes = std::slice::from_raw_parts(data, n * frame_bytes);
                    for frame in bytes.chunks_exact(frame_bytes) {
                        let mut acc = 0.0f32;
                        for ch in frame.chunks_exact(self.format.bytes()) {
                            acc += self.format.read(ch);
                        }
                        mono.push(acc / self.channels as f32);
                    }
                }
                self.capture.ReleaseBuffer(frames)?;
            }
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: stopping a started client.
        let _ = unsafe { self.client.Stop() };
    }
}

fn run(
    inputs: SharedInputs,
    stop: &AtomicBool,
    ready: &std_mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    let _com = Com::init()?;
    // SAFETY: COM is initialised on this thread.
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|e| e.to_string())?;
    let mut stream = Some(Stream::open(&enumerator)?);
    let _ = ready.send(Ok(()));

    let mut rate = stream.as_ref().map_or(48_000, |s| s.rate);
    let mut analyzer = Analyzer::new(rate, UPDATES_PER_SEC);
    let mut mono: Vec<f32> = Vec::with_capacity(4096);
    let mut last_samples = Instant::now();
    let mut last_check = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        mono.clear();
        if let Some(s) = &stream
            && let Err(e) = s.drain(&mut mono)
        {
            // AUDCLNT_E_DEVICE_INVALIDATED and friends: unplugged headset,
            // driver restart. Reopened below.
            tracing::info!("audio device lost ({e}), reopening");
            stream = None;
        }
        if mono.is_empty() {
            // Windows sends nothing while no application plays sound:
            // silence is fed so the visualiser falls back to rest, like the
            // PipeWire monitor does.
            let idle = last_samples.elapsed();
            if idle >= Duration::from_millis(20) {
                let n = (rate as u128 * idle.as_micros() / 1_000_000) as usize;
                mono.extend(std::iter::repeat_n(0.0, n.min(rate as usize)));
                last_samples = Instant::now();
            }
        } else {
            last_samples = Instant::now();
        }
        if !mono.is_empty()
            && let Some(spectrum) = analyzer.push(&mono)
        {
            inputs.try_set_audio(spectrum);
        }
        if last_check.elapsed() >= DEVICE_CHECK || stream.is_none() {
            last_check = Instant::now();
            let current = Stream::default_device_id(&enumerator);
            let changed = match (&stream, &current) {
                (Some(s), Some(id)) => *id != s.device_id,
                (None, Some(_)) => true,
                _ => false,
            };
            if changed {
                drop(stream.take());
                match Stream::open(&enumerator) {
                    Ok(s) => {
                        if s.rate != rate {
                            rate = s.rate;
                            analyzer.set_rate(rate, UPDATES_PER_SEC);
                        }
                        stream = Some(s);
                    }
                    Err(e) => tracing::info!("audio output unavailable: {e}"),
                }
            }
        }
        std::thread::park_timeout(if stream.is_some() { POLL } else { DEVICE_CHECK });
    }
    tracing::info!("audio capture stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_normalised() {
        assert_eq!(SampleFormat::I16.read(&i16::MIN.to_le_bytes()), -1.0);
        assert_eq!(SampleFormat::F32.read(&0.5f32.to_le_bytes()), 0.5);
        let v = SampleFormat::I24.read(&[0x00, 0x00, 0x40]);
        assert!((v - 0.5).abs() < 1e-6, "{v}");
    }
}
