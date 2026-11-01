//! PipeWire capture of the default output monitor ("what you hear").

use crate::CaptureError;
use crate::analyzer::Analyzer;
use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use spa::param::format::{MediaSubtype, MediaType};
use spa::param::format_utils;
use spa::pod::Pod;
use std::sync::mpsc as std_mpsc;
use std::thread::JoinHandle;
use std::time::Duration;
use tuxrgb_core::signal::SharedInputs;

/// Spectra per second (one FFT every 10 ms at 48 kHz): faster than the
/// effect engine, so every frame uses fresh data.
const UPDATES_PER_SEC: u32 = 100;

/// Running audio capture; dropping it stops the PipeWire thread.
pub struct AudioCapture {
    stop: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl AudioCapture {
    /// Starts capturing. Returns once the PipeWire stream is set up (or failed).
    pub fn start(inputs: SharedInputs) -> Result<Self, CaptureError> {
        let (stop_tx, stop_rx) = pw::channel::channel::<()>();
        let (ready_tx, ready_rx) = std_mpsc::channel::<Result<(), String>>();
        let thread = std::thread::Builder::new()
            .name("tuxrgb-audio".into())
            .spawn(move || {
                if let Err(e) = run(inputs, stop_rx, &ready_tx) {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| CaptureError::PipeWire(e.to_string()))?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                stop: stop_tx,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(CaptureError::PipeWire(e))
            }
            Err(_) => {
                let _ = stop_tx.send(());
                Err(CaptureError::PipeWire("PipeWire ne répond pas".into()))
            }
        }
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct UserData {
    format: spa::param::audio::AudioInfoRaw,
    analyzer: Analyzer,
    mono: Vec<f32>,
    inputs: SharedInputs,
}

fn run(
    inputs: SharedInputs,
    stop_rx: pw::channel::Receiver<()>,
    ready: &std_mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
    let core = context.connect_rc(None).map_err(|e| e.to_string())?;

    let weak_loop = mainloop.downgrade();
    let _stop = stop_rx.attach(mainloop.loop_(), move |_| {
        if let Some(l) = weak_loop.upgrade() {
            l.quit();
        }
    });

    let props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Music",
        *pw::keys::STREAM_CAPTURE_SINK => "true",
        *pw::keys::NODE_NAME => "tuxrgb-visualizer",
        *pw::keys::APP_NAME => "TuxRGB",
        // ~10 ms buffers: the LEDs follow the music without visible lag.
        *pw::keys::NODE_LATENCY => "512/48000",
    };
    let stream = pw::stream::StreamBox::new(&core, "TuxRGB – visualiseur audio", props)
        .map_err(|e| e.to_string())?;

    let data = UserData {
        format: Default::default(),
        analyzer: Analyzer::new(48_000, UPDATES_PER_SEC),
        mono: Vec::with_capacity(4096),
        inputs,
    };

    let _listener = stream
        .add_local_listener_with_user_data(data)
        .param_changed(|_, ud, id, param| {
            let Some(param) = param else { return };
            if id != pw::spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((media_type, media_subtype)) = format_utils::parse_format(param) else {
                return;
            };
            if media_type != MediaType::Audio || media_subtype != MediaSubtype::Raw {
                return;
            }
            if ud.format.parse(param).is_ok() {
                ud.analyzer.set_rate(ud.format.rate(), UPDATES_PER_SEC);
                tracing::info!(
                    rate = ud.format.rate(),
                    channels = ud.format.channels(),
                    "audio capture format negotiated"
                );
            }
        })
        .process(|stream, ud| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            let Some(data) = datas.first_mut() else {
                return;
            };
            let channels = ud.format.channels().max(1) as usize;
            let offset = data.chunk().offset() as usize;
            let size = data.chunk().size() as usize;
            let Some(bytes) = data.data() else { return };
            // The valid samples are `size` bytes from `offset` (bounded by
            // the mapped buffer, whatever the chunk claims).
            let start = offset.min(bytes.len());
            let end = start.saturating_add(size).min(bytes.len());
            let bytes = &bytes[start..end];
            ud.mono.clear();
            for frame in bytes.chunks_exact(4 * channels) {
                let mut acc = 0.0f32;
                for ch in frame.chunks_exact(4) {
                    acc += f32::from_le_bytes([ch[0], ch[1], ch[2], ch[3]]);
                }
                ud.mono.push(acc / channels as f32);
            }
            // Real-time thread: never wait for the engine's lock.
            if let Some(spectrum) = ud.analyzer.push(&ud.mono) {
                ud.inputs.try_set_audio(spectrum);
            }
        })
        .register()
        .map_err(|e| e.to_string())?;

    let mut audio_info = spa::param::audio::AudioInfoRaw::new();
    audio_info.set_format(spa::param::audio::AudioFormat::F32LE);
    let obj = pw::spa::pod::Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: pw::spa::param::ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .map_err(|e| format!("{e:?}"))?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&values).ok_or("invalid audio format pod")?];

    stream
        .connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )
        .map_err(|e| e.to_string())?;

    let _ = ready.send(Ok(()));
    tracing::info!("audio capture running");
    mainloop.run();
    tracing::info!("audio capture stopped");
    Ok(())
}
