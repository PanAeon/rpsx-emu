use cgmath::prelude::*;

use cpal::traits::StreamTrait;
use env_logger::init;
// use env_logger::fmt::style::Color;
use std::cmp::min;
use std::fs::File;
use std::io::{BufWriter, Cursor, Read, Write};
use std::sync::Mutex;
use std::{iter, sync::Arc};
use wgpu::util::DeviceExt;
use winit::dpi::LogicalSize;
use winit::event_loop::EventLoopProxy;
use std::path::Path;
use winit::keyboard::Key;
use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::Window,
};

use gilrs::{Button, Event, GamepadId, Gilrs};

use std::{borrow::Cow, collections::HashMap, hash::Hash, num::NonZeroU64};
use vek::{Mat4, Vec2, Vec4};

use crate::cdrom::CDRom;
use crate::sio::Sio;
use crate::spu::Spu;

mod bios;
mod cpu;
mod dma;
mod gpu;
mod memory_bus;
mod ram;
mod spu;
mod scratchpad;
mod audio;
mod irq;
mod timers;
mod scheduler;
mod cdrom;
mod gte;
mod sio;
mod mdec;
mod renderer;

mod resources;
mod cdxa;

fn main() {
    run().unwrap();
}

#[rustfmt::skip]
pub const OPENGL_TO_WGPU_MATRIX: cgmath::Matrix4<f32> = cgmath::Matrix4::from_cols(
    cgmath::Vector4::new(1.0, 0.0, 0.0, 0.0),
    cgmath::Vector4::new(0.0, 1.0, 0.0, 0.0),
    cgmath::Vector4::new(0.0, 0.0, 0.5, 0.0),
    cgmath::Vector4::new(0.0, 0.0, 0.5, 1.0),
);

const fn mat4_const_from_rows(m: [[f32; 4]; 4]) -> Mat4<f32> {
    Mat4 {
        cols: Vec4 {
            x: Vec4::new(m[0][0], m[1][0], m[2][0], m[3][0]),
            y: Vec4::new(m[0][1], m[1][1], m[2][1], m[3][1]),
            z: Vec4::new(m[0][2], m[1][2], m[2][2], m[3][2]),
            w: Vec4::new(m[0][3], m[1][3], m[2][3], m[3][3]),
        },
    }
}

#[rustfmt::skip]
pub const FULLSCREEN_QUAD_CAMERA: Mat4<f32> = mat4_const_from_rows([
    [2.0, 0.0, 0.0, -1.0],
    [0.0, 2.0, 0.0, -1.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
]);

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
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
                }
            ]
        }
    }
}

const VERTICES: &[Vertex] = &[
    Vertex { position: [0.0, 0.0, 0.0], uv: [0.0, 0.0] },
    Vertex { position: [1.0, 0.0, 0.0], uv: [1.0, 0.0] },
    Vertex { position: [0.0, 1.0, 0.0], uv: [0.0, 1.0] },
    Vertex { position: [0.0, 1.0, 0.0], uv: [0.0, 1.0] },
    Vertex { position: [1.0, 0.0, 0.0], uv: [1.0, 0.0] },
    Vertex { position: [1.0, 1.0, 0.0], uv: [1.0, 1.0] },
];




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
    a: u8
}

// #[derive(Clone)]
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
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    // camera_controller: CameraController,
    // tilemap: TilemapData<'static>,
    // sprites: TilemapData<'static>,
    vertex_buffer: wgpu::Buffer,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    texture_params_buffer: wgpu::Buffer,
    texture: wgpu::Texture,
    texture_bind_group: wgpu::BindGroup,
    // dimensions: (u32, u32),
    framebuffer: Arc<Mutex<Vec<Color>>>,
    // image_rgba: image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    cpu: cpu::Cpu,
    texture_size: wgpu::Extent3d,
    audio_sender: crossbeam::channel::Sender<[i16; 2]>,
    audio_stream: cpal::Stream,
    paused: bool,
    gilrs: Gilrs,
    active_gamepad: Option<GamepadId>,
    output_width: usize,
    output_height: usize,
    display_vram: bool
}

impl State {
    async fn new(window: Arc<Window>) -> anyhow::Result<State> {
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

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await?;

        let limits = wgpu::Limits::default().using_resolution(adapter.limits());
        // limits.max_texture_dimension_2d *= 2;
        // limits.max_dynamic_storage_buffers_per_pipeline_layout = 8;
        // limits.max_storage_textures_per_shader_stage = 8;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu::Features::empty(),
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
        // let img_data = include_bytes!("happy-tree.png");
        // use image::ImageReader;
        // let image = ImageReader::new(Cursor::new(img_data))
        //     .with_guessed_format()
        //     .unwrap()
        //     .decode()
        //     .unwrap();
        // let image_rgba = image.to_rgba8();
        // use image::GenericImageView;
        // let dimensions = image.dimensions();

        let texture_size = wgpu::Extent3d {
            width: 1024,
            height: 512,
            // All textures are stored as 3D, we represent our 2D texture
            // by setting depth to 1.
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            size: texture_size,
            mip_level_count: 1, // We'll talk about this a little later
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Most images are stored using sRGB, so we need to reflect that here.
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            // TEXTURE_BINDING tells wgpu that we want to use this texture in shaders
            // COPY_DST means that we want to copy data to this texture
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            label: Some("diffuse_texture"),
            // This is the same as with the SurfaceConfig. It
            // specifies what texture formats can be used to
            // create TextureViews for this texture. The base
            // texture format (Rgba8UnormSrgb in this case) is
            // always supported. Note that using a different
            // texture format is not supported on the WebGL2
            // backend.
            view_formats: &[],
        });

        let depth_stencil: Option<wgpu::DepthStencilState> = None;

        let shader_source = Cow::Borrowed(include_str!("shader.wgsl"));
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shaders"),
            source: wgpu::ShaderSource::Wgsl(shader_source),
        });
        let camera_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("camera_bind_group_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(
                            ::std::mem::size_of::<[[f32; 4]; 4]>() as u64
                        ),
                    },
                    count: None,
                }],
            });
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tilemap_camera_buffer"),
            size: ::std::mem::size_of::<[[f32; 4]; 4]>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera_bind_group"),
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
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
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("texture_bind_group_layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(
                                ::std::mem::size_of::<TextureBuffer>() as u64,
                            ),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline_layout"),
            bind_group_layouts: &[
                Some(&camera_bind_group_layout),
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
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
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
        let texture_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("texture_params_buffer"),
            size: ::std::mem::size_of::<TextureBuffer>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("texture bind_group"),
            layout: &texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: texture_params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&diffuse_sampler),
                },
            ],
        });

        // let sprites_view =
        //     tilemap_sprites_texture.create_view(&wgpu::TextureViewDescriptor::default());
        // let index_view = tilemap_index_texture.create_view(&wgpu::TextureViewDescriptor::default());
        // let tilemap_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        //     label: Some("tilemap_bind_group"),
        //     layout: &tilemap_bind_group_layout,
        //     entries: &[
        //         wgpu::BindGroupEntry {
        //             binding: 0,
        //             resource: tilemap_params_buffer.as_entire_binding(),
        //         },
        //         wgpu::BindGroupEntry {
        //             binding: 1,
        //             resource: wgpu::BindingResource::TextureView(&index_view),
        //         },
        //         wgpu::BindGroupEntry {
        //             binding: 2,
        //             resource: wgpu::BindingResource::TextureView(&sprites_view),
        //         },
        //     ],
        // });

    let bios = bios::Bios::new(Path::new("/foo/SCPH1001.BIN"))?;
    // let bios = bios::Bios::new(Path::new("/foo/openbios.bin"))?;
    let ram = ram::Ram::new();
    let scratchpad = scratchpad::Scratchpad::new();
    let dma = dma::Dma::new();
    // let gpu = gpu::Gpu::new();
    let spu = spu::Spu::new();
    let cdrom = CDRom::default();
    // let spu = spu::Spu::default();
    let irqctl = irq::InterruptController::default();
    let sio = sio::Sio::new();
    let mdec = mdec::Mdec::new();
//
//     // let bytes = fs::read("/foo/SCPH1001.BIN")?;
//     for i in (0..40).step_by(4) {
//         print!("0x{:02X}", bios.data[i+3]);
//         print!("{:02X}", bios.data[i+2]);
//         print!("{:02X}", bios.data[i+1]);
//         print!("{:02X}", bios.data[i+0]);
//         println!();
//     }
//
    let mut scheduler = scheduler::Scheduler::default();
    scheduler.init();
    let timers = timers::Timers::new();
    let (sender, receiver, handle) = renderer::Renderer::create();
    let gpu = gpu::Gpu::new(sender, receiver, handle);
    let memory_bus = memory_bus::MemoryBus::new(bios, ram, scratchpad, dma, spu, irqctl, scheduler, timers, cdrom, sio, mdec,
        gpu);
    let cpu = cpu::Cpu::new(memory_bus);


    let core_ids = core_affinity::get_core_ids().unwrap();
    let res = core_affinity::set_for_current(core_ids[0]);
    if res {
        println!("main thread pinned to 0"); 
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
            // camera_uniform,
            camera_buffer,
            camera_bind_group,
            // camera_controller,
            render_finished: web_time::Instant::now(),
            render_time_ms: 0,
            frame_num: 0,
            // tilemap,
            vertex_buffer,
            pipeline,
            texture_bind_group_layout,
            texture_params_buffer,
            // tileset_params_buffer,
            // tileset_texture,
            // tileset_bind_group,
            // tilemap_params_buffer,
            // tilemap_index_texture,
            texture_bind_group,
            texture,
            framebuffer: Arc::new(Mutex::new(vec![Color {r:0, g:0 ,b:0, a: 255}; 1024 * 512])),
            texture_size,
            // image_rgba,
            // dimensions, 
            // game_state,
                        // sprites,
                        // normal_font,
                        // normal_blue_font,
                        // small_font,
                        // sounds
            cpu,
            audio_stream,
            audio_sender,
            paused: false,
            gilrs: Gilrs::new().unwrap(),
            active_gamepad: None,
            output_width: 0,
            output_height: 0,
            display_vram: false,
            // writer,
        };

        for (id, gamepad) in state.gilrs.gamepads() {
            println!("{} is {:?}", gamepad.name(), gamepad.power_info());
            if gamepad.name().eq("Microsoft Xbox Controller") {
                state.active_gamepad = Some(id)
            }
        }

        set_camera(&state, &state.queue, FULLSCREEN_QUAD_CAMERA);
        State::upload_framebuffer(&state);
        // for _ in 0..120*735 {
        //     state.audio_sender.send([0i16, 0i16]).expect("can't send audio sample");
        // }
        State::sideload_exe(&mut state);
        state.audio_stream.play()?;
        //

        // state.queue.write_texture(
        //     // Tells wgpu where to copy the pixel data
        //     wgpu::TexelCopyTextureInfo {
        //         texture: &state.texture,
        //         mip_level: 0,
        //         origin: wgpu::Origin3d::ZERO,
        //         aspect: wgpu::TextureAspect::All,
        //     },
        //     // The actual pixel data
        //     bytemuck::cast_slice(&state.framebuffer),
        //     // The layout of the texture
        //     wgpu::TexelCopyBufferLayout {
        //         offset: 0,
        //         bytes_per_row: Some(4 * 1024),
        //         rows_per_image: Some(512),
        //     },
        //     texture_size,
        // );
        // let tileset =
        //     TilesetData::from_image_with_spacing(&image, Vec2::broadcast(16), Vec2::broadcast(0));
        // let fontset = TilesetData::from_image_with_spacing(
        //     &font_image,
        //     Vec2::broadcast(16),
        //     Vec2::broadcast(0),
        // );
        // upload_tileset(&state, &state.device, &state.queue, &tileset);
        // upload_fonts(&state, &state.device, &state.queue, &fontset);
        // upload_tilemap(
        //     &state,
        //     &state.device,
        //     &state.queue,
        //     &TilemapDrawData {
        //         transform: Mat4::identity(),
        //         tilemap: Cow::Borrowed(&state.tilemap),
        //         tileset: 0,
        //     },
        // );
        Ok(state)
    }

    pub fn upload_framebuffer(&self) {
        self.queue.write_texture(
            // Tells wgpu where to copy the pixel data
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
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

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            let surface = self.surface.as_ref().unwrap();
            surface.configure(&self.device, &self.config);
            self.is_surface_configured = true;
            self.update_vertex_buffer_if_needed(self.output_width, self.output_height, true);
            // self.depth_texture =
            //     texture::Texture::create_depth_texture(&self.device, &self.config, "depth_texture");
        }
    }

    fn sideload_exe(&mut self) {
        let filename = "/foo/psxtest_cpu.exe";
        // let filename = "/foo/psxtest_gte.exe";
        // let filename = "/foo/psxtest_gpu.exe";
        // let filename = "/foo/psx/PSX/CPUTest/CPU/LOADSTORE/LB/CPULB.exe";
        // let filename = "/foo/psx/PSX/GPU/16BPP/MemoryTransfer/MemoryTransfer16BPP.exe";
        // let filename = "/foo/psx/PSX/Cube/Cube.exe";
        // let filename = "/foo/psx/PSX/GPU/16BPP/RenderTextureRectangle/CLUT4BPP/RenderTextureRectangleCLUT4BPP.exe";
        // let filename = "/foo/psx/PSX/GPU/16BPP/RenderTextureRectangle/CLUT8BPP/RenderTextureRectangleCLUT8BPP.exe";
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
        while self.cpu.pc != 0x8003_0000 {
            self.cpu.run_next_instruction();
            self.cpu.check_for_tty_output();
        }

        // exe header
        let initial_pc   = u32::from_le_bytes(data[0x10..0x14].try_into().unwrap());
        let initial_r28  = u32::from_le_bytes(data[0x14..0x18].try_into().unwrap());
        let exe_ram_addr = u32::from_le_bytes(data[0x18..0x1C].try_into().unwrap()) & 0x001F_FFFF;
        let exe_size= u32::from_le_bytes(data[0x1C..0x20].try_into().unwrap()) as usize;
        let initial_sp   = u32::from_le_bytes(data[0x30..0x34].try_into().unwrap());

        // exe_ram_addr = crate::memory_bus::mask_region(exe_ram_addr);
        println!("exe ram addr: 0x{:X}", exe_ram_addr);
        println!("initial pc: 0x{:X}", initial_pc);

        // let exe_size = (file_size - 2048) as u32;
        // let exe_size = (exe_size_2kb);
        // let exe_size = 1013760 - 2048;
        println!("exe size: {}", exe_size);
        self.cpu.memory_bus.ram.data[exe_ram_addr as usize .. (exe_ram_addr  as usize + exe_size)]
            .copy_from_slice(&data[2048..2048 + exe_size as usize]);
        //  let dest = self
        //     .cpu.memory_bus.ram.data
        //     .bytes()
        //     .get_mut(exe_ram_addr as usize..exe_ram_addr as usize + exe_size)
        //     .context("EXE load address out of RAM bounds")?;
        //
        // let src = data
        //     .get(2048..2048 + exe_size)
        //     .context("EXE file truncated")?;
        //
        // dest.copy_from_slice(src);

        self.cpu.set_reg(28, initial_r28);
        if initial_sp != 0 {
            self.cpu.set_reg(29, initial_sp);
            self.cpu.set_reg(30, initial_sp);
        }
        self.cpu.pc = initial_pc;
        self.cpu.next_pc = initial_pc + 4;
    }

    fn update_vertex_buffer_if_needed(&mut self, width: usize, height: usize, force: bool) {
        if width == 0 && height == 0 {
            self.output_width = width;
            self.output_height = height;
            return;
        }
        if !force && width == self.output_width && height == self.output_height {
            return;
        }

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

        let u = width as f32 / 1024.0;
        let v = height as f32 / 512.0;


            // self.config.width = width;
            // self.config.height = height;
        let vertices: &[Vertex] = &[
            Vertex { position: [bx, by, 0.0], uv: [0.0, v] },
            Vertex { position: [bx + w, by, 0.0], uv: [u, v] },
            Vertex { position: [bx, by + h, 0.0], uv: [0.0, 0.0] },

            Vertex { position: [bx, by + h, 0.0], uv: [0.0, 0.0] },
            Vertex { position: [bx + w, by, 0.0], uv: [u, v] },
            Vertex { position: [bx + w, by + h, 0.0], uv: [u, 0.0] },
            // Vertex { position: [0.0, 0.0, 0.0], uv: [0.0, 0.0] },
            // Vertex { position: [w, 0.0, 0.0], uv: [u, 0.0] },
            // Vertex { position: [0.0, 1.0, 0.0], uv: [0.0, v] },
            // Vertex { position: [0.0, 1.0, 0.0], uv: [0.0, v] },
            // Vertex { position: [w, 0.0, 0.0], uv: [u, 0.0] },
            // Vertex { position: [w, 1.0, 0.0], uv: [u, v] },
        ];
        self.queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(vertices));
        self.output_width = width;
        self.output_height = height;
    }

    fn update(&mut self, event_loop: &ActiveEventLoop) {
        self.update_gamepad();
        if self.paused {
            println!("current pc: 0x{:X}", self.cpu.pc);
            // self.cpu.memory_bus.irqctl.status.set_sio(true);
            // self.cpu.memory_bus.irqctl.status.set_ctl_mem(true);
            // self.cpu.pc = self.cpu.pc + 4;
            // self.cpu.next_pc = self.cpu.pc + 8;
            self.paused = false;
            return;
        }
        loop {
            if let Some(event) = self.cpu.memory_bus.scheduler.get_next_event() {
                match event {
                    scheduler::Event::SpuTick => {
                        Spu::clock(&mut self.cpu.memory_bus);
                         // self.cpu.memory_bus.spu.clock();
                         // let spu = &mut self.cpu.memory_bus.spu;
                        let cdrom = &mut self.cpu.memory_bus.cdrom;
                         let sample = self.cpu.memory_bus.spu.mix(cdrom);
                         self.audio_sender.send(sample).expect("can't send audio sample");

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
                        // self.cpu.memory_bus.spu.clock();
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
                        // self.cpu.memory_bus.gpu_sender.send(gpu::GpuMsg::ProduceFB(self.framebuffer.clone(), self.display_vram)).expect("ok");
                        let (w, h) = self.cpu.memory_bus.gpu.render_fb(self.framebuffer.clone(), self.display_vram);
                        self.update_vertex_buffer_if_needed(w, h, false);
                        // if self.cpu.memory_bus.gpu.interrupt == false {
                            self.cpu.memory_bus.irqctl.status.set_vblank(true);
                        // }
                        timers::Timers::enter_vsync(&mut self.cpu.memory_bus);
                        // self.cpu.memory_bus.gpu_sender.send(gpu::GpuMsg::EnterVSync).expect("ok");
                        self.cpu.memory_bus.gpu.enter_vsync();
                    },
                    scheduler::Event::VBlankEnd => {
                        // self.cpu.memory_bus.gpu_sender.send(gpu::GpuMsg::ExitVSync).expect("ok");
                        self.cpu.memory_bus.gpu.exit_vsync();
                        timers::Timers::exit_vsync(&mut self.cpu.memory_bus);
                        // let (w, h) = self.cpu.memory_bus.gpu_ctrl_receiver.recv().expect("ok");
                        // self.update_vertex_buffer_if_needed(w, h, false);
                        break;
                    },
                    scheduler::Event::HBlankStart => {
                        // self.cpu.memory_bus.gpu_sender.send(gpu::GpuMsg::EnterHSync).expect("ok");
                        self.cpu.memory_bus.gpu.enter_hsync();
                        timers::Timers::enter_hsync(&mut self.cpu.memory_bus);
                    },
                    scheduler::Event::HBlankEnd => {
                        // self.cpu.memory_bus.gpu_sender.send(gpu::GpuMsg::ExitHSync).expect("ok");
                        self.cpu.memory_bus.gpu.exit_hsync();
                        timers::Timers::exit_hsync(&mut self.cpu.memory_bus);
                    },
                    scheduler::Event::CDRomResultIrq(resp) => {
                        cdrom::CDRom::process_response(&mut self.cpu.memory_bus, resp);
                    },
                    scheduler::Event::Timer(i) => timers::Timers::process_interrupt(&mut self.cpu.memory_bus, i),
                    scheduler::Event::SerialSend => Sio::process_serial_send(&mut self.cpu.memory_bus),
                    scheduler::Event::DsrOff     => self.cpu.memory_bus.sio.turn_dsr_off(),
                }
            }
            for _ in 0..20 {
                self.cpu.run_next_instruction();
                self.cpu.check_for_tty_output();
            }
            self.cpu.memory_bus.scheduler.advance(40); // 40???
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
        //                  self.cpu.memory_bus.irqctl.status.set_vblank(true);
        //     }
        //     self.cpu.memory_bus.spu.clock();
        //     audio_buffer.push(self.cpu.memory_bus.spu.mix());
        //     // self.audio_sender.send(self.cpu.memory_bus.spu.mix()).expect("can't send audio");
        // }


    }

    fn render(&mut self, view: &wgpu::TextureView) {
        self.upload_framebuffer();
        self.window.request_redraw();

        // let output = self.surface.as_ref().unwrap().get_current_texture();
        // let view = output
        //     .texture
        //     .create_view(&wgpu::TextureViewDescriptor::default());

        // let mut encoder = self
        //     .device
        //     .create_command_encoder(&wgpu::CommandEncoderDescriptor {
        //         label: Some("Render Encoder"),
        //     });

        {
            // let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            //     label: Some("Render Pass"),
            //     color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            //         view: &view,
            //         resolve_target: None,
            //         ops: wgpu::Operations {
            //             load: wgpu::LoadOp::Clear(self.clear_color),
            //             store: wgpu::StoreOp::Store,
            //         },
            //         depth_slice: None,
            //     })],
            //     depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            //         view: &self.depth_texture.view,
            //         depth_ops: Some(wgpu::Operations {
            //             load: wgpu::LoadOp::Clear(1.0),
            //             store: wgpu::StoreOp::Store,
            //         }),
            //         stencil_ops: None,
            //     }),
            //     occlusion_query_set: None,
            //     timestamp_writes: None,
            //     multiview_mask: None,
            // });
            //     render_pass.set_pipeline(&self.render_pipeline); // 2.
            //     render_pass.set_bind_group(0, &self.diffuse_bind_group, &[]);
            //     // render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            //     // render_pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
            //     render_pass.set_bind_group(1, &self.camera_bind_group, &[]);
            //     // render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            //     // render_pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
            //
            //     render_pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
            //     render_pass.set_pipeline(&self.render_pipeline);
            //     render_pass.set_bind_group(0, &self.diffuse_bind_group, &[]);
            //     render_pass.set_bind_group(1, &self.camera_bind_group, &[]);
            //
            //     use model::DrawModel;
            //     render_pass
            //         .draw_mesh_instanced(&self.obj_model.meshes[0], 0..self.instances.len() as u32);
            // }
            //
            // self.queue.submit(iter::once(encoder.finish()));

            // tilemap stuff
            // {
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
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    ..Default::default()
                });
                render(self, &self.device, &mut rpass);
            }
            self.queue.submit(vec![encoder.finish()]);
        }
        // tilemap stuff end
        // output.present();
        // TODO: must move up..
        // self.frame_num += 1;

        // Ok(())
    }

    fn handle_mouse_moved(&mut self, x: f64, y: f64) {
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
        event_loop: &ActiveEventLoop,
        code: KeyCode,
        key_state: ElementState,
        logical_key: Key,
        repeat: bool,
    ) {
        match (code, key_state.is_pressed()) {
            (KeyCode::Space, false) => self.paused = !self.paused,
            (KeyCode::KeyV, false) => self.display_vram = !self.display_vram,
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
                match (logical_key.to_text()) {
                    Some(str) => {
                        // self.game_state.last_input = Some((String::from(str), repeat));
                    }
                    _ => {}
                }
                // self.camera_controller.handle_key(code, is_pressed);
            }
        }
    }
    pub fn update_gamepad(&mut self) {
        let mut prev_buttons = self.cpu.memory_bus.sio.gamepad1.digital_switches;
        while let Some(Event { id, event, time, .. }) = self.gilrs.next_event() {
            // println!("{:?} New event from {}: {:?}", time, id, event);
            match event {
                gilrs::EventType::ButtonPressed(button, _) => match button {
                    gilrs::Button::South => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Cross as usize));
                    }, // Cross
                    gilrs::Button::East => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Circle as usize));
                    }, // Circle
                    gilrs::Button::North => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Triangle as usize));
                    }, // Triangle
                    gilrs::Button::West => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Square as usize));
                    }, // Square
                    gilrs::Button::Select => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Select as usize));
                    },
                    gilrs::Button::Start => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Start as usize));
                    },
                    gilrs::Button::DPadUp => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Up as usize));
                    },
                    gilrs::Button::DPadDown => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Down as usize));
                    },
                    gilrs::Button::DPadLeft => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Left as usize));
                    },
                    gilrs::Button::DPadRight => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::Right as usize));
                    },
                    gilrs::Button::LeftTrigger => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::L1 as usize));
                    }
                    gilrs::Button::RightTrigger => {
                        prev_buttons &= !(0x1 << (crate::sio::Button::R1 as usize));
                    }
                    _ => {}, // ignore..
                },
                gilrs::EventType::ButtonReleased(button, _) => match button {
                    gilrs::Button::South => {
                        prev_buttons |= (0x1 << (crate::sio::Button::Cross as usize));
                    }, // Cross
                    gilrs::Button::East => {
                        prev_buttons |= (0x1 << (crate::sio::Button::Circle as usize));
                    }, // Circle
                    gilrs::Button::North => {
                        prev_buttons |= (0x1 << (crate::sio::Button::Triangle as usize));
                    }, // Triangle
                    gilrs::Button::West => {
                        prev_buttons |= (0x1 << (crate::sio::Button::Square as usize));
                    }, // Square
                    gilrs::Button::Select => {
                        prev_buttons |= (0x1 << (crate::sio::Button::Select as usize));
                    },
                    gilrs::Button::Start => {
                        prev_buttons |= (0x1 << (crate::sio::Button::Start as usize));
                    },
                    gilrs::Button::DPadUp => {
                        prev_buttons |= (0x1 << (crate::sio::Button::Up as usize));
                    },
                    gilrs::Button::DPadDown => {
                        prev_buttons |= (0x1 << (crate::sio::Button::Down as usize));
                    },
                    gilrs::Button::DPadLeft => {
                        prev_buttons |= (0x1 << (crate::sio::Button::Left as usize));
                    },
                    gilrs::Button::DPadRight => {
                        prev_buttons |= (0x1 << (crate::sio::Button::Right as usize));
                    },
                    gilrs::Button::LeftTrigger => {
                        prev_buttons |= 0x1 << (crate::sio::Button::L1 as usize);
                    }
                    gilrs::Button::RightTrigger => {
                        prev_buttons |= 0x1 << (crate::sio::Button::R1 as usize);
                    }
                    _ => {}, // ignore..
                },
                gilrs::EventType::AxisChanged(axis, value, _) => {},
                gilrs::EventType::Connected => {},
                gilrs::EventType::Disconnected => {},
                _ => {}//println!("gamepad evvent ignored {:?} ", event)
            }
        }
        // println!("0x{:X}", prev_buttons);
        self.cpu.memory_bus.sio.gamepad1.set_buttons(prev_buttons);
    }
}


pub struct App {
    state: Option<State>,
    occluded: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            state: None,
            occluded: false,
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
        self.state = Some(pollster::block_on(State::new(window)).unwrap());
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
                state.audio_stream.pause().expect("audio stream should pause");
                event_loop.exit();
            },
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
            WindowEvent::MouseInput { state, button, .. } => match (button, state.is_pressed()) {
                (MouseButton::Left, true) => {}
                (MouseButton::Left, false) => {}
                _ => {}
            },
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

pub fn run() -> anyhow::Result<()> {
    env_logger::init();

    let event_loop = EventLoop::with_user_event().build()?;
    let mut app = App::new();
    event_loop.run_app(&mut app)?;
    // app.state.unwrap().writer.flush()?;
    // println!("why u not exiting??");
    // drop(app.state.unwrap().audio_sender);
    // drop(app);

    Ok(())
}




/// Set the camera matrix that maps from world coordinates to Normalized Device Coordinates.
pub fn set_camera(state: &State, queue: &wgpu::Queue, camera: Mat4<f32>) {
    queue.write_buffer(
        &state.camera_buffer,
        0,
        bytemuck::cast_slice(&camera.into_col_arrays()),
    );
}
/// Render the tilemaps to the provided renderpass, whose color attachment must match the
/// texture format provided when this was created.
pub fn render<'a: 'pass, 'pass>(
    state: &State,
    device: &wgpu::Device,
    rpass: &mut wgpu::RenderPass<'pass>,
) {
    rpass.set_pipeline(&state.pipeline);
    rpass.set_vertex_buffer(0, state.vertex_buffer.slice(..));
    rpass.set_bind_group(0, &state.camera_bind_group, &[]);

    rpass.set_bind_group(1, &state.texture_bind_group, &[]);
    rpass.draw(0..6, 0..1);
}
