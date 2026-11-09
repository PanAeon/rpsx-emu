use cpal::traits::StreamTrait;
use gilrs::Axis;
use gilrs::ff::BaseEffect;
use gilrs::ff::BaseEffectType;
use gilrs::ff::EffectBuilder;
use gilrs::ff::Replay;
use gilrs::ff::Ticks;
use log::LevelFilter;
use log::info;
use wgpu::hal::vulkan::conv::map_filter_mode;
use wgpu_profiler::GpuProfiler;
use winit::window::CustomCursor;
// use env_logger::fmt::style::Color;
use std::io::Read;
use std::ops::Mul;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread::current;
use std::time::Duration;
use wgpu::util::DeviceExt;
use winit::keyboard::Key;
use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::Window,
};

use gilrs::{Event, GamepadId, Gilrs};

use std::{borrow::Cow, num::NonZeroU64};

use crate::cdrom::CDRom;
use crate::dynarec::Dynarec;
use crate::gpu::DisplayDepth;
use crate::save_state::load_state;
use crate::save_state::save_state;
use crate::sio::Dualshock;
use crate::sio::GunCon;
use crate::sio::Justifier;
use crate::sio::Sio;
use crate::spu::Spu;
use crate::system::System;
use crate::timers::ClockSource;

mod audio;
mod bios;
mod cdrom;
mod compute_renderer;
mod cpu;
mod dma;
mod gpu;
mod gte;
mod hw_renderer;
mod irq;
mod mdec;
mod ram;
mod renderer;
mod scheduler;
mod scratchpad;
mod sio;
mod sio2;
mod spu;
mod system;
mod timers;

mod adpcm;
mod block_cache;
mod cd_cache;
mod cdrom_xa;
mod cdxa;
mod cue;
mod dummy_renderer;
mod dynarec;
mod fastmem;
mod interpreter;
mod iso9660;
mod presenter;
mod save_state;
// const FIVE_BIT_TO_8BIT: [u8; 32] = {
//     let mut table = [0u8; 32];
//     let mut i = 0;
//     while i < 32 {
//         table[i] = (i as f64 * 255.0 / 31.0).round() as u8;
//         i += 1;
//     }
//     table
// };

use clap::Parser;

#[derive(Parser, Clone)]
pub struct Cli {
    filename: Option<String>,
    #[arg(long)]
    softgpu: bool,
    #[arg(long)]
    sideload: bool,
    #[arg(long)]
    profilegpu: bool,
    #[arg(long)]
    enable_crt_shader: bool,
    #[arg(long)]
    negcon: bool,
    #[arg(long)]
    interpreter: bool,
    #[arg(long)]
    hw_renderer: bool,
    #[arg(long, default_value_t = 1)]
    scale: u32,
    #[arg(long)]
    mouse1: bool,
    #[arg(long)]
    mouse2: bool,
    #[arg(long)]
    guncon: bool,
    #[arg(long)]
    justifier: bool,
}

fn main() {
    let cli = Cli::parse();
    // for x in FIVE_BIT_TO_8BIT {
    //     println!("{},", (x as f32) / 255.0 )
    // }
    crate::fastmem::install_segfault_handler();
    // let res = crate::cue::parse_cue("/foo/psx/Mortal Kombat Trilogy (USA) (v1.1).cue");
    // println!("{:?}", res);

    run(cli).unwrap();

    // fastmem_proto::run();
    println!("goodbye!");
}

// const fn mat4_const_from_rows(m: [[f32; 4]; 4]) -> Mat4<f32> {
//     Mat4 {
//         cols: Vec4 {
//             x: Vec4::new(m[0][0], m[1][0], m[2][0], m[3][0]),
//             y: Vec4::new(m[0][1], m[1][1], m[2][1], m[3][1]),
//             z: Vec4::new(m[0][2], m[1][2], m[2][2], m[3][2]),
//             w: Vec4::new(m[0][3], m[1][3], m[2][3], m[3][3]),
//         },
//     }
// }

// #[rustfmt::skip]
// pub const FULLSCREEN_QUAD_CAMERA: Mat4<f32> = mat4_const_from_rows([
//     [2.0, 0.0, 0.0, -1.0],
//     [0.0, 2.0, 0.0, -1.0],
//     [0.0, 0.0, 1.0, 0.0],
//     [0.0, 0.0, 0.0, 1.0],
// ]);

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    position: [f32; 3],
    uv: [f32; 2],
}

impl Vertex {
    fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::size_of::<[f32; 3]>() as wgpu::BufferAddress,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x2,
                },
            ],
        }
    }
}

pub const VERTICES: &[Vertex] = &[
    Vertex {
        position: [0.0, 0.0, 0.0],
        uv: [0.0, 0.0],
    },
    Vertex {
        position: [1.0, 0.0, 0.0],
        uv: [1.0, 0.0],
    },
    Vertex {
        position: [0.0, 1.0, 0.0],
        uv: [0.0, 1.0],
    },
    Vertex {
        position: [0.0, 1.0, 0.0],
        uv: [0.0, 1.0],
    },
    Vertex {
        position: [1.0, 0.0, 0.0],
        uv: [1.0, 0.0],
    },
    Vertex {
        position: [1.0, 1.0, 0.0],
        uv: [1.0, 1.0],
    },
];

#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct DisplayUniforms {
    is_24bpp: u32,
}

#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct TextureBuffer {
    width: u32,
    height: u32,
}
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct Color {
    r: u8,
    g: u8,
    b: u8,
    a: u8,
}

#[allow(dead_code)]
pub struct State {
    instance: wgpu::Instance,
    surface: Option<wgpu::Surface<'static>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    is_surface_configured: bool,
    window: Arc<Window>,
    clear_color: wgpu::Color,
    render_finished: web_time::Instant,
    frame_num: u64,
    render_time_ms: u32,
    // camera: Camera,
    // camera_buffer: wgpu::Buffer,
    uniforms_buffer: wgpu::Buffer,
    uniforms_bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    // texture_params_buffer: wgpu::Buffer,
    render_texture: wgpu::Texture,
    texture_bind_group: wgpu::BindGroup,
    // dimensions: (u32, u32),
    framebuffer: Arc<Mutex<Vec<u32>>>,
    // image_rgba: image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    system: Pin<Box<System>>,
    interpreter: interpreter::Interpreter,
    texture_size: wgpu::Extent3d,
    audio_sender: crossbeam::channel::Sender<[i16; 2]>,
    audio_stream: cpal::Stream,
    paused: bool,
    gilrs: Option<Gilrs>,
    active_gamepad: Option<GamepadId>,
    output_width: usize,
    output_height: usize,
    sx: usize,
    sy: usize,
    display_depth: DisplayDepth,
    display_vram: bool,
    software_render: bool,
    sideload_exe: Option<String>,
    ctrl_pressed: bool,
    draw_pipeline: wgpu::RenderPipeline,
    draw_texture: wgpu::Texture,
    draw_vertex_buffer: wgpu::Buffer,
    draw_texture_bind_group: wgpu::BindGroup,
    negcon: bool,
    use_interpreter: bool,
    scale: u32,
    mouse1: bool,
    mouse2: bool,
    mouse_pos: (f64, f64),
    mouse_moved: bool,
    guncon: bool,
    justifier: bool,
    line_start_cycle: u64,
    // ff_effect: Option<gilrs::ff::Effect>,
}

impl State {
    async fn new(window: Arc<Window>, cli: Cli) -> anyhow::Result<State> {
        let size = window.inner_size();

        // The instance is a handle to our GPU
        // BackendBit::PRIMARY => Vulkan + Metal + DX12 + Browser WebGPU
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            flags: Default::default(),
            memory_budget_thresholds: Default::default(),
            backend_options: Default::default(),
            display: None,
            // ..Default::default()
        });

        let surface = instance.create_surface(window.clone()).unwrap();

        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        for a in adapters {
            info!("available: {}", a.get_info().name);
        }

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                // power_preference: wgpu::PowerPreference::LowPower,
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await?;

        info!("Selected adapter name: {}", adapter.get_info().name);

        let mut limits = wgpu::Limits::default().using_resolution(adapter.limits());
        limits.max_immediate_size = 8;
        // limits.max_texture_dimension_2d *= 2;
        // limits.max_dynamic_storage_buffers_per_pipeline_layout = 8;
        // limits.max_storage_textures_per_shader_stage = 8;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu::Features::TEXTURE_FORMAT_16BIT_NORM
                    | wgpu::Features::VERTEX_WRITABLE_STORAGE
                    | GpuProfiler::ALL_WGPU_TIMER_FEATURES
                    | wgpu::Features::IMMEDIATES
                    | wgpu::Features::DUAL_SOURCE_BLENDING,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                // WebGL doesn't support all of wgpu's features, so if
                // we're building for the web we'll have to disable some.
                required_limits: if cfg!(target_arch = "wasm32") {
                    wgpu::Limits::downlevel_webgl2_defaults()
                } else {
                    limits
                },
                memory_hints: Default::default(),
                trace: wgpu::Trace::Off,
            })
            .await?;

        device.on_uncaptured_error(Arc::new(|error| {
            eprintln!("Uncaptured wgpu error: {:#?}", error);
        }));

        let surface_caps = surface.get_capabilities(&adapter);

        // Shader code in this tutorial assumes an Srgb surface texture. Using a different
        // one will result all the colors comming out darker. If you want to support non
        // Srgb surfaces, you'll need to account for that when drawing to the frame.
        let surface_format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width,
            height: size.height,
            present_mode: surface_caps.present_modes[0],
            alpha_mode: surface_caps.alpha_modes[0],
            desired_maximum_frame_latency: 2,
            view_formats: vec![surface_format],
            color_space: wgpu::wgt::SurfaceColorSpace::Auto,
        };

        let scale: u32 = if cli.scale > 0 && cli.hw_renderer && !cli.softgpu {
            cli.scale
        } else {
            1
        };
        let texture_size = wgpu::Extent3d {
            width: 1024 * scale,
            height: 512 * scale,
            // All textures are stored as 3D, we represent our 2D texture
            // by setting depth to 1.
            depth_or_array_layers: 1,
        };
        let render_texture = if cli.hw_renderer {
            device.create_texture(&wgpu::TextureDescriptor {
                size: texture_size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // Most images are stored using sRGB, so we need to reflect that here.
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                // TEXTURE_BINDING tells wgpu that we want to use this texture in shaders
                // COPY_DST means that we want to copy data to this texture
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                label: Some("render texture"),
                // This is the same as with the SurfaceConfig. It
                // specifies what texture formats can be used to
                // create TextureViews for this texture. The base
                // texture format (Rgba8UnormSrgb in this case) is
                // always supported. Note that using a different
                // texture format is not supported on the WebGL2
                // backend.
                view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
            })
        } else {
            device.create_texture(&wgpu::TextureDescriptor {
                size: texture_size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // Most images are stored using sRGB, so we need to reflect that here.
                format: wgpu::TextureFormat::R32Uint,
                // TEXTURE_BINDING tells wgpu that we want to use this texture in shaders
                // COPY_DST means that we want to copy data to this texture
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                label: Some("render texture"),
                // This is the same as with the SurfaceConfig. It
                // specifies what texture formats can be used to
                // create TextureViews for this texture. The base
                // texture format (Rgba8UnormSrgb in this case) is
                // always supported. Note that using a different
                // texture format is not supported on the WebGL2
                // backend.
                view_formats: &[wgpu::TextureFormat::R32Uint],
            })
        };

        let depth_stencil: Option<wgpu::DepthStencilState> = None;

        // let shader_src: String;
        let shader_source = if cli.hw_renderer {
            //  let src = include_str!("raw-shader.wgsl");
            // let width = format!("{}", 1024 * scale);
            // let height = format!("{}", 512 * scale);
            // shader_src = src.replace("1024", &width).replace("512", &height);
            // let src: &str = shader_src.as_str();
            Cow::Borrowed(include_str!("raw-shader.wgsl"))
        } else {
            let src = include_str!("shader.wgsl");
            Cow::Borrowed(src)
        };
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shaders"),
            source: wgpu::ShaderSource::Wgsl(shader_source),
        });
        let uniforms_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("uniforms_bind_group_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::all(),
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(
                            ::std::mem::size_of::<DisplayUniforms>() as u64
                        ),
                    },
                    count: None,
                }],
            });
        // let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        //     label: Some("camera_buffer"),
        //     size: ::std::mem::size_of::<[[f32; 4]; 4]>() as u64,
        //     usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        //     mapped_at_creation: false,
        // });
        let uniforms_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniform_buffer"),
            size: ::std::mem::size_of::<DisplayUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniforms_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniforms_bind_group"),
            layout: &uniforms_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms_buffer.as_entire_binding(),
            }],
        });
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertex_buffer"),
            contents: bytemuck::cast_slice(VERTICES),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        // let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        //     label: Some("vertex_buffer"),
        //     size: 1,
        //     usage: wgpu::BufferUsages::VERTEX,
        //     mapped_at_creation: false,
        // });
        let texture_bind_group_layout = if cli.hw_renderer {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("texture_bind_group_layout"),
                entries: &[
                    // wgpu::BindGroupLayoutEntry {
                    //     binding: 0,
                    //     visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    //     ty: wgpu::BindingType::Buffer {
                    //         ty: wgpu::BufferBindingType::Uniform,
                    //         has_dynamic_offset: false,
                    //         min_binding_size: NonZeroU64::new(
                    //             ::std::mem::size_of::<TextureBuffer>() as u64,
                    //         ),
                    //     },
                    //     count: None,
                    // },
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            })
        } else {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("texture_bind_group_layout"),
                entries: &[
                    // wgpu::BindGroupLayoutEntry {
                    //     binding: 0,
                    //     visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    //     ty: wgpu::BindingType::Buffer {
                    //         ty: wgpu::BufferBindingType::Uniform,
                    //         has_dynamic_offset: false,
                    //         min_binding_size: NonZeroU64::new(
                    //             ::std::mem::size_of::<TextureBuffer>() as u64,
                    //         ),
                    //     },
                    //     count: None,
                    // },
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Uint,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // wgpu::BindGroupLayoutEntry {
                    //     binding: 2,
                    //     visibility: wgpu::ShaderStages::FRAGMENT,
                    //     ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    //     count: None,
                    // },
                ],
            })
        };
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline_layout"),
            bind_group_layouts: &[
                Some(&uniforms_bind_group_layout),
                Some(&texture_bind_group_layout),
            ],
            immediate_size: 0,
            // push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_module,
                entry_point: Some(&"vert_main"),
                buffers: &[Some(Vertex::desc())],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader_module,
                entry_point: Some(&"frag_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
            // multiview: None,
        });

        // let data_view = tileset_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let diffuse_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        // let texture_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        //     label: Some("texture_params_buffer"),
        //     size: ::std::mem::size_of::<TextureBuffer>() as u64,
        //     usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        //     mapped_at_creation: false,
        // });
        let texture_view = render_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let texture_bind_group = if cli.hw_renderer {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("texture bind_group"),
                layout: &texture_bind_group_layout,
                entries: &[
                    // wgpu::BindGroupEntry {
                    //     binding: 0,
                    //     resource: texture_params_buffer.as_entire_binding(),
                    // },
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&diffuse_sampler),
                    },
                ],
            })
        } else {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("texture bind_group"),
                layout: &texture_bind_group_layout,
                entries: &[
                    // wgpu::BindGroupEntry {
                    //     binding: 0,
                    //     resource: texture_params_buffer.as_entire_binding(),
                    // },
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&texture_view),
                    },
                    // wgpu::BindGroupEntry {
                    //     binding: 2,
                    //     resource: wgpu::BindingResource::Sampler(&diffuse_sampler),
                    // },
                ],
            })
        };

        let (draw_pipeline, draw_texture, draw_vertex_buffer, draw_texture_bind_group) =
            crate::presenter::create_draw_pipeline(&device, config.format, cli.enable_crt_shader);

        let (ram_slice, bios_slice) = fastmem::create_memory_mappings();
        let bios = bios::Bios::new(bios_slice, Path::new("/foo/SCPH1001.BIN"))?;
        // let bios = bios::Bios::new(bios_slice, Path::new("/foo/openbios.bin"))?;
        let ram = ram::Ram::new(ram_slice);
        let scratchpad = scratchpad::Scratchpad::new();
        let dma = dma::Dma::new();
        // let gpu = gpu::Gpu::new();
        let spu = spu::Spu::new();
        let cdrom = CDRom::new(if cli.sideload {
            None
        } else {
            cli.filename.clone()
        });
        // let spu = spu::Spu::default();
        let irqctl = irq::InterruptController::default();
        let sio = sio::Sio::new(
            cli.negcon,
            cli.mouse1,
            cli.mouse2,
            cli.guncon,
            cli.justifier,
        );
        let sio2 = sio2::Sio2::new();
        let mdec = mdec::Mdec::new();
        let mut scheduler = scheduler::Scheduler::default();
        scheduler.init();
        let timers = timers::Timers::new();

        let software_render = cli.softgpu;

        let (sender, receiver, handle) = if software_render {
            renderer::Renderer::create()
        } else {
            if cli.hw_renderer {
                crate::hw_renderer::HWRenderer::create(
                    device.clone(),
                    queue.clone(),
                    config.format,
                    render_texture.clone(),
                    cli.profilegpu,
                    scale,
                )
            } else {
                compute_renderer::ComputeRenderer::create(
                    device.clone(),
                    queue.clone(),
                    config.format,
                    render_texture.clone(),
                    cli.profilegpu,
                )
            }
        };
        let gpu = gpu::Gpu::new(sender, receiver, handle, scale);
        let block_cache = block_cache::BlockCache::new();
        let cpu = dynarec::Dynarec::new();
        let system = system::System::new(
            cpu,
            bios,
            ram,
            scratchpad,
            dma,
            spu,
            irqctl,
            scheduler,
            timers,
            cdrom,
            sio,
            sio2,
            mdec,
            gpu,
            block_cache,
        );
        let mut system = Box::pin(system);

        Dynarec::create_compiler(&mut system);

        // interpreter
        // let bios = bios::Bios::new(Path::new("/foo/SCPH1001.BIN"))?;
        // // let bios = bios::Bios::new(Path::new("/foo/openbios.bin"))?;
        // let ram = ram::Ram::new();
        // let scratchpad = scratchpad::Scratchpad::new();
        // let dma = dma::Dma::new();
        // // let gpu = gpu::Gpu::new();
        // let spu = spu::Spu::new();
        // let cdrom = CDRom::default();
        // // let spu = spu::Spu::default();
        // let irqctl = irq::InterruptController::default();
        // let sio = sio::Sio::new();
        // let mdec = mdec::Mdec::new();
        // let mut scheduler = scheduler::Scheduler::default();
        // scheduler.init();
        // let timers = timers::Timers::new();
        // let (sender, receiver, handle) =
        //     renderer::Renderer::create();
        //
        // let gpu = gpu::Gpu::new(sender, receiver, handle);
        // let block_cache = block_cache::BlockCache::new();
        // let system = system::System::new(
        //     bios, ram, scratchpad, dma, spu, irqctl, scheduler, timers, cdrom, sio, mdec, gpu, block_cache,
        // );
        // let interpreter = cpu::Cpu::new(system);
        let interpreter = interpreter::Interpreter::new();

        // I trust you, linux scheduler, but..
        let core_ids = core_affinity::get_core_ids().unwrap();
        let res = core_affinity::set_for_current(core_ids[0]);
        if res {
            info!("main thread pinned to 0");
        }

        let (audio_stream, audio_sender) = crate::audio::build_audio_stream()?;
        // let file = File::create("output.pcm")?;
        // let mut writer = BufWriter::new(file);

        let mut state = Self {
            instance,
            surface: Some(surface),
            device,
            queue,
            config,
            is_surface_configured: false,
            // render_pipeline,
            window,
            clear_color: wgpu::Color::BLACK,
            // render_finished: std::time::Instant::now(),
            // diffuse_bind_group,
            // camera,
            // camera_buffer,
            uniforms_buffer,
            uniforms_bind_group,
            // camera_controller,
            render_finished: web_time::Instant::now(),
            render_time_ms: 0,
            frame_num: 0,
            // tilemap,
            vertex_buffer,
            pipeline,
            texture_bind_group_layout,
            // texture_params_buffer,
            // tileset_params_buffer,
            // tileset_texture,
            // tileset_bind_group,
            // tilemap_params_buffer,
            // tilemap_index_texture,
            texture_bind_group,
            render_texture,
            framebuffer: Arc::new(Mutex::new(vec![0; 1024 * 512])),
            texture_size,

            draw_pipeline,
            draw_texture,
            draw_vertex_buffer,
            draw_texture_bind_group,
            // image_rgba,
            // dimensions,
            // game_state,
            // sprites,
            // normal_font,
            // normal_blue_font,
            // small_font,
            // sounds
            system,
            interpreter,
            audio_stream,
            audio_sender,
            paused: false,
            gilrs: Some(Gilrs::new().unwrap()),
            active_gamepad: None,
            output_width: 0,
            output_height: 0,
            display_vram: false,
            software_render,
            sx: 0,
            sy: 0,
            display_depth: DisplayDepth::D15Bits, // writer,
            sideload_exe: cli
                .sideload
                .then(|| cli.filename.expect("sideload binary to be present")),
            ctrl_pressed: false,
            negcon: cli.negcon,
            use_interpreter: cli.interpreter,
            scale,
            mouse1: cli.mouse1,
            mouse2: cli.mouse2,
            mouse_pos: (0.0, 0.0),
            mouse_moved: false,
            guncon: cli.guncon,
            justifier: cli.justifier,
            line_start_cycle: 0,
            // ff_effect: None
        };

        for (id, gamepad) in state.gilrs.as_ref().unwrap().gamepads() {
            info!("{} is {:?} ; ff supported? {}", gamepad.name(), gamepad.power_info(), gamepad.is_ff_supported());
            if gamepad.name().eq("Microsoft Xbox Controller") {
                state.active_gamepad = Some(id)
            }
        }

        // set_camera(&state, &state.queue, FULLSCREEN_QUAD_CAMERA);
        set_uniforms(&state, &state.queue);

        State::upload_framebuffer(&state);
        // for _ in 0..120*735 {
        //     state.audio_sender.send([0i16, 0i16]).expect("can't send audio sample");
        // }
        // State::sideload_exe(&mut state);
        state.audio_stream.play()?;
        //

        Ok(state)
    }

    pub fn upload_framebuffer(&self) {
        if self.software_render {
            self.queue.write_texture(
                // Tells wgpu where to copy the pixel data
                wgpu::TexelCopyTextureInfo {
                    texture: &self.render_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                // The actual pixel data
                bytemuck::cast_slice(&self.framebuffer.lock().expect("ok")),
                // The layout of the texture
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * 1024),
                    rows_per_image: Some(512),
                },
                self.texture_size,
            );
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            let surface = self.surface.as_ref().unwrap();
            surface.configure(&self.device, &self.config);
            self.is_surface_configured = true;
            self.update_vertex_buffer_if_needed(
                self.output_width,
                self.output_height,
                self.sx,
                self.sy,
                self.display_depth,
                true,
            );
            // self.depth_texture =
            //     texture::Texture::create_depth_texture(&self.device, &self.config, "depth_texture");
        }
    }
    fn sideload_exe_dynarec(&mut self) {
        let Some(filename) = self.sideload_exe.take() else {
            panic!("not filename specified");
        };

        let mut file = match std::fs::File::open(filename) {
            Ok(file) => file,
            Err(e) => panic!("Can't load exe {}", e),
        };

        let mut data: Vec<u8> = Vec::new();
        let _file_size = match file.read_to_end(&mut data) {
            Ok(x) => x,
            Err(e) => panic!("Can't read exe {}", e),
        };
        // exe header
        let initial_pc = u32::from_le_bytes(data[0x10..0x14].try_into().unwrap());
        let initial_r28 = u32::from_le_bytes(data[0x14..0x18].try_into().unwrap());
        let exe_ram_addr = u32::from_le_bytes(data[0x18..0x1C].try_into().unwrap()) & 0x001F_FFFF;
        let exe_size = u32::from_le_bytes(data[0x1C..0x20].try_into().unwrap()) as usize;
        let initial_sp = u32::from_le_bytes(data[0x30..0x34].try_into().unwrap());

        // exe_ram_addr = crate::system::mask_region(exe_ram_addr);
        info!("exe ram addr: 0x{:X}", exe_ram_addr);
        info!("initial pc: 0x{:X}", initial_pc);

        // let exe_size = (file_size - 2048) as u32;
        // let exe_size = (exe_size_2kb);
        // let exe_size = 1013760 - 2048;
        info!("exe size: {}", exe_size);
        self.system.ram.data[exe_ram_addr as usize..(exe_ram_addr as usize + exe_size)]
            .copy_from_slice(&data[2048..2048 + exe_size as usize]);
        // self.system.block_cache.invalidate();
        //  let dest = self
        //     .cpu.system.ram.data
        //     .bytes()
        //     .get_mut(exe_ram_addr as usize..exe_ram_addr as usize + exe_size)
        //     .context("EXE load address out of RAM bounds")?;
        //
        // let src = data
        //     .get(2048..2048 + exe_size)
        //     .context("EXE file truncated")?;
        //
        // dest.copy_from_slice(src);

        self.system.cpu.regs.regs[28] = initial_r28;
        if initial_sp != 0 {
            self.system.cpu.regs.regs[29] = initial_sp;
            self.system.cpu.regs.regs[30] = initial_sp;
        }
        self.system.cpu.regs.pc = initial_pc;
    }

    /*fn sideload_exe(&mut self) {
        let Some(filename) = &self.sideload_exe else {
            panic!("not filename specified");
        };


        // let filename = "/foo/psxtest_cpu.exe";
        // let filename = "/foo/psxtest_gte.exe";
        // let filename = "/foo/psxtest_gpu.exe";
        // let filename = "/foo/psx/PSX/CPUTest/CPU/LOADSTORE/LB/CPULB.exe";
        // let filename = "/foo/psx/PSX/GPU/16BPP/MemoryTransfer/MemoryTransfer16BPP.exe";
        // let filename = "/foo/psx/PSX/Cube/Cube.exe";
        // let filename = "/foo/psx/PSX/GPU/16BPP/RenderTextureRectangle/CLUT4BPP/RenderTextureRectangleCLUT4BPP.exe";
        // let filename = "/foo/psx/PSX/GPU/16BPP/RenderTextureRectangle/CLUT8BPP/RenderTextureRectangleCLUT8BPP.exe";
        // let filename = "/foo/psx/PSX/GPU/16BPP/RenderTextureRectangle/15BPP/RenderTextureRectangle15BPP.exe";
        // let filename = "/foo/psx/PSX/GPU/16BPP/RenderLine/RenderLine16BPP.exe";
        let mut file = match std::fs::File::open(filename) {
            Ok(file) => file,
            Err(e) => panic!("Can't load exe {}", e),
        };
        // let mut file = match std::fs::File::open() {
        //     Ok(file) => file,
        //     Err(e) => panic!("Can't load exe {}", e),
        // };
        let mut data: Vec<u8> = Vec::new();
        let file_size = match file.read_to_end(&mut data) {
            Ok(x) => x,
            Err(e) => panic!("Can't read exe {}", e),
        };
        // while self.cpu.pc != 0x8003_0000 {
        //     self.cpu.run_next_instruction();
        //     self.cpu.check_for_tty_output();
        // }

        // exe header
        let initial_pc = u32::from_le_bytes(data[0x10..0x14].try_into().unwrap());
        let initial_r28 = u32::from_le_bytes(data[0x14..0x18].try_into().unwrap());
        let exe_ram_addr = u32::from_le_bytes(data[0x18..0x1C].try_into().unwrap()) & 0x001F_FFFF;
        let exe_size = u32::from_le_bytes(data[0x1C..0x20].try_into().unwrap()) as usize;
        let initial_sp = u32::from_le_bytes(data[0x30..0x34].try_into().unwrap());

        // exe_ram_addr = crate::system::mask_region(exe_ram_addr);
        println!("exe ram addr: 0x{:X}", exe_ram_addr);
        println!("initial pc: 0x{:X}", initial_pc);

        // let exe_size = (file_size - 2048) as u32;
        // let exe_size = (exe_size_2kb);
        // let exe_size = 1013760 - 2048;
        println!("exe size: {}", exe_size);
        self.interpreter.system.ram.data[exe_ram_addr as usize..(exe_ram_addr as usize + exe_size)]
            .copy_from_slice(&data[2048..2048 + exe_size as usize]);

        //  let dest = self
        //     .cpu.system.ram.data
        //     .bytes()
        //     .get_mut(exe_ram_addr as usize..exe_ram_addr as usize + exe_size)
        //     .context("EXE load address out of RAM bounds")?;
        //
        // let src = data
        //     .get(2048..2048 + exe_size)
        //     .context("EXE file truncated")?;
        //
        // dest.copy_from_slice(src);

        self.interpreter.set_reg(28, initial_r28);
        if initial_sp != 0 {
            self.interpreter.set_reg(29, initial_sp);
            self.interpreter.set_reg(30, initial_sp);
        }
        self.interpreter.pc = initial_pc;
        self.interpreter.next_pc = initial_pc + 4;

    }*/

    fn update_vertex_buffer_if_needed(
        &mut self,
        width: usize,
        height: usize,
        sx: usize,
        sy: usize,
        depth: DisplayDepth,
        force: bool,
    ) {
        if !force
            && width == self.output_width
            && height == self.output_height
            && sx == self.sx
            && sy == self.sy
            && depth == self.display_depth
        {
            return;
        }
        self.output_width = width;
        self.output_height = height;
        self.display_depth = depth;
        self.sx = sx;
        self.sy = sy;

        // let (startx, starty, width, height) = self.system.gpu.update_crt_display_params();
        // let width = width as usize;
        // let height = height as usize;
        // let sx = startx as usize;
        // let sy = starty as usize;

        // println!("vram: {} x {} start: {}x{}; display: {} x {} start: {} x {}", w, h, sx, sy, width, height, _sx, _sy);

        let scale = height as f32 / self.config.height as f32;
        let mut w = width as f32 / scale / self.config.width as f32;
        let mut h: f32 = 1.0;
        let mut bx: f32 = (1.0 - w) / 2.0;
        let mut by: f32 = 0.0;

        if w * width as f32 > self.output_width as f32 {
            let scale = width as f32 / self.config.width as f32;
            h = height as f32 / scale / self.config.height as f32;
            w = 1.0;
            bx = 0.0;
            by = (1.0 - h) / 2.0;
        }

        // let u = width as f32 / 1024.0;
        // let v = height as f32 / 512.0;
        let u = ((width + sx) as f64 / (1024.0 * self.scale as f64)) as f32;
        let v = ((height + sy) as f64 / (512.0 * self.scale as f64)) as f32;

        let sx = (sx as f64 / (1024.0 * self.scale as f64)) as f32;
        let sy = (sy as f64 / (512.0 * self.scale as f64)) as f32;

        // self.config.width = width;
        // self.config.height = height;
        let vertices: &[Vertex] = &[
            Vertex {
                position: [bx, by, 0.0],
                uv: [sx, v],
            },
            Vertex {
                position: [bx + w, by, 0.0],
                uv: [u, v],
            },
            Vertex {
                position: [bx, by + h, 0.0],
                uv: [sx, sy],
            },
            Vertex {
                position: [bx, by + h, 0.0],
                uv: [sx, sy],
            },
            Vertex {
                position: [bx + w, by, 0.0],
                uv: [u, v],
            },
            Vertex {
                position: [bx + w, by + h, 0.0],
                uv: [u, sy],
            },
            // Vertex { position: [0.0, 0.0, 0.0], uv: [0.0, 0.0] },
            // Vertex { position: [w, 0.0, 0.0], uv: [u, 0.0] },
            // Vertex { position: [0.0, 1.0, 0.0], uv: [0.0, v] },
            // Vertex { position: [0.0, 1.0, 0.0], uv: [0.0, v] },
            // Vertex { position: [w, 0.0, 0.0], uv: [u, 0.0] },
            // Vertex { position: [w, 1.0, 0.0], uv: [u, v] },
        ];
        self.queue
            .write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(vertices));
        self.output_width = width;
        self.output_height = height;
        set_uniforms(self, &self.queue);
        self.display_depth = depth;
    }

    #[cold]
    fn call_interpreter(&mut self) {
        let num_cycles = 20;
        for _ in 1..num_cycles {
            self.interpreter.run_next_instruction(&mut self.system);
            // self.interpreter.check_for_tty_output();
        }
        cdrom::CDRom::tick(&mut self.system, 2 * num_cycles);
        self.system.scheduler.advance(2 * num_cycles); // 40???
    }
    fn update(&mut self, _event_loop: &ActiveEventLoop) {
        self.update_gamepad();
        if self.paused {
            println!("current pc: 0x{:X}", self.system.cpu.regs.pc);
            // self.system.irqctl.status.set_dma(false);
            // self.cpu.system.irqctl.status.set_ctl_mem(true);
            // self.cpu.pc = self.cpu.pc + 4;
            // self.cpu.next_pc = self.cpu.pc + 8;
            self.paused = false;
            // self.system.store(0x800A0874, 0_u32);
            // crate::cdrom::CDRom::write_command(&mut self.system, 0x3); //play
            // self.system.cdrom.write_command();
            return;
        }
        loop {
            if let Some(event) = self.system.scheduler.get_next_event() {
                match event {
                    scheduler::Event::SpuTick => {
                        Spu::clock(&mut self.system);
                        // self.cpu.system.spu.clock();
                        // let spu = &mut self.cpu.system.spu;
                        // let cdrom = &mut self.cpu.system.cdrom;
                        // let sample = self.cpu.system.spu.mix(cdrom);
                        let sample = Spu::mix(&mut self.system);
                        self.audio_sender
                            .send(sample)
                            .expect("can't send audio sample");
                        // if self.audio_sender.len() > 4*735 { // 2.5 * 735
                        //     break;
                        // }
                        //
                        // self.audio_tick += 1;
                        // if self.audio_tick == 735 {
                        //     self.audio_tick = 0;
                        //    break;
                        // }

                        // self.audio_buffer.push(sample);
                        // if self.audio_buffer.len() == 20*735 {
                        //     self.audio_buffer.clear();
                        //     break;
                        // }

                        // self.writer.write_all(&sample[0].to_le_bytes()).expect("foo");
                        // self.cpu.system.spu.clock();
                        // self.audio_buffer.push(sample);
                        // if self.audio_buffer.len() == 735 {
                        //     self.audio_buffer.iter().for_each(|sample| {
                        //         self.audio_sender.send(*sample).expect("can't send audio sample");
                        //     });
                        //     self.audio_buffer.clear();
                        //
                        // }
                    }
                    scheduler::Event::VBlankStart => {
                        // self.cpu.system.gpu_sender.send(gpu::GpuMsg::ProduceFB(self.framebuffer.clone(), self.display_vram)).expect("ok");
                        let (w, h, sx, sy, depth) = self
                            .system
                            .gpu
                            .render_fb(self.framebuffer.clone(), self.display_vram);
                        self.update_vertex_buffer_if_needed(w, h, sx, sy, depth, false);
                        // if self.cpu.system.gpu.interrupt == false {
                        self.system.irqctl.set_vblank();
                        // }
                        timers::Timers::enter_vsync(&mut self.system);
                        // self.cpu.system.gpu_sender.send(gpu::GpuMsg::EnterVSync).expect("ok");
                        self.system.gpu.enter_vsync();
                    }
                    scheduler::Event::VBlankEnd => {
                        // self.cpu.system.gpu_sender.send(gpu::GpuMsg::ExitVSync).expect("ok");
                        self.system.gpu.exit_vsync();
                        timers::Timers::exit_vsync(&mut self.system);
                        sio::save_memcards(&mut self.system);
                        // let (w, h) = self.cpu.system.gpu_ctrl_receiver.recv().expect("ok");
                        // self.update_vertex_buffer_if_needed(w, h, false);
                        if self.audio_sender.len() > 5 * 735 {
                            // 2.5 * 735
                            break;
                        }
                    }
                    scheduler::Event::HBlankStart => {
                        // self.cpu.system.gpu_sender.send(gpu::GpuMsg::EnterHSync).expect("ok");
                        self.system.gpu.enter_hsync();
                        timers::Timers::enter_hsync(&mut self.system);
                    }
                    scheduler::Event::HBlankEnd => {
                        // self.cpu.system.gpu_sender.send(gpu::GpuMsg::ExitHSync).expect("ok");
                        self.system.gpu.exit_hsync();
                        timers::Timers::exit_hsync(&mut self.system);
                        self.line_start_cycle = self.system.scheduler.cycle;
                    }
                    scheduler::Event::CDRomResultIrq(_resp) => {
                        // cdrom::CDRom::process_response(&mut self.system, resp);
                    }
                    scheduler::Event::Timer(i) => {
                        timers::Timers::process_interrupt(&mut self.system, i)
                    }
                    scheduler::Event::SerialSend => Sio::process_serial_send(&mut self.system),
                    scheduler::Event::DsrOff => self.system.sio.turn_dsr_off(),
                    scheduler::Event::Dma => dma::dma_event(&mut self.system),
                    scheduler::Event::LightGun(tick, first, last) => {
                        self.system.scheduler.unschedule_light_gun();
                        let Some(justifier) = self
                            .system
                            .sio
                            .gamepad1
                            .as_any()
                            .downcast_mut::<Justifier>()
                        else {
                            panic!("oop failed");
                        };

                        if !justifier.shoot_offscreen {
                            self.system.irqctl.set_ctl_light();
                        }
                        let line = self.system.gpu.current_line;
                        // let current_cycle = (self.system.scheduler.cycle - self.line_start_cycle);
                        // let current_tick = (current_cycle * 11 / 7) as u16;
                        // let tmr: u32 = crate::timers::load(&mut self.system, 0);
                        // self.system.timers.timers[0].counter = tick  as u32  / 5;
                        // self.system.timers.timers[0].counter = tick as u32 / 2;
                        // self.system.timers.timers[1].counter = line as u32;
                        // let t1 = self.system.timers.timers[1].counter;
                        if self.system.timers.timers[0].counter !=  0 {
                          println!("t0: {}", self.system.timers.timers[0].counter);
                        }
                        // const NTSC_FREQ: f64 = 53693175.0;
                        // let divider = NTSC_FREQ / 8000000.0;
                        // let total = ticks as u64 + lines as u64 * NTSC_TICKS_PER_LINE;
                        // let tk = (tick as f64 - 140.0) / divider; 
                        // println!("target_pos: {}, {}", tk as u32,line);
                        // self.system.timers.timers[1].counter = line;
                        // println!("timer: {}", tmr);
                        let target_line = if line < first as u32 || line > last as u32 {
                            first
                        } else {
                            // println!("target: {}, current: {}", tick, current_tick);
                            // let error = tick as i32 - current_tick as i32;
                            // if error.abs() < 50 {
                            // }
                            // if tick > current_tick {
                            /*  */

                            // } else {
                            // (line as u16 + 1, 3413 - tick + current_tick)
                            // }
                            line as u16 + 1
                        };
                        let cycles = self.cycles_until_line_and_tick(target_line, tick);
                        self.system.scheduler.schedule(
                            scheduler::Event::LightGun(tick, first, last),
                            cycles,
                            None,
                        );
                    }
                }
            }
            // self.update_interpreter();
            if self.use_interpreter {
                self.call_interpreter();
                continue;
            }

            // ok everything works, except memory card... check that later
            // let budget = self.cpu.system.scheduler.next_event_budget();
            // TODO:
            //if !pending_interrupt || (is_gte && !self.delay_slot) {

            // let _pc = self.system.cpu.regs.pc;
            // // let instr = crate::cpu::Instruction(self.system.load(_pc));
            // let is_gte = (instr.0 & 0xFE00_0000) == 0x4A00_0000;
            // if Dynarec::check_for_pending_interrupts(&mut self.system) && !is_gte {

            if Dynarec::check_for_pending_interrupts(&mut self.system) {
                self.system.cpu.external_interrupt();
            }
            if self.sideload_exe.is_some() && self.system.cpu.regs.pc == 0x8003_0000 {
                // self.sideload_exe();
                self.sideload_exe_dynarec();
            }
            if self.system.cpu.regs.pc & 3 != 0 {
                let baddr = self.system.cpu.regs.pc;
                self.system
                    .cpu
                    .exception(crate::dynarec::Exception::LoadAddressError, Some(baddr));
            }
            let pc = self.system.cpu.pc();

            Dynarec::receive_compiled_blocks(&mut self.system);

            // let block_idx = None;
            let block_idx = if let Some(entry) = self.system.block_cache.get_entry(pc) {
                if self
                    .system
                    .block_cache
                    .is_block_dirty(entry.block_idx.unwrap())
                {
                    if !Dynarec::is_block_valid(&mut self.system, entry.block_idx.unwrap()) {
                        // recompile block...
                        Dynarec::schedule_compile_block(&mut self.system, pc);
                        // let block = Dynarec::compile_block(&mut self.system, pc);
                        // self.system
                        //     .block_cache
                        //     .insert_block_at(entry.block_idx, pc, block);
                        None
                    } else {
                        self.system.block_cache.mark_fresh(entry.block_idx.unwrap());
                        entry.block_idx
                    }
                } else {
                    entry.block_idx
                }
            } else {
                Dynarec::schedule_compile_block(&mut self.system, pc);
                None
                // let block = Dynarec::compile_block(&mut self.system, pc);
                // let idx = self.system.block_cache.insert_block(pc, block);
                // idx
                // compile new block...
            };
            // let mut block_list : HashSet<u32> = HashSet::new();
            // block_list.insert(0x80011028); // blocking causes unaligned addr
            // block_list.insert(0x80011010);
            // block_list.insert(0x80010FF8);

            // let num_cycles = if let Some(block_idx) = block_idx && !block_list.contains(&pc) {
            let num_cycles = if let Some(block_idx) = block_idx {
                Dynarec::run_block(&mut self.system, block_idx)
            } else {
                // run interpreter
                // continue;
                self.interpreter.next_pc = pc + 4;
                self.interpreter.run_block(&mut self.system)
            };

            // let next_pc = self.system.cpu.pc();
            // if next_pc == 0x8004A878 {
            //     // self.system.cpu.regs.regs[26] = 0x800A5EDC;
            //     println!("jumping to 0x8004A878 from {:X}", pc);
            //     println!("regs:");
            //     self.system.cpu.regs.pretty_print();
            //     // println!("{:?}", self.system.cpu.regs);
            // }
            // if next_pc == 0 {
            //     println!("jumping to 0 from {:X}", pc);
            // }
            // then run it...

            // let interpreter_pc = self.interpreter.pc;
            // self.interpreter.run_next_instruction_debug(true);
            // let num_cycles = 20;
            // for _ in 1..num_cycles {
            //     self.interpreter.run_next_instruction(&mut self.system);
            //     // self.interpreter.check_for_tty_output();
            // }
            // check_dynarec(pc, interpreter_pc, &self.cpu, &self.interpreter);
            cdrom::CDRom::tick(&mut self.system, 2 * num_cycles);
            self.system.scheduler.advance(2 * num_cycles); // 40???
        }
        //     for _ in 0..200 {
        //         self.cpu.run_next_instruction();
        //         self.cpu.check_for_tty_output();
        //     }
        // // println!(".");
        // for i in 0..735 {
        //     // for _ in 0..334 {
        //     for _ in 0..668 {
        //         self.cpu.run_next_instruction();
        //         self.cpu.check_for_tty_output();
        //     }
        //     if i == 500 {
        //                  self.cpu.system.irqctl.status.set_vblank(true);
        //     }
        //     self.cpu.system.spu.clock();
        //     audio_buffer.push(self.cpu.system.spu.mix());
        //     // self.audio_sender.send(self.cpu.system.spu.mix()).expect("can't send audio");
        // }
    }

    fn update_draw_texture_if_needed(
        &mut self,
        width: u32,
        height: u32,
        texture_format: wgpu::TextureFormat,
    ) {
        if self.draw_texture.width() != width || self.draw_texture.height() != height {
            info!("updating offscreen render (draw) texture");
            let draw_texture = self.device.create_texture(&wgpu::TextureDescriptor {
                size: wgpu::Extent3d {
                    width,
                    height,
                    // All textures are stored as 3D, we represent our 2D texture
                    // by setting depth to 1.
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // Most images are stored using sRGB, so we need to reflect that here.
                format: texture_format,
                // TEXTURE_BINDING tells wgpu that we want to use this texture in shaders
                // COPY_DST means that we want to copy data to this texture
                usage: wgpu::TextureUsages::TEXTURE_BINDING
            // | wgpu::TextureUsages::COPY_DST
            // | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
                label: Some("render texture"),
                // This is the same as with the SurfaceConfig. It
                // specifies what texture formats can be used to
                // create TextureViews for this texture. The base
                // texture format (Rgba8UnormSrgb in this case) is
                // always supported. Note that using a different
                // texture format is not supported on the WebGL2
                // backend.
                view_formats: &[texture_format],
            });
            self.draw_texture = draw_texture;

            // updating texture bind group....
            let texture_bind_group_layout =
                self.device
                    .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: Some("draw_texture_bind_group_layout"),
                        entries: &[
                            wgpu::BindGroupLayoutEntry {
                                binding: 0,
                                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                                ty: wgpu::BindingType::Texture {
                                    sample_type: wgpu::TextureSampleType::Float {
                                        filterable: true,
                                    },
                                    view_dimension: wgpu::TextureViewDimension::D2,
                                    multisampled: false,
                                },
                                count: None,
                            },
                            wgpu::BindGroupLayoutEntry {
                                binding: 1,
                                visibility: wgpu::ShaderStages::FRAGMENT,
                                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                                count: None,
                            },
                        ],
                    });
            let diffuse_sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Nearest,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            });
            let texture_view = self
                .draw_texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let texture_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("draw  texture bind_group"),
                layout: &texture_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&diffuse_sampler),
                    },
                ],
            });
            self.draw_texture_bind_group = texture_bind_group;
        }
    }
    fn render(&mut self, view: &wgpu::TextureView) {
        self.upload_framebuffer();
        self.window.request_redraw();

        let w = view.texture().width();
        let h = view.texture().height();
        self.update_draw_texture_if_needed(w, h, view.texture().format());

        {
            let draw_texture_view = self
                .draw_texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("draw_encoder"),
                });
            {
                let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("draw_rpass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &draw_texture_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            // load: wgpu::LoadOp::Load,
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            // load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    ..Default::default()
                });
                rpass.set_pipeline(&self.pipeline);
                rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                rpass.set_bind_group(0, &self.uniforms_bind_group, &[]);

                rpass.set_bind_group(1, &self.texture_bind_group, &[]);
                rpass.draw(0..6, 0..1);
            }
            self.queue.submit(vec![encoder.finish()]);
        }
        {
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("frame_encoder"),
                });
            {
                let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("surface_rpass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            // load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    ..Default::default()
                });
                rpass.set_pipeline(&self.draw_pipeline);
                rpass.set_vertex_buffer(0, self.draw_vertex_buffer.slice(..));
                // rpass.set_bind_group(0, &self.uniforms_bind_group, &[]);

                rpass.set_bind_group(0, &self.draw_texture_bind_group, &[]);
                rpass.draw(0..6, 0..1);
            }
            self.queue.submit(vec![encoder.finish()]);
        }
    }

    fn get_display_pos(&mut self, x: f64, y: f64) -> (f64, f64) {
        // I understant in ticks / lines
        let (_, _, width, height, hs, he, vs, ve) = self.system.gpu.update_crt_display_params();
        let scale = height as f64 / self.config.height as f64;
        let w = width as f64 / scale / self.config.width as f64;
        let h: f64;
        let mut bx: f64 = (1.0 - w) / 2.0;
        let mut by: f64 = 0.0;

        if w * width as f64 > self.output_width as f64 {
            let scale = width as f64 / self.config.width as f64;
            h = height as f64 / scale / self.config.height as f64;
            bx = 0.0;
            by = (1.0 - h) / 2.0;
        }
        let mx = (x - bx * self.config.width as f64)
              // / (self.config.width as f64);
            / (self.config.width as f64 - 2.0 * bx * self.config.width as f64);
        let my = (y - by * self.config.height as f64)
        // / self.config.height as f64;
            / (self.config.height as f64 - 2.0 * by * self.config.height as f64);

        // let mx = mx * (he as f64 - hs as  f64) / width as f64;// +  bx * width as f64;
        let mx = mx * 0.72; //TODO: why?, needed for guncon
        let my = my * 0.91;

        let mx = mx * 3413.0 + hs as f64;
        let my = my * 263.0 + vs as f64;
        // println!("mx:  {}, my: {}", mx, my);
        (mx, my)
    }

    fn get_pointer_pos(&mut self, x: f64, y: f64) -> (u16, u16) {
        let (ticks, lines) = self.get_display_pos(x, y);
        let tick_offset = -140.0;
        let ticks = ticks + tick_offset;

        const NTSC_FREQ: f64 = 53693175.0;
        // 8MHz units for X = 44100*768*11/7 = 53222400 / 8000000 = 6.6528
        let divider = NTSC_FREQ / 8000000.0;


        let m_pos_x = (ticks / divider) as u16;
        let m_pos_y = lines as u16;

        // println!(">>>> src pos: {} {}", m_pos_x, m_pos_y);

        (m_pos_x, m_pos_y)
    }

    pub fn cycles_until_line_and_tick(&mut self, line: u16, tick: u16) -> u64 {
        let mut current_line = self.system.gpu.current_line as u16;
        let current_cycle = (self.system.scheduler.cycle - self.line_start_cycle);
        let current_tick = current_cycle * 11 / 7;
        let fractional_ticks = 0; //current_cycle * 11 % 7;
        let ticks = if tick as u64 >= current_tick {
            tick as u64 - current_tick
        } else {
            current_line += 1;
            3413 + tick as u64 - current_tick
        };
        let lines = if line > current_line {
            (line - current_line)
        } else {
            263 - current_line + line
        };
        const NTSC_TICKS_PER_LINE: u64 = 3413;
        let total = ticks as u64 + lines as u64 * NTSC_TICKS_PER_LINE;
        let system_cycles = ((total * 451584 + fractional_ticks + 715908) / 715909);
        system_cycles
    }
    pub fn schedule_justifier_irq(&mut self) {
        self.system.scheduler.unschedule_light_gun();
        let (ticks, lines) = self.get_display_pos(self.mouse_pos.0, self.mouse_pos.1);

        const NTSC_TOTAL_LINES: u16 = 263;
        // const NTSC_FREQ: f64 = 53693175.0;
        // let divider = NTSC_FREQ / 8000000.0;
        // let m_irq_tick = (ticks * 5).saturating_sub(300);
        // 8MHz units for X = 44100*768*11/7 = 53222400 / 8000000 = 6.6528
        let m_irq_tick = ticks as u16;//(ticks * 5).saturating_sub(300);
        let lines  =  lines as u16;

        // println!("target tick: {} (0..3413) ", m_irq_tick);
        let start_line_ofsset = 12;
        let end_line_offset = 6;
        let m_irq_first_line = (lines + NTSC_TOTAL_LINES - start_line_ofsset) % NTSC_TOTAL_LINES;
        let m_irq_last_line = (lines + NTSC_TOTAL_LINES - end_line_offset) % NTSC_TOTAL_LINES;
        // let m_irq_first_line = (lines + NTSC_TOTAL_LINES - start_line_ofsset) % NTSC_TOTAL_LINES;
        // let m_irq_last_line = (lines + NTSC_TOTAL_LINES - end_line_offset) % NTSC_TOTAL_LINES;

        // self.justifier_irq = Some((m_irq_tick, m_irq_first_line, m_irq_last_line));
        let cycles = self.cycles_until_line_and_tick(lines, m_irq_tick);
        self.system.scheduler.schedule(
            scheduler::Event::LightGun(m_irq_tick, m_irq_first_line, m_irq_last_line),
            cycles,
            None,
        );
        // self.system.scheduler.schedule(scheduler::Event::LightGun(1), sys_cycles, None);
        // self.system.scheduler.schedule(scheduler::Event::LightGun(2), system_cycles + 140, None);
        // self.system.scheduler.schedule(scheduler::Event::LightGun(3), sys_cycles + 140, None);
    }

    fn handle_mouse_moved(&mut self, x: f64, y: f64) {
        self.mouse_moved = true;
        if self.guncon {
            let (mx, my) = self.get_pointer_pos(x, y);
            let [b0, b1] = (mx as u16).to_le_bytes();
            let [b2, b3] = (my as u16).to_le_bytes();
            self.system.sio.gamepad1.set_stick_axis((b2, b3), (b0, b1));
        } else if self.mouse1 || self.mouse2 {
            //            0-7   Horizontal Motion (-80h..+7Fh = Left..Right) (00h=No motion)
            // 8-15  Vertical Motion   (-80h..+7Fh = Up..Down)    (00h=No motion)
            // println!(">> moved, x :{}, y: {}", x, y);
            let dx = x - self.mouse_pos.0;
            let dy = y - self.mouse_pos.1;
            let width = if self.output_width == 0 {
                1
            } else {
                self.output_width / 2
            };
            let height = if self.output_height == 0 {
                1
            } else {
                self.output_height / 2
            };
            self.mouse_pos.0 = x;
            self.mouse_pos.1 = y;
            let hsensitivity = 2.0;
            let vsensitivity = 1.8;
            let hdeadzone = 0.009;
            let vdeadzone = 0.005;
            let h = ((dx / width as f64) * hsensitivity);
            let v = ((dy / height as f64) * vsensitivity);
            let h = if h.abs() < hdeadzone { 0.0 } else { h };
            let v = if v.abs() < vdeadzone { 0.0 } else { v };
            let h = h.mul(127.5) as i8 as u8;
            let v = v.mul(127.5) as i8 as u8;
            let mouse = if self.mouse1 {
                &mut self.system.sio.gamepad1
            } else {
                &mut self.system.sio.gamepad2
            };
            mouse.set_stick_axis((0, 0), (h, v));
        }
        self.mouse_pos.0 = x;
        self.mouse_pos.1 = y;
        // self.clear_color.r = x / self.config.width as f64;
        // self.clear_color.g = y / self.config.height as f64;
    }

    fn acquire_surface(&mut self, window: Arc<Window>) -> Option<wgpu::SurfaceTexture> {
        use wgpu::CurrentSurfaceTexture;

        let surface = self.surface.as_ref().unwrap();

        match surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame) => Some(frame),
            CurrentSurfaceTexture::Timeout | CurrentSurfaceTexture::Occluded => None,
            CurrentSurfaceTexture::Suboptimal(texture) => {
                drop(texture);
                surface.configure(&self.device, &self.config);
                match surface.get_current_texture() {
                    CurrentSurfaceTexture::Success(frame)
                    | CurrentSurfaceTexture::Suboptimal(frame) => Some(frame),
                    other => panic!("Failed to acquire next surface texture: {other:?}"),
                }
            }
            CurrentSurfaceTexture::Outdated => {
                surface.configure(&self.device, &self.config);
                match surface.get_current_texture() {
                    CurrentSurfaceTexture::Success(frame)
                    | CurrentSurfaceTexture::Suboptimal(frame) => Some(frame),
                    other => panic!("Failed to acquire next surface texture: {other:?}"),
                }
            }
            CurrentSurfaceTexture::Validation => {
                unreachable!("No error scope registered so validation errors will panic")
            }
            CurrentSurfaceTexture::Lost => {
                self.surface = Some(self.instance.create_surface(window).unwrap());
                self.surface
                    .as_ref()
                    .unwrap()
                    .configure(&self.device, &self.config);
                match self.surface.as_ref().unwrap().get_current_texture() {
                    CurrentSurfaceTexture::Success(frame)
                    | CurrentSurfaceTexture::Suboptimal(frame) => Some(frame),
                    other => panic!("Failed to acquire next surface texture: {other:?}"),
                }
            }
        }
    }

    fn handle_key(
        &mut self,
        _event_loop: &ActiveEventLoop,
        code: KeyCode,
        key_state: ElementState,
        _logical_key: Key,
        _repeat: bool,
    ) {
        if code == KeyCode::ControlLeft || code == KeyCode::ControlRight {
            self.ctrl_pressed = key_state.is_pressed();
        }
        match (code, key_state.is_pressed()) {
            (KeyCode::Space, false) => self.paused = !self.paused,
            (KeyCode::KeyV, false) => self.display_vram = !self.display_vram,
            (KeyCode::Digit1, false) => {
                if self.ctrl_pressed {
                    load_state(1, &mut self.system);
                } else {
                    save_state(1, &mut self.system);
                }
            }
            (KeyCode::Digit2, false) => {
                if self.ctrl_pressed {
                    load_state(2, &mut self.system);
                } else {
                    save_state(2, &mut self.system);
                }
            }
            (KeyCode::Digit3, false) => {
                if self.ctrl_pressed {
                    load_state(3, &mut self.system);
                } else {
                    save_state(3, &mut self.system);
                }
            }
            (KeyCode::Digit4, false) => {
                if self.ctrl_pressed {
                    load_state(4, &mut self.system);
                } else {
                    save_state(4, &mut self.system);
                }
            }
            (KeyCode::Digit5, false) => {
                if self.ctrl_pressed {
                    load_state(5, &mut self.system);
                } else {
                    save_state(5, &mut self.system);
                }
            }
            (KeyCode::Enter, false) => {
                load_state(1, &mut self.system);
            }
            (KeyCode::KeyA, true) => {
                let buttons = self.system.sio.gamepad1.get_buttons();
                self.system.sio.gamepad1.set_buttons(buttons & !(1 << 3));
            }
            (KeyCode::KeyA, false) => {
                let buttons = self.system.sio.gamepad1.get_buttons();
                self.system.sio.gamepad1.set_buttons(buttons | (1 << 3));
            }
            (KeyCode::KeyF, true) => {
                let buttons = self.system.sio.gamepad1.get_buttons();
                self.system.sio.gamepad1.set_buttons(buttons & !(1 << 14));
            }
            (KeyCode::KeyF, false) => {
                let buttons = self.system.sio.gamepad1.get_buttons();
                self.system.sio.gamepad1.set_buttons(buttons | (1 << 14));
            }
            (KeyCode::KeyQ, true) => {
                let buttons = self.system.sio.gamepad1.get_buttons();
                self.system.sio.gamepad1.set_buttons(buttons & !(1 << 3));
            }
            (KeyCode::KeyQ, false) => {
                let buttons = self.system.sio.gamepad1.get_buttons();
                self.system.sio.gamepad1.set_buttons(buttons | (1 << 3));
            }
            // (KeyCode::ArrowRight, true) => self.game_state.camera_pos.0 += 33.0,
            // (KeyCode::ArrowLeft, true) => if self.game_state.camera_pos.0 >= 33.0 {self.game_state.camera_pos.0 -= 33.0 },
            // (KeyCode::ArrowUp, true) => if self.game_state.camera_pos.1 >= 33.0  {self.game_state.camera_pos.1 -= 33.0},
            // (KeyCode::ArrowDown, true) => self.game_state.camera_pos.1 += 33.0,
            // (KeyCode::ArrowRight, true) => {
            //     self.game_state.last_input = Some((String::from("ArrowRight"), repeat));
            // }
            // (KeyCode::ArrowLeft, true) => {
            //     if self.game_state.camera_pos.0 >= 33.0 {
            //         self.game_state.camera_pos.0 -= 33.0
            //     }
            // }
            // (KeyCode::ArrowUp, true) => {
            //     if self.game_state.camera_pos.1 >= 33.0 {
            //         self.game_state.camera_pos.1 -= 33.0
            //     }
            // }
            // (KeyCode::ArrowDown, true) => self.game_state.camera_pos.1 += 33.0,
            // (_, false) => self.game_state.last_input = None,
            // (KeyCode::Escape, true) => {
            //     self.game_state.last_input = Some((String::from("Escape"), repeat));
            // }
            // (KeyCode::Enter, true) => {
            //     self.game_state.last_input = Some((String::from("Enter"), repeat));
            // }
            // (KeyCode::Backspace, true) => {
            //     self.game_state.last_input = Some((String::from("Backspace"), repeat));
            // }
            // (KeyCode::ShiftLeft, true) => {
            //     self.game_state.last_input = Some((String::from("Shift"), repeat));
            // }
            // (KeyCode::ShiftRight, true) => {
            //     self.game_state.last_input = Some((String::from("Shift"), repeat));
            // }
            // (KeyCode::Escape, true) => event_loop.exit(),
            _ => {
                // match logical_key.to_text() {
                //     Some(_str) => {
                //         // self.game_state.last_input = Some((String::from(str), repeat));
                //     }
                //     _ => {}
                // }
                // self.camera_controller.handle_key(code, is_pressed);
            }
        }
    }

    fn handle_mouse_button(
        &mut self,
        button: MouseButton,
        is_pressed: bool,
        // _event_loop: &ActiveEventLoop,
        // code: KeyCode,
        // key_state: ElementState,
        // _logical_key: Key,
        // _repeat: bool,
    ) {
        if self.justifier {
            let buttons = self.system.sio.gamepad1.get_buttons();
            match (button, is_pressed) {
                (MouseButton::Left, true) => {
                    self.system.sio.gamepad1.set_buttons(buttons & !(1 << 15));
                    // aha..
                    // self.system.irqctl.set_ctl_light();
                    self.schedule_justifier_irq();
                }
                (MouseButton::Left, false) => {
                    self.system.sio.gamepad1.set_buttons(buttons | (1 << 15));
                }
                (MouseButton::Right, true) => {
                    let Some(justifier) = self
                        .system
                        .sio
                        .gamepad1
                        .as_any()
                        .downcast_mut::<Justifier>()
                    else {
                        panic!("oop failed");
                    };
                    justifier.shoot_offscreen = true;
                }
                (MouseButton::Right, false) => {
                    let Some(justifier) = self
                        .system
                        .sio
                        .gamepad1
                        .as_any()
                        .downcast_mut::<Justifier>()
                    else {
                        panic!("oop failed");
                    };
                    justifier.shoot_offscreen = false;
                }
                _ => {}
            }
        } else if self.guncon {
            let buttons = self.system.sio.gamepad1.get_buttons();
            match (button, is_pressed) {
                (MouseButton::Left, true) => {
                    self.system.sio.gamepad1.set_buttons(buttons & !(1 << 13));
                }
                (MouseButton::Left, false) => {
                    self.system.sio.gamepad1.set_buttons(buttons | (1 << 13));
                }
                (MouseButton::Right, true) => {
                    let Some(guncon) = self.system.sio.gamepad1.as_any().downcast_mut::<GunCon>()
                    else {
                        panic!("oop failed");
                    };
                    guncon.shoot_offscreen = true;
                }
                (MouseButton::Right, false) => {
                    let Some(guncon) = self.system.sio.gamepad1.as_any().downcast_mut::<GunCon>()
                    else {
                        panic!("oop failed");
                    };
                    guncon.shoot_offscreen = false;
                }
                _ => {}
            }
        } else {
            let mouse = if self.mouse1 {
                &mut self.system.sio.gamepad1
            } else {
                &mut self.system.sio.gamepad2
            };
            match (button, is_pressed) {
                (MouseButton::Left, true) => {
                    let btns = mouse.get_buttons();
                    mouse.set_buttons(btns & !(1 << 11));
                }
                (MouseButton::Left, false) => {
                    let btns = mouse.get_buttons();
                    mouse.set_buttons(btns | (1 << 11));
                }
                (MouseButton::Right, true) => {
                    let btns = mouse.get_buttons();
                    mouse.set_buttons(btns & !(1 << 10));
                }
                (MouseButton::Right, false) => {
                    let btns = mouse.get_buttons();
                    mouse.set_buttons(btns | (1 << 10));
                }
                _ => {}
            }
        }
    }

    pub fn update_gamepad(&mut self) {
        if self.gilrs.is_none() {
            self.gilrs = self.system.gilrs.take(); // hmm, better push it  to system altogether..
        }
        if self.mouse1 || self.mouse2 {
            if !self.mouse_moved {
                let mouse = if self.mouse1 {
                    &mut self.system.sio.gamepad1
                } else {
                    &mut self.system.sio.gamepad2
                };
                if self.mouse_pos.0 < 20.0 {
                    mouse.set_stick_axis((0, 0), (0xc0, 0x0));
                } else if self.mouse_pos.0 > self.config.width.saturating_sub(20) as f64 {
                    mouse.set_stick_axis((0, 0), (0x40, 0x0));
                } else if self.mouse_pos.1 < 20.0 {
                    mouse.set_stick_axis((0, 0), (0x0, 0xc0));
                } else if self.mouse_pos.1 > self.config.height.saturating_sub(20) as f64 {
                    mouse.set_stick_axis((0, 0), (0x0, 0x40));
                } else {
                    mouse.set_stick_axis((0, 0), (0x0, 0x0));
                }
            } else {
                self.mouse_moved = false;
            }
        }
        if self.negcon {
            Self::update_negcon(self);
            return;
        }
        let mut prev_buttons = self.system.sio.gamepad1.get_buttons();
        while let Some(Event {
            id: _,
            event,
            time: _,
            ..
        }) = self.gilrs.as_mut().unwrap().next_event()
        {
            // println!("{:?} New event from {}: {:?}", time, id, event);
            match event {
                gilrs::EventType::ButtonPressed(button, _) => match button {
                    gilrs::Button::South => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Cross as usize));
                    } // Cross
                    gilrs::Button::East => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Circle as usize));
                    } // Circle
                    gilrs::Button::North => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Triangle as usize));
                    } // Triangle
                    gilrs::Button::West => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Square as usize));
                    } // Square
                    gilrs::Button::Select => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Select as usize));
                    }
                    gilrs::Button::Start => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Start as usize));
                    }
                    gilrs::Button::DPadUp => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Up as usize));
                    }
                    gilrs::Button::DPadDown => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Down as usize));
                    }
                    gilrs::Button::DPadLeft => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Left as usize));
                    }
                    gilrs::Button::DPadRight => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Right as usize));
                    }
                    gilrs::Button::LeftTrigger => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::L1 as usize));
                    }
                    gilrs::Button::RightTrigger => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::R1 as usize));
                    }
                    _ => {} // ignore..
                },
                gilrs::EventType::ButtonReleased(button, _) => match button {
                    gilrs::Button::South => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Cross as usize);
                    } // Cross
                    gilrs::Button::East => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Circle as usize);
                    } // Circle
                    gilrs::Button::North => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Triangle as usize);
                    } // Triangle
                    gilrs::Button::West => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Square as usize);
                    } // Square
                    gilrs::Button::Select => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Select as usize);
                    }
                    gilrs::Button::Start => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Start as usize);
                    }
                    gilrs::Button::DPadUp => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Up as usize);
                    }
                    gilrs::Button::DPadDown => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Down as usize);
                    }
                    gilrs::Button::DPadLeft => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Left as usize);
                    }
                    gilrs::Button::DPadRight => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Right as usize);
                    }
                    gilrs::Button::LeftTrigger => {
                        prev_buttons |= 0x1 << (crate::sio::Button::L1 as usize);
                    }
                    gilrs::Button::RightTrigger => {
                        prev_buttons |= 0x1 << (crate::sio::Button::R1 as usize);
                    }
                    gilrs::Button::Mode => {
                        info!("analog mode changed!");
                        let prev = self.system.sio.gamepad1.get_analog_mode();
                        self.system.sio.gamepad1.set_analog_mode(!prev);
                    }
                    _ => {} // ignore..
                },
                gilrs::EventType::AxisChanged(axis, value, _) => {
                    let axes = self.system.sio.gamepad1.get_stick_axis();
                    let byte = value.mul_add(127.5, 127.5) as u8;
                    if axis == Axis::LeftStickX {
                        axes[2] = byte;
                    } else if axis == Axis::LeftStickY {
                        axes[1] = byte;
                    } else if axis == Axis::RightStickX {
                        axes[0] = byte;
                    } else if axis == Axis::RightStickY {
                        axes[3] = byte; // correct
                    }
                    // self.system.sio.gamepad1.set_stick_axis(left, right);
                }
                gilrs::EventType::Connected => {}
                gilrs::EventType::Disconnected => {}
                _ => {} //println!("gamepad evvent ignored {:?} ", event)
            }
        }
        // println!("0x{:X}", prev_buttons);
        self.system.sio.gamepad1.set_buttons(prev_buttons);

        self.system.gilrs = self.gilrs.take();
    }

    pub fn update_negcon(&mut self) {
        let mut prev_buttons = self.system.sio.gamepad1.get_buttons();
        let axes = self.system.sio.gamepad1.get_stick_axis();
        while let Some(Event {
            id: _,
            event,
            time: _,
            ..
        }) = self.gilrs.as_mut().unwrap().next_event()
        {
            // println!("{:?} New event from {}: {:?}", time, id, event);
            match event {
                gilrs::EventType::ButtonPressed(button, _) => match button {
                    gilrs::Button::South => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Cross as usize));
                    } // Cross
                    gilrs::Button::East => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Circle as usize));
                    } // Circle
                    gilrs::Button::North => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Triangle as usize));
                    } // Triangle
                    gilrs::Button::West => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Square as usize));
                    } // Square
                    gilrs::Button::Select => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Select as usize));
                    }
                    gilrs::Button::Start => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Start as usize));
                    }
                    gilrs::Button::DPadUp => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Up as usize));
                    }
                    gilrs::Button::DPadDown => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Down as usize));
                    }
                    gilrs::Button::DPadLeft => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Left as usize));
                    }
                    gilrs::Button::DPadRight => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Right as usize));
                    }
                    gilrs::Button::LeftTrigger => {
                        axes[3] = 0xff;
                        // prev_buttons &= !(0x1 << (crate::sio::Button::L1 as usize));
                    }
                    gilrs::Button::RightTrigger => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::R1 as usize));
                    }
                    _ => {} // ignore..
                },
                gilrs::EventType::ButtonReleased(button, _) => match button {
                    gilrs::Button::South => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Cross as usize);
                    } // Cross
                    gilrs::Button::East => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Circle as usize);
                    } // Circle
                    gilrs::Button::North => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Triangle as usize);
                    } // Triangle
                    gilrs::Button::West => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Square as usize);
                    } // Square
                    gilrs::Button::Select => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Select as usize);
                    }
                    gilrs::Button::Start => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Start as usize);
                    }
                    gilrs::Button::DPadUp => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Up as usize);
                    }
                    gilrs::Button::DPadDown => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Down as usize);
                    }
                    gilrs::Button::DPadLeft => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Left as usize);
                    }
                    gilrs::Button::DPadRight => {
                        prev_buttons |= 0x1 << (crate::sio::Button::Right as usize);
                    }
                    gilrs::Button::LeftTrigger => {
                        axes[3] = 0x00;
                        // prev_buttons |= 0x1 << (crate::sio::Button::L1 as usize);
                    }
                    gilrs::Button::RightTrigger => {
                        prev_buttons |= 0x1 << (crate::sio::Button::R1 as usize);
                    }
                    gilrs::Button::Mode => {
                        info!("analog mode changed!");
                        // let prev = self.system.sio.gamepad1.mode.get();
                        // self.system.sio.gamepad1.set_analog_mode(!prev);
                    }
                    _ => {} // ignore..
                },
                gilrs::EventType::AxisChanged(axis, value, _) => {
                    let byte = value.mul_add(127.5, 127.5) as u8;
                    if axis == Axis::RightZ {
                        axes[2] = byte;
                    } else if axis == Axis::LeftZ {
                        axes[1] = byte;
                    } else if axis == Axis::LeftStickX {
                        axes[0] = byte;
                    } else if axis == Axis::RightStickY {
                        // axes[3] = byte; // correct
                    }
                    // self.system.sio.gamepad1.set_stick_axis(left, right);
                }
                gilrs::EventType::Connected => {}
                gilrs::EventType::Disconnected => {}
                _ => {} //println!("gamepad evvent ignored {:?} ", event)
            }
        }
        // println!("0x{:X}", prev_buttons);
        self.system.sio.gamepad1.set_buttons(prev_buttons);
    }
}

pub struct App {
    state: Option<State>,
    occluded: bool,
    cli: Cli,
}

impl App {
    pub fn new(cli: Cli) -> Self {
        Self {
            state: None,
            occluded: false,
            cli,
        }
    }
}

impl ApplicationHandler<State> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        #[allow(unused_mut)]
        let mut window_attributes = Window::default_attributes();
        // .with_inner_size(LogicalSize::new(1920, 1080))
        // .with_max_inner_size(LogicalSize::new(1920, 1080));

        let window = Arc::new(event_loop.create_window(window_attributes).unwrap());
        window.set_title("rpsx-emu");
        if self.cli.guncon || self.cli.justifier {
            let w = 96;
            let h = 96;
            let rgba = include_bytes!("crosshair.rgba");
            // let rgba = vec![255; (w * h * 4) as usize];
            let source = CustomCursor::from_rgba(rgba, w, h, w / 2, h / 2).unwrap();
            let custom_cursor = event_loop.create_custom_cursor(source);
            window.set_cursor(custom_cursor);
        }

        self.state = Some(pollster::block_on(State::new(window, self.cli.clone())).unwrap());
    }

    #[allow(unused_mut)]
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, mut event: State) {
        println!("user event");
        self.state = Some(event);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        let state = match &mut self.state {
            Some(st) => st,
            None => return,
        };

        match event {
            WindowEvent::CloseRequested => {
                state
                    .audio_stream
                    .pause()
                    .expect("audio stream should pause");
                event_loop.exit();
            }
            WindowEvent::Resized(size) => state.resize(size.width, size.height),
            WindowEvent::RedrawRequested => {
                // let before = web_time::Instant::now();
                // let elapsed = before.duration_since(state.render_finished);
                // const FRAME_TIME: u128 = 16666;
                // if elapsed.as_micros() < FRAME_TIME {
                //     let sl = FRAME_TIME - elapsed.as_micros();
                //     // std::thread::sleep(std::time::Duration::new(0, 1000 * sl as u32));
                // }
                // let after = web_time::Instant::now();
                // state.render_time_ms = after
                //     .checked_duration_since(state.render_finished)
                //     .unwrap()
                //     .subsec_nanos()
                //     / 1000;
                // if state.frame_num % 60 == 1 {
                //     // println!("ms: {}",  (self.render_time_ms) as f32 / 100_000.0);
                //     let msg = format!("fps: {}", 1_000_000.0 / (state.render_time_ms) as f32);
                //     println!("{}", msg);
                // }

                state.update(event_loop);
                if self.occluded {
                    return;
                }
                let window_arc = state.window.clone();

                if let Some(frame) = state.acquire_surface(window_arc) {
                    let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
                        format: Some(state.config.view_formats[0]),
                        ..wgpu::TextureViewDescriptor::default()
                    });
                    state.render(&view);

                    state.window.pre_present_notify();

                    state.queue.present(frame);
                }
                state.render_finished = web_time::Instant::now();
            }
            WindowEvent::MouseInput {
                state: st, button, ..
            } => state.handle_mouse_button(button, st.is_pressed()),
            WindowEvent::CursorMoved { position, .. } => {
                state.handle_mouse_moved(position.x, position.y);
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state: key_state,
                        logical_key,
                        repeat,
                        ..
                    },
                ..
            } => state.handle_key(event_loop, code, key_state, logical_key, repeat),
            _ => {}
        }
    }
}

pub fn run(cli: Cli) -> anyhow::Result<()> {
    env_logger::Builder::new()
        .filter_level(LevelFilter::Warn)
        // .filter_module("rpsx_emu::cdrom", LevelFilter::Info)
        .init();
    // env_logger::init();

    let event_loop = EventLoop::with_user_event().build()?;
    let mut app = App::new(cli);
    event_loop.run_app(&mut app)?;
    // app.state.unwrap().writer.flush()?;
    // println!("why u not exiting??");
    // drop(app.state.unwrap().audio_sender);
    // drop(app);

    Ok(())
}

pub fn set_uniforms(state: &State, queue: &wgpu::Queue) {
    let is_24bpp = if state.display_depth == DisplayDepth::D24Bits {
        1
    } else {
        0
    };
    queue.write_buffer(
        &state.uniforms_buffer,
        0,
        bytemuck::cast_slice(&[DisplayUniforms { is_24bpp }]),
    );
}
// Set the camera matrix that maps from world coordinates to Normalized Device Coordinates.
// pub fn set_camera(state: &State, queue: &wgpu::Queue, camera: Mat4<f32>) {
//     queue.write_buffer(
//         &state.camera_buffer,
//         0,
//         bytemuck::cast_slice(&camera.into_col_arrays()),
//     );
// }
// pub fn render<'a: 'pass, 'pass>(
//     state: &State,
//     _device: &wgpu::Device,
//     rpass: &mut wgpu::RenderPass<'pass>,
// ) {
//     rpass.set_pipeline(&state.pipeline);
//     rpass.set_vertex_buffer(0, state.vertex_buffer.slice(..));
//     rpass.set_bind_group(0, &state.uniforms_bind_group, &[]);
//
//     rpass.set_bind_group(1, &state.texture_bind_group, &[]);
//     rpass.draw(0..6, 0..1);
// }
