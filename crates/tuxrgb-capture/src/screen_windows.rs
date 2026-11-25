//! Screen capture for the Screen Sync effect on Windows.
//!
//! GDI reads the primary monitor straight into a small top-down 32-bit
//! bitmap (`StretchBlt`, one sample per analysed sub-cell), then the same
//! box filter as on Linux builds the `SAMPLE_W × SAMPLE_H` frame. No picker
//! and no permission: Windows lets an application read the desktop.
//!
//! Limitation: games in exclusive full screen are not visible to GDI
//! (the frame stays black); borderless / windowed games are.

use crate::CaptureError;
use crate::sampling::SUB;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tuxrgb_core::signal::{ScreenFrame, SharedInputs};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, COLORONCOLOR, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SRCCOPY,
    SelectObject, SetStretchBltMode, StretchBlt,
};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

pub use crate::sampling::{MAX_FPS, SAMPLE_H, SAMPLE_W, downsample, downsample_into};

/// Size of the bitmap GDI writes: one pixel per sample of the box filter.
const GRAB_W: usize = SAMPLE_W * SUB;
const GRAB_H: usize = SAMPLE_H * SUB;

pub struct ScreenCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ScreenCapture {
    /// Starts reading the primary monitor. There is no picker on Windows,
    /// so no restore token either (the one given is ignored).
    pub async fn start(
        inputs: SharedInputs,
        _restore_token: Option<String>,
    ) -> Result<(Self, Option<String>), CaptureError> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
        let thread = std::thread::Builder::new()
            .name("tuxrgb-screen".into())
            .spawn(move || run(inputs, flag, ready_tx))
            .map_err(|e| CaptureError::Portal(e.to_string()))?;
        match tokio::time::timeout(Duration::from_secs(5), ready_rx).await {
            Ok(Ok(Ok(()))) => {
                tracing::info!("screen capture (GDI) started");
                Ok((
                    Self {
                        stop,
                        thread: Some(thread),
                    },
                    None,
                ))
            }
            Ok(Ok(Err(e))) => {
                tracing::warn!("screen capture (GDI) unavailable: {e}");
                Err(CaptureError::Portal(e))
            }
            _ => {
                stop.store(true, Ordering::Relaxed);
                Err(CaptureError::Portal(
                    "la capture d'écran ne répond pas".into(),
                ))
            }
        }
    }

    /// Stops the capture thread.
    pub async fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = tokio::task::spawn_blocking(move || t.join()).await;
        }
    }
}

impl Drop for ScreenCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

/// GDI objects of the capture, released in the right order.
struct Grabber {
    screen: HDC,
    memory: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *const u8,
}

impl Grabber {
    fn new() -> Result<Self, String> {
        // SAFETY: plain GDI object creation; every object is released in
        // `Drop`, and `bits` lives as long as `bitmap`.
        unsafe {
            let screen = GetDC(HWND::default());
            if screen.is_invalid() {
                return Err("écran inaccessible (GetDC)".into());
            }
            let memory = CreateCompatibleDC(screen);
            if memory.is_invalid() {
                ReleaseDC(HWND::default(), screen);
                return Err("CreateCompatibleDC a échoué".into());
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: GRAB_W as i32,
                    // Negative height: top-down rows, like the PipeWire frames.
                    biHeight: -(GRAB_H as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) if !bits.is_null() => b,
                _ => {
                    let _ = DeleteDC(memory);
                    ReleaseDC(HWND::default(), screen);
                    return Err("CreateDIBSection a échoué".into());
                }
            };
            let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
            SetStretchBltMode(memory, COLORONCOLOR);
            Ok(Self {
                screen,
                memory,
                bitmap,
                previous,
                bits: bits as *const u8,
            })
        }
    }

    /// Copies the primary monitor into the bitmap; `None` while the desktop
    /// cannot be read (locked session, secure desktop of a UAC prompt).
    fn grab(&self) -> Option<&[u8]> {
        // SAFETY: valid DCs; the DIB section holds GRAB_W × GRAB_H × 4 bytes.
        unsafe {
            let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
            if sw <= 0 || sh <= 0 {
                return None;
            }
            StretchBlt(
                self.memory,
                0,
                0,
                GRAB_W as i32,
                GRAB_H as i32,
                self.screen,
                0,
                0,
                sw,
                sh,
                SRCCOPY,
            )
            .as_bool()
            .then(|| std::slice::from_raw_parts(self.bits, GRAB_W * GRAB_H * 4))
        }
    }
}

impl Drop for Grabber {
    fn drop(&mut self) {
        // SAFETY: releases what `new` created, in reverse order.
        unsafe {
            SelectObject(self.memory, self.previous);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.memory);
            ReleaseDC(HWND::default(), self.screen);
        }
    }
}

fn run(
    inputs: SharedInputs,
    stop: Arc<AtomicBool>,
    ready: tokio::sync::oneshot::Sender<Result<(), String>>,
) {
    let grabber = match Grabber::new() {
        Ok(g) => g,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    let period = Duration::from_secs_f32(1.0 / MAX_FPS as f32);
    let mut spare: Option<Arc<ScreenFrame>> = None;
    while !stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        if let Some(bytes) = grabber.grab() {
            // GDI bitmaps are BGRA.
            SharedInputs::publish_frame(&inputs.screen, &mut spare, SAMPLE_W, SAMPLE_H, |pixels| {
                downsample_into(pixels, bytes, GRAB_W, GRAB_H, GRAB_W * 4, (2, 1, 0))
            });
        }
        std::thread::park_timeout(period.saturating_sub(started.elapsed()));
    }
    tracing::info!("screen capture stopped");
}
