use cgmath::prelude::*;

use cpal::traits::StreamTrait;
use env_logger::init;
// use env_logger::fmt::style::Color;
use std::cmp::min;
use std::fs::File;
use std::io::{BufWriter, Cursor, Read, Write};
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

use std::{borrow::Cow, collections::HashMap, hash::Hash, num::NonZeroU64};
use vek::{Mat4, Vec2, Vec4};

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

mod resources;

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

struct Camera {
    // target: cgmath::Point3<f32>,
    x: f32,
    y: f32,
}

impl Camera {
    fn build_view_projection_matrix(&self) -> cgmath::Matrix4<f32> {
        return OPENGL_TO_WGPU_MATRIX;
    }
}

pub const VERTEX_LAYOUT: wgpu::VertexBufferLayout = wgpu::VertexBufferLayout {
    array_stride: 0,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &[],
};

// #[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
// #[repr(C)]
// pub struct TilesetBuffer {
//     width: u32,
//     height: u32,
//     tile_width: u32,
//     tile_height: u32,
// }
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
    framebuffer: Vec<Color>,
    // image_rgba: image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    cpu: cpu::Cpu,
    texture_size: wgpu::Extent3d,
    audio_sender: crossbeam::channel::Sender<[i16; 2]>,
    audio_stream: cpal::Stream,
    audio_buffer: Vec<[i16;2]>,
    // writer: BufWriter<File>
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
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vertex_buffer"),
            size: 1,
            usage: wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });
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
                buffers: &[Some(VERTEX_LAYOUT.clone())],
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
    let ram = ram::Ram::new();
    let scratchpad = scratchpad::Scratchpad::new();
    let dma = dma::Dma::new();
    let gpu = gpu::Gpu::new();
    let spu = spu::Spu::new();
    // let spu = spu::Spu::default();
    let irqctl = irq::InterruptController::default();
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
    let memory_bus = memory_bus::MemoryBus::new(bios, ram, scratchpad, dma, gpu, spu, irqctl, scheduler, timers);
    let cpu = cpu::Cpu::new(memory_bus);

    let (audio_stream, audio_sender) = crate::audio::build_audio_stream()?;
         let file = File::create("output.pcm")?;
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
            framebuffer: vec![Color {r:0, g:0 ,b:0, a: 255}; 1024 * 512],
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
            audio_buffer: Vec::with_capacity(735),
            // writer,
        };
        set_camera(&state, &state.queue, FULLSCREEN_QUAD_CAMERA);
        State::upload_framebuffer(&state);
        // for _ in 0..120*735 {
        //     state.audio_sender.send([0i16, 0i16]).expect("can't send audio sample");
        // }
        state.audio_stream.play()?;
        // State::sideload_exe(&mut state);
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
            bytemuck::cast_slice(&self.framebuffer),
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
            // self.depth_texture =
            //     texture::Texture::create_depth_texture(&self.device, &self.config, "depth_texture");
        }
    }

    fn sideload_exe(&mut self) {
        let mut file = match std::fs::File::open("/foo/psxtest_cpu.exe") {
            Ok(file) => file,
            Err(e) => panic!("Can't load exe {}", e),
        };
        let mut data: Vec<u8> = Vec::new();
        let file_size = match file.read_to_end(&mut data) {
            Ok(x) => x,
            Err(e) => panic!("Can't read exe {}", e),
        };
        while self.cpu.pc != 0x80030000 {
            self.cpu.run_next_instruction();
            self.cpu.check_for_tty_output();
        }

        // exe header
        let initial_pc   = u32::from_le_bytes(data[0x10..0x14].try_into().unwrap());
        let initial_r28  = u32::from_le_bytes(data[0x14..0x18].try_into().unwrap());
        let exe_ram_addr = u32::from_le_bytes(data[0x18..0x1C].try_into().unwrap()) & 0x1FFFFF;
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

    fn update(&mut self, event_loop: &ActiveEventLoop) {
        loop {
            if let Some(event) = self.cpu.memory_bus.scheduler.get_next_event() {
                match event {
                    scheduler::Event::SpuTick => {
                         self.cpu.memory_bus.spu.clock();
                         let sample = self.cpu.memory_bus.spu.mix();
                         self.audio_sender.send(sample).expect("can't send audio sample");


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
                        // TODO: produce framebuffer here?
                        self.cpu.memory_bus.gpu.render_vram(&mut self.framebuffer);
                        self.cpu.memory_bus.irqctl.status.set_vblank(true);
                        timers::Timers::enter_vsync(&mut self.cpu.memory_bus);
                        // println!("vsync?");
                    },
                    scheduler::Event::VBlankEnd => {
                        timers::Timers::exit_vsync(&mut self.cpu.memory_bus);
                        break;
                    },
                    scheduler::Event::HBlankStart => {
                        timers::Timers::enter_hsync(&mut self.cpu.memory_bus);
                    },
                    scheduler::Event::HBlankEnd => {
                        timers::Timers::exit_hsync(&mut self.cpu.memory_bus);
                    },
                    scheduler::Event::Timer(i) => timers::Timers::process_interrupt(&mut self.cpu.memory_bus, i),
                }
            }
            for _ in 0..20 {
                self.cpu.run_next_instruction();
                self.cpu.check_for_tty_output();
            }
            self.cpu.memory_bus.scheduler.advance(37); // 40???
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

        // let before = web_time::Instant::now();
        // let elapsed = before.duration_since(self.render_finished);
        // println!("elsaped: {}", self.render_time_ms);
        // game_state::update_game_state(&mut self.game_state,
        //     self.render_time_ms as f64 / 1000.0,
        //     &self.sounds);
        // if !self.sounds.initialized && self.game_state.user_engaged {
        //   let res = self.sounds.engine.initialize_audio_output_device();
        //     // println!(">>>>");
        //   self.sounds.initialized = res.is_ok();
        // }
        // self.game_state.update();
        // self.camera_controller.update_camera(&mut self.camera);
        // let x = self.game_state.camera_pos.0 as usize % 16;
        // let y = self.game_state.camera_pos.1 as usize % 16;
        // set_camera(
        //     self,
        //     &self.queue,
        //     wgpu_tilemap::camera_with_shift(x as f32, y as f32),
        // );

        // self.sprites.clear();
        // wgpu_tilemap::draw_sprites(&self.game_state, &mut self.sprites, &self.normal_blue_font);
        // upload_sprites(
        //     &self,
        //     &self.device,
        //     &self.queue,
        //     &TilemapDrawData {
        //         transform: Mat4::identity(),
        //         tilemap: Cow::Borrowed(&self.sprites),
        //         tileset: 0,
        //     },
        // );
        // if !self.game_state.run {
        //     event_loop.exit();
        // }
        // self.proxy.send_event(self.clone());
        // drop(self.window.clone());
        // self.device.destroy();
        // self.queue.write_buffer(
        //     &self.camera_buffer,
        //     0,
        //     bytemuck::cast_slice(&[self.camera_uniform]),
        // );
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
        self.frame_num += 1;

        // Ok(())
    }

    fn handle_mouse_moved(&mut self, x: f64, y: f64) {
        self.clear_color.r = x / self.config.width as f64;
        self.clear_color.g = y / self.config.height as f64;
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
        let before = web_time::Instant::now();
        let elapsed = before.duration_since(state.render_finished);
        const FRAME_TIME: u128 = 16666;
        if elapsed.as_micros() < FRAME_TIME {
            let sl = FRAME_TIME - elapsed.as_micros();
            // std::thread::sleep(std::time::Duration::new(0, 1000 * sl as u32));
        }
        let after = web_time::Instant::now();
        state.render_time_ms = after
            .checked_duration_since(state.render_finished)
            .unwrap()
            .subsec_nanos()
            / 1000;
        if state.frame_num % 60 == 1 {
            // println!("ms: {}",  (self.render_time_ms) as f32 / 100_000.0);
            let msg = format!("fps: {}", 1_000_000.0 / (state.render_time_ms) as f32);
            println!("{}", msg);
        }

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

struct CameraController {
    speed: f32,
    is_forward_pressed: bool,
    is_backward_pressed: bool,
    is_left_pressed: bool,
    is_right_pressed: bool,
}

impl CameraController {
    fn new(speed: f32) -> Self {
        Self {
            speed,
            is_forward_pressed: false,
            is_backward_pressed: false,
            is_left_pressed: false,
            is_right_pressed: false,
        }
    }

    fn handle_key(&mut self, code: KeyCode, is_pressed: bool) -> bool {
        match code {
            KeyCode::KeyW | KeyCode::ArrowUp => {
                self.is_forward_pressed = is_pressed;
                true
            }
            KeyCode::KeyA | KeyCode::ArrowLeft => {
                self.is_left_pressed = is_pressed;
                true
            }
            KeyCode::KeyS | KeyCode::ArrowDown => {
                self.is_backward_pressed = is_pressed;
                true
            }
            KeyCode::KeyD | KeyCode::ArrowRight => {
                self.is_right_pressed = is_pressed;
                true
            }
            _ => false,
        }
    }
}

struct Instance {
    position: cgmath::Vector3<f32>,
    rotation: cgmath::Quaternion<f32>,
}

// Create a new `TilemapPipeline` capable of rendering to the provided `texture_format`.
// pub fn new(
//     device: &wgpu::Device,
//     texture_format: wgpu::TextureFormat,
//     depth_stencil: Option<wgpu::DepthStencilState>,
// ) -> TilemapPipeline {
// }
/*
/// Upload a list of tilesets to the GPU, replacing the previous set of tilesets, and reusing texture allocations if the sizes are compatible.
pub fn upload_tileset(
    state: &State,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tileset: &TilesetData,
) {
    let params = TilesetBuffer {
        width: 2048,     //tileset.pixel_size.x,
        height: 2048,    //tileset.pixel_size.y,
        tile_width: 16,  //tileset.size_of_tile.x,
        tile_height: 16, //tileset.size_of_tile.y,
    };

    // self.tilesets.allocate_and_upload(
    // (tileset.pixel_size, tileset.size_of_tile),
    // device,
    // queue,
    // |device, (size, tilesize)| {
    // TilemapPipeline::allocate_tilesets(
    //     device,
    //     &self.tileset_bind_group_layout,
    //     size,
    //     tilesize,
    // )
    // },
    // &params,
    // |i, datum| {
    // self.active_tilesets
    //     .push(((tileset.pixel_size, tileset.size_of_tile), i as u32));
    let texture_data = &tileset.data;
    let idl = wgpu::TexelCopyBufferLayout {
        offset: 0,
        bytes_per_row: Some(4 * 2048),
        rows_per_image: Some(2048),
    };
    let extent = wgpu::Extent3d {
        width: 2048,
        height: 2048,
        depth_or_array_layers: 1, //tile_size.x * tile_size.y,
    };
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &state.tileset_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice::<u32, u8>(&texture_data),
        idl,
        extent,
    );
    queue.write_buffer(
        &state.tileset_params_buffer,
        0,
        &bytemuck::bytes_of(&[params])[..],
    );
}

/// Upload a list of tilesets to the GPU, replacing the previous set of tilesets, and reusing texture allocations if the sizes are compatible.
pub fn upload_fonts(
    state: &State,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tileset: &TilesetData,
) {
    let params = TilesetBuffer {
        width: 2048,     //tileset.pixel_size.x,
        height: 2048,    //tileset.pixel_size.y,
        tile_width: 16,  //tileset.size_of_tile.x,
        tile_height: 16, //tileset.size_of_tile.y,
    };

    // self.tilesets.allocate_and_upload(
    // (tileset.pixel_size, tileset.size_of_tile),
    // device,
    // queue,
    // |device, (size, tilesize)| {
    // TilemapPipeline::allocate_tilesets(
    //     device,
    //     &self.tileset_bind_group_layout,
    //     size,
    //     tilesize,
    // )
    // },
    // &params,
    // |i, datum| {
    // self.active_tilesets
    //     .push(((tileset.pixel_size, tileset.size_of_tile), i as u32));
    let texture_data = &tileset.data;
    let idl = wgpu::TexelCopyBufferLayout {
        offset: 0,
        bytes_per_row: Some(4 * 2048),
        rows_per_image: Some(2048),
    };
    let extent = wgpu::Extent3d {
        width: 2048,
        height: 2048,
        depth_or_array_layers: 1, //tile_size.x * tile_size.y,
    };
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &state.fonts_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice::<u32, u8>(&texture_data),
        idl,
        extent,
    );
    queue.write_buffer(
        &state.tileset_params_buffer,
        0,
        &bytemuck::bytes_of(&[params])[..],
    );
}

pub fn upload_tilemap(
    state: &State,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tilemap_draw_data: &TilemapDrawData,
) {
    let TilemapDrawData {
        transform,
        tilemap,
        tileset,
    } = tilemap_draw_data;
    let size = tilemap.tile_size;
    let params = TilemapBuffer {
        transform: transform.into_row_arrays(),
        // transform: transform.into_col_arrays(),
        width: size.x,
        height: size.y,
        _noise_data: Default::default(),
        _pad: Default::default(),
    };
    // state.draw_calls.allocate_and_upload(
    // size,
    // device,
    // queue,
    // |device, size| {
    // TilemapPipeline::allocate_draw_call(
    // device,
    // &state.tilemap_bind_group_layout,
    // size,
    // )
    // },
    // &params,
    // |_, call| {
    let texture_data = &tilemap.data;
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &state.tilemap_index_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice::<u16, u8>(texture_data.as_ref()),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * 64), //(4*tilemap.tile_size.x),//4*128),
            rows_per_image: Some(64),    //(tilemap.tile_size.y)//128),
        },
        wgpu::Extent3d {
            width: 64,  //size.x,
            height: 64, //size.y,
            depth_or_array_layers: 16,
        },
    );
    // state.tilemap_params_buffer = params.;
    queue.write_buffer(
        &state.tilemap_params_buffer,
        0,
        &bytemuck::bytes_of(&[params])[..],
    );
}

pub fn upload_sprites(
    state: &State,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tilemap_draw_data: &TilemapDrawData,
) {
    let TilemapDrawData {
        transform,
        tilemap,
        tileset,
    } = tilemap_draw_data;
    let size = tilemap.tile_size;
    let params = TilemapBuffer {
        transform: transform.into_row_arrays(),
        // transform: transform.into_col_arrays(),
        width: size.x,
        height: size.y,
        _noise_data: Default::default(),
        _pad: Default::default(),
    };
    // state.draw_calls.allocate_and_upload(
    // size,
    // device,
    // queue,
    // |device, size| {
    // TilemapPipeline::allocate_draw_call(
    // device,
    // &state.tilemap_bind_group_layout,
    // size,
    // )
    // },
    // &params,
    // |_, call| {
    let texture_data = &tilemap.data;
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &state.tilemap_sprites_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice::<u16, u8>(texture_data.as_ref()),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * 64), //(4*tilemap.tile_size.x),//4*128),
            rows_per_image: Some(64),    //(tilemap.tile_size.y)//128),
        },
        wgpu::Extent3d {
            width: 64,  //size.x,
            height: 64, //size.y,
            depth_or_array_layers: 16,
        },
    );
    // state.tilemap_params_buffer = params.;
    // queue.write_buffer(&state.tilemap_params_buffer, 0,
    //      &bytemuck::bytes_of(&[params])[..]);
}*/

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
