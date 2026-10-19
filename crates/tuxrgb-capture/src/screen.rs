//! Screen capture for the Screen Sync effect, through the Wayland-native
//! xdg-desktop-portal ScreenCast interface and PipeWire.
//!
//! The compositor (KWin, Mutter…) shows its own picker the first time; the
//! returned restore token is stored so later sessions start silently.

use crate::CaptureError;
use ashpd::desktop::screencast::{
    CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType,
    StartCastOptions,
};
use ashpd::desktop::{CreateSessionOptions, PersistMode, Session};
use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use spa::param::video::VideoFormat;
use spa::pod::Pod;
use std::os::fd::OwnedFd;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tuxrgb_core::signal::{ScreenFrame, SharedInputs};

pub use crate::sampling::{MAX_FPS, SAMPLE_H, SAMPLE_W, downsample, downsample_into};

pub struct ScreenCapture {
    stop: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
    session: Option<Session<Screencast>>,
}

impl ScreenCapture {
    /// Opens a portal session (may show the compositor's picker) and starts
    /// streaming. Returns the capture and the restore token to persist.
    pub async fn start(
        inputs: SharedInputs,
        restore_token: Option<String>,
    ) -> Result<(Self, Option<String>), CaptureError> {
        let portal = |e: ashpd::Error| match e {
            ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => {
                CaptureError::Cancelled
            }
            other => CaptureError::Portal(other.to_string()),
        };
        let proxy = Screencast::new().await.map_err(portal)?;
        let session = proxy
            .create_session(CreateSessionOptions::default())
            .await
            .map_err(portal)?;
        proxy
            .select_sources(
                &session,
                SelectSourcesOptions::default()
                    .set_cursor_mode(CursorMode::Hidden)
                    .set_sources(ashpd::enumflags2::BitFlags::from(SourceType::Monitor))
                    .set_multiple(false)
                    .set_persist_mode(PersistMode::ExplicitlyRevoked)
                    .set_restore_token(restore_token.as_deref()),
            )
            .await
            .map_err(portal)?
            .response()
            .map_err(portal)?;
        let streams = proxy
            .start(&session, None, StartCastOptions::default())
            .await
            .map_err(portal)?
            .response()
            .map_err(portal)?;
        let token = streams.restore_token().map(str::to_owned);
        let stream = streams
            .streams()
            .first()
            .ok_or_else(|| CaptureError::Portal("aucun écran partagé".into()))?;
        let node_id = stream.pipe_wire_node_id();
        let fd = proxy
            .open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default())
            .await
            .map_err(portal)?;
        tracing::info!(node_id, "screen cast session started");

        let (stop_tx, stop_rx) = pw::channel::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("tuxrgb-screen".into())
            .spawn(move || {
                if let Err(e) = run(fd, node_id, inputs, stop_rx) {
                    tracing::warn!("screen capture stopped: {e}");
                }
            })
            .map_err(|e| CaptureError::PipeWire(e.to_string()))?;
        Ok((
            Self {
                stop: stop_tx,
                thread: Some(thread),
                session: Some(session),
            },
            token,
        ))
    }

    /// Stops PipeWire and closes the portal session.
    pub async fn stop(mut self) {
        let _ = self.stop.send(());
        if let Some(t) = self.thread.take() {
            // Joining blocks: not on an async worker.
            let _ = tokio::task::spawn_blocking(move || t.join()).await;
        }
        if let Some(s) = self.session.take() {
            let _ = s.close().await;
        }
    }
}

impl Drop for ScreenCapture {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct UserData {
    format: spa::param::video::VideoInfoRaw,
    inputs: SharedInputs,
    last: Instant,
    /// Previous frame, whose pixel buffer is reused once the engine let go.
    spare: Option<Arc<ScreenFrame>>,
}

fn run(
    fd: OwnedFd,
    node_id: u32,
    inputs: SharedInputs,
    stop_rx: pw::channel::Receiver<()>,
) -> Result<(), String> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
    let core = context.connect_fd_rc(fd, None).map_err(|e| e.to_string())?;

    let weak_loop = mainloop.downgrade();
    let _stop = stop_rx.attach(mainloop.loop_(), move |_| {
        if let Some(l) = weak_loop.upgrade() {
            l.quit();
        }
    });

    let stream = pw::stream::StreamBox::new(
        &core,
        "TuxRGB – synchro écran",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|e| e.to_string())?;

    let min_interval = Duration::from_secs_f32(1.0 / MAX_FPS as f32);
    let _listener = stream
        .add_local_listener_with_user_data(UserData {
            format: Default::default(),
            inputs,
            last: Instant::now() - Duration::from_secs(1),
            spare: None,
        })
        .param_changed(|_, ud, id, param| {
            let Some(param) = param else { return };
            if id != pw::spa::param::ParamType::Format.as_raw() {
                return;
            }
            if ud.format.parse(param).is_ok() {
                tracing::info!(
                    format = ?ud.format.format(),
                    w = ud.format.size().width,
                    h = ud.format.size().height,
                    "screen cast format negotiated"
                );
            }
        })
        .process(move |stream, ud| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            if ud.last.elapsed() < min_interval {
                return;
            }
            let fmt = ud.format.format();
            let size = ud.format.size();
            let (w, h) = (size.width as usize, size.height as usize);
            let datas = buffer.datas_mut();
            let Some(data) = datas.first_mut() else {
                return;
            };
            let stride = data.chunk().stride().max(0) as usize;
            let offset = data.chunk().offset() as usize;
            let Some(bytes) = data.data() else { return };
            if w == 0 || h == 0 || bytes.len() <= offset {
                return;
            }
            let bytes = &bytes[offset..];
            let stride = if stride == 0 { w * 4 } else { stride };
            let (ri, gi, bi) = match fmt {
                VideoFormat::BGRx | VideoFormat::BGRA => (2, 1, 0),
                VideoFormat::RGBx | VideoFormat::RGBA => (0, 1, 2),
                VideoFormat::xRGB | VideoFormat::ARGB => (1, 2, 3),
                VideoFormat::xBGR | VideoFormat::ABGR => (3, 2, 1),
                _ => return,
            };
            ud.last = Instant::now();
            SharedInputs::publish_frame(
                &ud.inputs.screen,
                &mut ud.spare,
                SAMPLE_W,
                SAMPLE_H,
                |pixels| downsample_into(pixels, bytes, w, h, stride, (ri, gi, bi)),
            );
        })
        .register()
        .map_err(|e| e.to_string())?;

    let obj = pw::spa::pod::object!(
        pw::spa::utils::SpaTypes::ObjectParamFormat,
        pw::spa::param::ParamType::EnumFormat,
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaType,
            Id,
            pw::spa::param::format::MediaType::Video
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaSubtype,
            Id,
            pw::spa::param::format::MediaSubtype::Raw
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA,
            VideoFormat::xRGB,
            VideoFormat::xBGR,
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            pw::spa::utils::Rectangle {
                width: 320,
                height: 180
            },
            pw::spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            pw::spa::utils::Rectangle {
                width: 8192,
                height: 8192
            }
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction {
                num: MAX_FPS,
                denom: 1
            },
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction { num: 240, denom: 1 }
        ),
    );
    let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .map_err(|e| format!("{e:?}"))?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&values).ok_or("invalid video format pod")?];

    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| e.to_string())?;

    mainloop.run();
    Ok(())
}
