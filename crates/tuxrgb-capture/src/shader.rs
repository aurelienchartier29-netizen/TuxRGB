//! GLSL shader effect: a fragment shader rendered off-screen by the GPU
//! (wgpu, Vulkan or OpenGL ES) into a small 128×72 image, which the effect
//! engine then samples at each LED's position, exactly like the screen in
//! Screen Sync. This is the native equivalent of the "Shaders" effect of
//! the OpenRGB Effects plugin.
//!
//! Shaders use the Shadertoy convention:
//!
//! ```glsl
//! void mainImage(out vec4 fragColor, in vec2 fragCoord) { … }
//! ```
//!
//! with `iResolution` (vec3), `iTime` (float, seconds), and two TuxRGB
//! extras: `iAccent` (vec3, the desktop accent colour) and `iAudio` (vec4:
//! bass, mid, treble, level – zero unless audio capture runs). A custom
//! shader is read from `~/.config/tuxrgb/shaders/custom.frag`.

use crate::CaptureError;
use crossbeam_channel::{Receiver, Sender, bounded};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tuxrgb_core::Rgb;
use tuxrgb_core::signal::{ScreenFrame, SharedInputs};

pub const WIDTH: u32 = 128;
pub const HEIGHT: u32 = 72;
const FPS: f32 = 30.0;

/// What to render.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderSettings {
    /// GLSL source of `mainImage` (see [`preset_source`]).
    pub source: String,
    /// Time multiplier.
    pub speed: f32,
    pub accent: Rgb,
}

/// Source of a built-in shader, by index in
/// [`tuxrgb_core::effects::SHADER_PRESETS`]; `None` for "Personnalisé".
pub fn preset_source(index: usize) -> Option<&'static str> {
    match index {
        0 => Some(NEON),
        1 => Some(TUNNEL),
        2 => Some(NEBULA),
        3 => Some(OCEAN),
        _ => None,
    }
}

/// Example written to `custom.frag` the first time "Personnalisé" is used.
pub const CUSTOM_TEMPLATE: &str = r#"// TuxRGB – shader personnalisé (convention Shadertoy).
// Modifiez ce fichier puis resélectionnez « Personnalisé » dans TuxRGB.
//
// Entrées : iResolution (vec3), iTime (float),
//           iAccent (vec3, couleur d'accent du bureau),
//           iAudio (vec4 : basses, médiums, aigus, niveau).
// Chaque LED reprend la couleur de sa position sur le canevas.

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec3 col = 0.5 + 0.5 * cos(iTime + uv.xyx * 3.0 + vec3(0.0, 2.0, 4.0));
    col = mix(col, iAccent, 0.35 + 0.35 * iAudio.x);
    fragColor = vec4(col, 1.0);
}
"#;

const NEON: &str = r#"
void mainImage(out vec4 o, in vec2 f) {
    vec2 p = f / iResolution.y * 3.0;
    float t = iTime * 0.6;
    for (int i = 1; i < 6; i++) {
        float fi = float(i);
        p.x += 0.6 / fi * sin(fi * p.y + t);
        p.y += 0.6 / fi * cos(fi * p.x + t * 1.3);
    }
    vec3 c = 0.5 + 0.5 * cos(vec3(0.0, 2.1, 4.2) + p.x + p.y + t);
    o = vec4(c * c, 1.0);
}
"#;

const TUNNEL: &str = r#"
void mainImage(out vec4 o, in vec2 f) {
    vec2 p = (2.0 * f - iResolution.xy) / iResolution.y;
    float r = max(length(p), 0.04);
    float a = atan(p.y, p.x);
    float t = iTime;
    float rings = 0.5 + 0.5 * sin(6.0 / r - t * 3.0);
    float spokes = 0.6 + 0.4 * sin(a * 6.0 + t * 0.7);
    vec3 c = 0.5 + 0.5 * cos(vec3(0.0, 2.0, 4.0) + 1.5 / r + t * 0.4);
    o = vec4(c * rings * spokes * smoothstep(0.0, 0.35, r), 1.0);
}
"#;

const NEBULA: &str = r#"
float h(vec2 p) { return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }
float n(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    return mix(mix(h(i), h(i + vec2(1.0, 0.0)), u.x),
               mix(h(i + vec2(0.0, 1.0)), h(i + vec2(1.0, 1.0)), u.x), u.y);
}
float fbm(vec2 p) {
    float v = 0.0;
    float a = 0.5;
    for (int i = 0; i < 5; i++) { v += a * n(p); p *= 2.03; a *= 0.5; }
    return v;
}
void mainImage(out vec4 o, in vec2 f) {
    vec2 p = f / iResolution.y * 2.5;
    float t = iTime * 0.15;
    float q = fbm(p + vec2(t, -t));
    float r = fbm(p + 3.0 * q + vec2(-t * 1.7, t * 0.9));
    vec3 deep = vec3(0.02, 0.01, 0.06);
    vec3 c = mix(deep, iAccent, smoothstep(0.25, 0.85, r));
    c = mix(c, vec3(1.0, 0.45, 0.85), smoothstep(0.6, 1.0, q) * 0.6);
    o = vec4(c, 1.0);
}
"#;

const OCEAN: &str = r#"
void mainImage(out vec4 o, in vec2 f) {
    vec2 uv = f / iResolution.xy;
    float t = iTime;
    float w = 0.0;
    for (int i = 0; i < 4; i++) {
        float fi = float(i) + 1.0;
        w += sin(uv.x * 6.0 * fi + t * (0.6 + 0.3 * fi) + uv.y * 2.0) / fi;
    }
    float crest = smoothstep(0.55, 1.2, w * 0.5 + uv.y * 1.2);
    vec3 c = mix(vec3(0.0, 0.06, 0.18), vec3(0.0, 0.45, 0.75), uv.y);
    c = mix(c, vec3(0.75, 0.95, 1.0), crest);
    o = vec4(c, 1.0);
}
"#;

const PRELUDE: &str = r#"#version 450
layout(location = 0) out vec4 tux_out;
layout(set = 0, binding = 0, std140) uniform TuxUniforms {
    vec4 u_res;
    vec4 u_time;
    vec4 u_accent;
    vec4 u_audio;
};
#define iResolution u_res.xyz
#define iTime u_time.x
#define iAccent u_accent.rgb
#define iAudio u_audio
"#;

const EPILOGUE: &str = r#"
void main() {
    vec4 c = vec4(0.0, 0.0, 0.0, 1.0);
    mainImage(c, vec2(gl_FragCoord.x, u_res.y - gl_FragCoord.y));
    tux_out = vec4(clamp(c.rgb, 0.0, 1.0), 1.0);
}
"#;

const VERTEX: &str = r#"
@vertex
fn main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}
"#;

/// Compiles a `mainImage` shader with naga (GLSL → IR) and validates it,
/// returning a readable error for the GUI on failure.
pub fn compile(source: &str) -> Result<wgpu::naga::Module, String> {
    use wgpu::naga;
    let full = format!("{PRELUDE}\n{source}\n{EPILOGUE}");
    let mut frontend = naga::front::glsl::Frontend::default();
    let module = frontend
        .parse(
            &naga::front::glsl::Options::from(naga::ShaderStage::Fragment),
            &full,
        )
        .map_err(|e| e.emit_to_string(&full))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .map_err(|e| format!("{e:?}"))?;
    Ok(module)
}

/// Running renderer; dropping it stops the GPU thread.
pub struct ShaderRenderer {
    settings: Sender<Option<ShaderSettings>>,
    thread: Option<JoinHandle<()>>,
}

impl ShaderRenderer {
    /// Opens the GPU and starts rendering; returns once the first shader
    /// compiled (or failed).
    pub fn start(inputs: SharedInputs, settings: ShaderSettings) -> Result<Self, CaptureError> {
        let module = compile(&settings.source).map_err(CaptureError::Shader)?;
        let (tx, rx) = bounded::<Option<ShaderSettings>>(8);
        let (ready_tx, ready_rx) = bounded::<Result<(), String>>(1);
        let thread = std::thread::Builder::new()
            .name("tuxrgb-shader".into())
            .spawn(move || match Gpu::new(module) {
                Ok(gpu) => {
                    let _ = ready_tx.send(Ok(()));
                    gpu.run(inputs, settings, rx);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| CaptureError::Shader(e.to_string()))?;
        match ready_rx.recv_timeout(Duration::from_secs(15)) {
            Ok(Ok(())) => Ok(Self {
                settings: tx,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(CaptureError::Shader(e))
            }
            Err(_) => Err(CaptureError::Shader("le GPU ne répond pas".into())),
        }
    }

    /// Changes the shader / speed / accent without restarting the GPU.
    /// Returns the compile error of a new source, if any.
    pub fn update(&self, settings: ShaderSettings) -> Result<(), String> {
        compile(&settings.source)?;
        let _ = self.settings.try_send(Some(settings));
        Ok(())
    }
}

impl Drop for ShaderRenderer {
    fn drop(&mut self) {
        let _ = self.settings.send_timeout(None, Duration::from_millis(200));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::PipelineLayout,
    vertex: wgpu::ShaderModule,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    target: wgpu::Texture,
    /// View of `target`, created once.
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
}

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const ROW_BYTES: u32 = WIDTH * 4; // 512, a multiple of 256 as wgpu requires.

impl Gpu {
    fn new(module: wgpu::naga::Module) -> Result<Self, String> {
        // Windows (beta): Direct3D 12 first, as iced uses it there.
        #[cfg(windows)]
        let backends = wgpu::Backends::DX12 | wgpu::Backends::VULKAN | wgpu::Backends::GL;
        #[cfg(not(windows))]
        let backends = wgpu::Backends::VULKAN | wgpu::Backends::GL;
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends,
            ..wgpu::InstanceDescriptor::from_env_or_default()
        });
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .map_err(|e| format!("aucun GPU utilisable ({e})"))?;
        let info = adapter.get_info();
        tracing::info!(name = %info.name, backend = ?info.backend, "shader GPU");
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("tuxrgb-shader"),
            required_limits:
                wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .map_err(|e| format!("GPU indisponible ({e})"))?;
        // Never panic on a GPU error: log it, the effect shows its fallback.
        device.on_uncaptured_error(Arc::new(|e| {
            tracing::warn!("shader GPU error: {e}");
        }));

        let vertex = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tuxrgb-fullscreen"),
            source: wgpu::ShaderSource::Wgsl(VERTEX.into()),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tuxrgb-uniforms"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tuxrgb-shader-target"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tuxrgb-readback"),
            size: (ROW_BYTES * HEIGHT) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let pipeline = Self::pipeline(&device, &layout, &vertex, module);
        let view = target.create_view(&Default::default());
        Ok(Self {
            device,
            queue,
            pipeline,
            layout,
            vertex,
            uniforms,
            bind_group,
            target,
            view,
            readback,
        })
    }

    fn pipeline(
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        vertex: &wgpu::ShaderModule,
        module: wgpu::naga::Module,
    ) -> wgpu::RenderPipeline {
        let fragment = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tuxrgb-user-shader"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module)),
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tuxrgb-shader"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: vertex,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &fragment,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        })
    }

    fn run(
        mut self,
        inputs: SharedInputs,
        mut settings: ShaderSettings,
        rx: Receiver<Option<ShaderSettings>>,
    ) {
        let period = Duration::from_secs_f32(1.0 / FPS);
        let mut time = 0.0f32;
        let mut last = Instant::now();
        // Previous frame: its pixel buffer is reused once the engine let go.
        let mut spare: Option<Arc<ScreenFrame>> = None;
        let mut failures = 0u32;
        loop {
            match rx.recv_timeout(period) {
                Ok(Some(new)) => {
                    if new.source != settings.source
                        && let Ok(module) = compile(&new.source)
                    {
                        self.pipeline =
                            Self::pipeline(&self.device, &self.layout, &self.vertex, module);
                    }
                    settings = new;
                }
                Ok(None) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            }
            let now = Instant::now();
            time += (now - last).as_secs_f32() * settings.speed;
            last = now;
            let audio = inputs
                .latest_audio()
                .filter(|a| a.captured_at.elapsed() < Duration::from_millis(500));
            let audio = audio
                .map(|a| [a.bass, a.mid, a.treble, a.level])
                .unwrap_or([0.0; 4]);
            let [ar, ag, ab] = settings.accent.to_f32();
            if self.render(time, [ar, ag, ab], audio, &inputs, &mut spare) {
                failures = 0;
            } else {
                failures += 1;
                if failures == 1 || failures.is_power_of_two() {
                    tracing::warn!(failures, "shader frame not read back from the GPU");
                }
                // Back off while the GPU is busy or resetting.
                std::thread::sleep(period * failures.min(30));
            }
        }
        inputs.set_shader(None);
    }

    /// Renders one frame and publishes it; `false` if the GPU did not
    /// deliver it in time.
    fn render(
        &self,
        time: f32,
        accent: [f32; 3],
        audio: [f32; 4],
        inputs: &SharedInputs,
        spare: &mut Option<Arc<ScreenFrame>>,
    ) -> bool {
        let mut u = [0.0f32; 16];
        u[0] = WIDTH as f32;
        u[1] = HEIGHT as f32;
        u[2] = 1.0;
        u[4] = time;
        u[8..11].copy_from_slice(&accent);
        u[12..16].copy_from_slice(&audio);
        let mut bytes = [0u8; 64];
        for (chunk, f) in bytes.chunks_exact_mut(4).zip(u) {
            chunk.copy_from_slice(&f.to_le_bytes());
        }
        self.queue.write_buffer(&self.uniforms, 0, &bytes);

        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(ROW_BYTES),
                    rows_per_image: Some(HEIGHT),
                },
            },
            wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = bounded(1);
        self.readback.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r.is_ok());
        });
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(2)),
        });
        if !rx.recv_timeout(Duration::from_secs(2)).unwrap_or(false) {
            // Cancel a mapping still pending (or failed): otherwise every
            // following `map_async` is rejected and the effect stays frozen.
            self.readback.unmap();
            return false;
        }
        {
            let data = self.readback.get_mapped_range(..);
            SharedInputs::publish_frame(
                &inputs.shader,
                spare,
                WIDTH as usize,
                HEIGHT as usize,
                |pixels| {
                    pixels.extend(data.chunks_exact(4).map(|p| Rgb::new(p[0], p[1], p[2])));
                },
            );
        }
        self.readback.unmap();
        true
    }
}

/// Minimal executor for wgpu's set-up futures (they resolve immediately on
/// native backends).
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    use std::task::{Context, Poll, Wake, Waker};
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_shaders_compile() {
        for i in 0..tuxrgb_core::effects::SHADER_PRESETS.len() {
            if let Some(src) = preset_source(i) {
                compile(src).unwrap_or_else(|e| panic!("preset {i}: {e}"));
            }
        }
        compile(CUSTOM_TEMPLATE).expect("custom template");
    }

    #[test]
    fn broken_shader_reports_an_error() {
        let e = compile("void mainImage(out vec4 o, in vec2 f) { o = nope; }").unwrap_err();
        assert!(!e.is_empty());
    }

    /// Renders on the GPU when one is available (skipped otherwise).
    #[test]
    fn renders_a_frame_when_a_gpu_exists() {
        let Ok(gpu) = Gpu::new(compile(NEON).unwrap()) else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let inputs = SharedInputs::default();
        let mut spare = None;
        for _ in 0..3 {
            assert!(gpu.render(1.0, [0.2, 0.6, 0.9], [0.0; 4], &inputs, &mut spare));
        }
        let px = inputs.latest_shader().expect("frame read back");
        assert_eq!(px.pixels.len(), (WIDTH * HEIGHT) as usize);
        assert!(px.pixels.iter().any(|c| *c != Rgb::BLACK));
    }
}
