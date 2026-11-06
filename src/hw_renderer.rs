use std::{
    borrow::Cow,
    cmp::{self, max, min},
    num::NonZeroU64,
    sync::Mutex,
    thread::{self, JoinHandle},
};

use crossbeam::channel::{Receiver, Sender};
use wgpu::util::DeviceExt;
use wgpu_profiler::{GpuProfiler, GpuProfilerSettings, GpuTimerQueryResult};

use crate::{
    gpu::{Colour, DisplayDepth, HorizontalRes, TextureDepth, Vertex, VerticalRes},
    renderer::{Clut, RendererMsg, RendererResponse, RenderingContext, Texture},
};

const VERT_BUFFER_SIZE: usize = 2048;

#[repr(C)]
struct RenderUniforms {
    target_stp: u32, // 0 for the first pass, 1 for the second pass
    force_set_mask_bit: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Vert {
    position: [i16; 2],
    color: [u8; 3],
    texture_depth: u8,
    uv: [u16; 2],
    clut: [u16; 2],
    texpage_base: [u8; 2],
    texture_window_mask: [u8; 2],
    texture_window_offset: [u8; 2],
    flags: u16,
    z: f32
    // _pad: u16, // draw_area_top_left: [u16;2],
    // draw_area_bottom_right: [u16;2],
}
impl Vert {
    fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vert>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Sint16x2,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::size_of::<[i16; 2]>() as wgpu::BufferAddress,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Uint32,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::offset_of!(Vert, uv) as wgpu::BufferAddress,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Uint16x2,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::offset_of!(Vert, flags) as wgpu::BufferAddress,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Uint16,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::offset_of!(Vert, clut) as wgpu::BufferAddress,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Uint16x2,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::offset_of!(Vert, texpage_base) as wgpu::BufferAddress,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Uint8x2,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::offset_of!(Vert, texture_window_mask) as wgpu::BufferAddress,
                    shader_location: 6,
                    format: wgpu::VertexFormat::Uint8x2,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::offset_of!(Vert, texture_window_offset)
                        as wgpu::BufferAddress,
                    shader_location: 7,
                    format: wgpu::VertexFormat::Uint8x2,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::offset_of!(Vert, z)
                        as wgpu::BufferAddress,
                    shader_location: 8,
                    format: wgpu::VertexFormat::Float32,
                },
            ],
        }
    }
}

bitfield::bitfield! {
    #[derive(Copy, Clone)]
    struct Flags(u16);
    _, set_textured: 0;
    _, set_semitrans: 1;
    _, set_blend: 2;
    _, set_dither: 3;
    u8, _, set_transparency: 5,4;
}

fn create_draw_pipeline(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    display_format: wgpu::TextureFormat,
    scale: u32,
) -> (
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
    wgpu::Texture,
    wgpu::Texture,
    wgpu::Buffer,
    wgpu::Buffer,
    wgpu::BindGroup,
    wgpu::TextureView,
) {
    let vram_texture = device.create_texture(&wgpu::TextureDescriptor {
        size: wgpu::Extent3d {
            width: 1024 * scale,
            height: 512 * scale,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1, // We'll talk about this a little later
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Uint,
        // TEXTURE_BINDING tells wgpu that we want to use this texture in shaders
        // COPY_DST means that we want to copy data to this texture
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
        // | wgpu::TextureUsages::RENDER_ATTACHMENT,
        // | wgpu::TextureUsages::TEXTURE_BINDING,
        // | wgpu::TextureUsages::STORAGE_BINDING,
        label: Some("vram texture"),
        view_formats: &[wgpu::TextureFormat::R32Uint],
    });

    let blit_texture = device.create_texture(&wgpu::TextureDescriptor {
        size: wgpu::Extent3d {
            width: 1024 * scale,
            height: 512 * scale,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1, // We'll talk about this a little later
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Uint,
        // TEXTURE_BINDING tells wgpu that we want to use this texture in shaders
        // COPY_DST means that we want to copy data to this texture
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
        // | wgpu::TextureUsages::RENDER_ATTACHMENT,
        // | wgpu::TextureUsages::TEXTURE_BINDING,
        // | wgpu::TextureUsages::STORAGE_BINDING,
        label: Some("vram texture"),
        view_formats: &[wgpu::TextureFormat::R32Uint],
    });

    let vram_texture_view = vram_texture.create_view(&wgpu::TextureViewDescriptor::default());

    // let vram_blit_texture = device.create_texture(&wgpu::TextureDescriptor {
    //     size: wgpu::Extent3d {
    //         width: 512,
    //         height: 512,
    //         depth_or_array_layers: 1,
    //     },
    //     mip_level_count: 1, // We'll talk about this a little later
    //     sample_count: 1,
    //     dimension: wgpu::TextureDimension::D2,
    //     format: wgpu::TextureFormat::R32Uint,
    //     usage:
    //              wgpu::TextureUsages::COPY_DST
    //             | wgpu::TextureUsages::COPY_SRC,
    //     label: Some("vram blit texture"),
    //     view_formats: &[wgpu::TextureFormat::R32Uint],
    // });

    let shader_source = Cow::Borrowed(include_str!("render.wgsl"));
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("shaders"),
        source: wgpu::ShaderSource::Wgsl(shader_source),
    });

    let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vertex_buffer"),
        size: (VERT_BUFFER_SIZE * size_of::<Vert>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let texture_bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::ReadWrite,
                        format: wgpu::TextureFormat::R32Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    // sample_type: wgpu::TextureSampleType::Uint,
                    // view_dimension: wgpu::TextureViewDimension::D2,
                    // multisampled: false,
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::all(),
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(
                            ::std::mem::size_of::<RenderUniforms>() as u64
                        ),
                    },
                    // ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("render_pipeline_layout"),
        bind_group_layouts: &[Some(&texture_bind_group_layout)],
        immediate_size: 8,
        // push_constant_ranges: &[],
    });
    let pipeline_descriptor = wgpu::RenderPipelineDescriptor {
        label: Some("render_pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader_module,
            entry_point: Some(&"vert_main"),
            buffers: &[Some(Vert::desc())],
            compilation_options: Default::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Cw,
            cull_mode: None,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth24PlusStencil8,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: wgpu::StencilState { // pass 1 pipeline (stp 0) TODO: global_masking_enabled
                front: wgpu::StencilFaceState {
                    compare: wgpu::CompareFunction::Always,
                    fail_op: wgpu::StencilOperation::Keep, // Drop pixel, leave stencil alone
                    depth_fail_op: wgpu::StencilOperation::Keep,
                    pass_op: wgpu::StencilOperation::Replace, // Overwrite with new STP value
                },
                back: wgpu::StencilFaceState::IGNORE,
                read_mask: 0x01,
                write_mask: 0x01,
            },
            bias: wgpu::DepthBiasState::default(),
        }),
        // depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader_module,
            entry_point: Some(&"frag_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                blend: Some(wgpu::BlendState::REPLACE),
                // blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                // blend: Some(wgpu::BlendState::ADDITIVE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: None,
        // multiview: None,
    };
    let mut addictive_descriptor = pipeline_descriptor.clone();
    addictive_descriptor.fragment.as_mut().unwrap().targets = &[Some(wgpu::ColorTargetState {
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        // blend: None,
        // blend: Some(wgpu::BlendState::ALPHA_BLENDING),
        blend: Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Src, // Y u not working????
                dst_factor: wgpu::BlendFactor::Src1,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::REPLACE,
        }),
        // blend: Some(wgpu::BlendState::ADDITIVE),
        write_mask: wgpu::ColorWrites::ALL,
    })];

    let addictive_pipeline = device.create_render_pipeline(&addictive_descriptor);

    let mut addictive_descriptor_mask_disabled = addictive_descriptor.clone();

    let mask_disabled_stencil_state = Some(wgpu::DepthStencilState {
        format: wgpu::TextureFormat::Depth24PlusStencil8,
        depth_write_enabled: Some(true),
        depth_compare: Some(wgpu::CompareFunction::LessEqual),
        stencil: wgpu::StencilState { // stage 2 (stp 1)
            front: wgpu::StencilFaceState {
                compare: wgpu::CompareFunction::Always, // always passes
                fail_op: wgpu::StencilOperation::Keep,  // Drop pixel, leave stencil alone
                depth_fail_op: wgpu::StencilOperation::Keep,
                pass_op: wgpu::StencilOperation::Replace, // Overwrite with new STP value
            },
            back: wgpu::StencilFaceState::IGNORE,
            read_mask: 0x01,
            write_mask: 0x01,
        },
        bias: wgpu::DepthBiasState::default(),
    });

    addictive_descriptor_mask_disabled.depth_stencil = mask_disabled_stencil_state.clone();

    let addictive_pipeline_mask_disabled =
        device.create_render_pipeline(&addictive_descriptor_mask_disabled);

    let mut subtractive_descriptor = pipeline_descriptor.clone();
    subtractive_descriptor.fragment.as_mut().unwrap().targets = &[Some(wgpu::ColorTargetState {
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        // blend: None,
        // blend: Some(wgpu::BlendState::ALPHA_BLENDING),
        blend: Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Src, // Y u not working????
                dst_factor: wgpu::BlendFactor::Src1,
                operation: wgpu::BlendOperation::ReverseSubtract,
            },
            alpha: wgpu::BlendComponent::REPLACE,
        }),
        // blend: Some(wgpu::BlendState::ADDITIVE),
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let subtractive_pipeline = device.create_render_pipeline(&subtractive_descriptor);

    let mut subtractive_descriptor_mask_disabled = subtractive_descriptor.clone();

    subtractive_descriptor_mask_disabled.depth_stencil = mask_disabled_stencil_state;

    let subtractive_pipeline_mask_disabled =
        device.create_render_pipeline(&subtractive_descriptor_mask_disabled);
    // let data_view = tileset_texture.create_view(&wgpu::TextureViewDescriptor::default());

    // let diffuse_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
    //     address_mode_u: wgpu::AddressMode::ClampToEdge,
    //     address_mode_v: wgpu::AddressMode::ClampToEdge,
    //     address_mode_w: wgpu::AddressMode::ClampToEdge,
    //     mag_filter: wgpu::FilterMode::Linear,
    //     min_filter: wgpu::FilterMode::Nearest,
    //     mipmap_filter: wgpu::MipmapFilterMode::Nearest,
    //     ..Default::default()
    // });
    // let texture_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
    //     label: Some("texture_params_buffer"),
    //     size: ::std::mem::size_of::<TextureBuffer>() as u64,
    //     usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    //     mapped_at_creation: false,
    // });
    // let texture_view = render_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let uniforms_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("render_uniforms_buffer"),
        size: ::std::mem::size_of::<RenderUniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
            queue
                .write_buffer(&uniforms_buffer, 0, bytemuck::cast_slice(&[scale as u32, 0_u32]));
    let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("texture bind_group"),
        layout: &texture_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&vram_texture_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uniforms_buffer.as_entire_binding(),
            },
        ],
    });

    let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
        size: wgpu::Extent3d {
            width: 1024 * scale,
            height: 512 * scale,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1, // We'll talk about this a little later
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth24PlusStencil8,
        // TEXTURE_BINDING tells wgpu that we want to use this texture in shaders
        // COPY_DST means that we want to copy data to this texture
        usage: wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        // | wgpu::TextureUsages::TEXTURE_BINDING,
        // | wgpu::TextureUsages::STORAGE_BINDING,
        label: Some("depth texture"),
        view_formats: &[wgpu::TextureFormat::Depth24PlusStencil8],
    });
    let depth_texture_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

    // let output_buffer = device.create_buffer(&wgpu::wgt::BufferDescriptor {
    //     label: Some("output"),
    //     size: 4 * 1024 * 512,
    //     usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
    //     mapped_at_creation: false,
    // });

    (
        addictive_pipeline,
        subtractive_pipeline,
        addictive_pipeline_mask_disabled,
        subtractive_pipeline_mask_disabled,
        vram_texture,
        blit_texture,
        vertex_buffer,
        uniforms_buffer,
        texture_bind_group,
        depth_texture_view,
    )
}

pub struct HWRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    addictive_pipeline: wgpu::RenderPipeline,
    subtractive_pipeline: wgpu::RenderPipeline,
    addictive_pipeline_mask_disabled: wgpu::RenderPipeline,
    subtractive_pipeline_mask_disabled: wgpu::RenderPipeline,
    render_texture: wgpu::Texture,
    vram_texture: wgpu::Texture,
    blit_texture: wgpu::Texture,
    vertex_buffer: wgpu::Buffer,
    texture_bind_group: wgpu::BindGroup,
    vertices: Vec<Vert>,
    render_view: wgpu::TextureView,
    drawing_area_top_left: (u16, u16),
    drawing_area_bottom_right: (u16, u16),
    dirty_region: DirtyRegion,
    profiler: GpuProfiler,
    profile_gpu: bool,
    frame_num: u64,
    results: Option<Vec<GpuTimerQueryResult>>,
    current_blending: Blending,
    clear_render_texture: bool,
    uniforms_buffer: wgpu::Buffer,
    global_masking_enabled: bool,
    depth_texture_view: wgpu::TextureView,
    force_set_mask_bit: bool, // blit_texture: wgpu::Texture,
                              // vram_blit_texture: wgpu::Texture,
                              // output_buffer: wgpu::Buffer,
    current_z: f32,
    scale: u32
}

impl HWRenderer {
    pub fn create(
        device: wgpu::Device,
        queue: wgpu::Queue,
        display_format: wgpu::TextureFormat,
        render_texture: wgpu::Texture,
        profile_gpu: bool,
        scale: u32,
    ) -> (
        Sender<RendererMsg>,
        Receiver<RendererResponse>,
        JoinHandle<()>,
    ) {
        let (to_gpu_sender, gpu_receiver) = crossbeam::channel::bounded(1024);
        let (to_renderer_sender, receiver) = crossbeam::channel::bounded(1024);

        let (
            addictive_pipeline,
            subtractive_pipeline,
            addictive_pipeline_mask_disabled,
            subtractive_pipeline_mask_disabled,
            vram_texture,
            blit_texture,
            vertex_buffer,
            uniforms_buffer,
            texture_bind_group,
            depth_texture_view,
        ) = create_draw_pipeline(&device, &queue, display_format, scale);
        let render_view = render_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let profiler = GpuProfiler::new(&device, GpuProfilerSettings::default()).unwrap();

        let handle = thread::spawn(move || {
            let mut renderer = HWRenderer {
                device,
                queue,
                addictive_pipeline,
                subtractive_pipeline,
                addictive_pipeline_mask_disabled,
                subtractive_pipeline_mask_disabled,
                render_texture,
                vram_texture,
                blit_texture,
                vertex_buffer,
                uniforms_buffer,
                texture_bind_group,
                vertices: vec![],
                render_view,
                drawing_area_top_left: (0, 0),
                drawing_area_bottom_right: (0, 0),
                dirty_region: DirtyRegion::empty(),
                profiler,
                profile_gpu,
                frame_num: 0,
                results: None,
                current_blending: Blending::Add,
                clear_render_texture: true,
                global_masking_enabled: false,
                force_set_mask_bit: false,
                depth_texture_view,
                current_z: 0.0,
                scale,
            };
            let core_ids = core_affinity::get_core_ids().unwrap();
            let res = core_affinity::set_for_current(core_ids[1]);
            if res {
                println!("renderer thread pinned to 1");
            }
            loop {
                let mut msg = match receiver.recv() {
                    Ok(msg) => msg,
                    Err(_) => return,
                };
                match &mut msg {
                    RendererMsg::DrawLine {
                        v0,
                        v1,
                        c0,
                        c1,
                        shaded,
                        semi_transparent,
                        ctx,
                    } => renderer.draw_line(*v0, *v1, *c0, *c1, *semi_transparent, *shaded, ctx),
                    RendererMsg::DrawRectangle {
                        v,
                        side,
                        c,
                        clut,
                        _u,
                        _v,
                        semi_transparent,
                        blend,
                        textured,
                        ctx,
                    } => renderer.render_rectangle(
                        *v,
                        *side,
                        *c,
                        *clut,
                        [*_u, *_v],
                        *semi_transparent,
                        *blend,
                        *textured,
                        ctx,
                    ),

                    RendererMsg::DrawPolygon {
                        cs,
                        clut,
                        page,
                        vs,
                        uvs,
                        semi_transparent,
                        blend,
                        is_triangle,
                        textured,
                        shaded,
                        ctx,
                    } => {
                        renderer.render_polygon(
                            *cs,
                            *clut,
                            *page,
                            vs,
                            uvs,
                            *textured,
                            *shaded,
                            *semi_transparent,
                            *blend,
                            *is_triangle,
                            ctx,
                        );
                    }
                    RendererMsg::FillRect { v, side, c } => renderer.fill_rect(*v, *side, *c),
                    RendererMsg::Vram2VramBlit {
                        src,
                        dst,
                        size,
                        ctx,
                    } => renderer.vram2vram_blit(*src, *dst, *size),
                    RendererMsg::RenderFB {
                        framebuffer,
                        full_ram,
                        ctx,
                    } => {
                        let (width, height, sx, sy, depth) =
                            renderer.render_fb(&framebuffer, *full_ram, ctx);
                        match to_gpu_sender.send(RendererResponse::FBUpdated {
                            width,
                            height,
                            sx,
                            sy,
                            depth,
                        }) {
                            Ok(_) => (),
                            Err(_) => return,
                        }
                    }
                    RendererMsg::CpuToVramCopy {
                        top_left,
                        size,
                        data,
                        ctx,
                    } => renderer.cpu_to_vram_copy(*top_left, *size, data),
                    RendererMsg::VramToCpuCopy { top_left, size } => {
                        let data = renderer.vram_to_cpu_copy(*top_left, *size);
                        match to_gpu_sender.send(RendererResponse::VramToCpuData { data }) {
                            Ok(_) => (),
                            Err(_) => return,
                        }
                    }
                    RendererMsg::DrawingAreaChange {
                        top_left,
                        bottom_right,
                    } => {
                        renderer.flush();
                        renderer.drawing_area_top_left.0 = top_left.0 * scale as u16;
                        renderer.drawing_area_top_left.1 = top_left.1;
                        renderer.drawing_area_bottom_right.0 = bottom_right.0 * scale as u16;
                        renderer.drawing_area_bottom_right.1 = bottom_right.1 * scale as u16;
                    }
                    RendererMsg::PreservedMaskedPixelsChanged {
                        preserve_masked_pixels,
                        force_set_mask_bit,
                    } => {
                        renderer.flush();
                        renderer.global_masking_enabled = *preserve_masked_pixels;
                        renderer.force_set_mask_bit = *force_set_mask_bit;
                    }
                    RendererMsg::PoisonPill => return,
                }
            }
        });

        (to_renderer_sender, gpu_receiver, handle)
    }

    pub fn flush(&mut self) {
        if !self.vertices.is_empty() {
            // first upload vertex buffer
            self.queue
                .write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.vertices));

                 // self.global_masking_enabled=false; 
                 // self.force_set_mask_bit=false; 
            if !self.force_set_mask_bit {
                // --- PASS 1: Process and write pixels that have STP = 0 ---

                let mut render_encoder =
                    self.device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("render_encoder"),
                        });
                {
                    let mut render_scope = self.profiler.scope("render", &mut render_encoder);

                    // self.clear_render_texture = false;
                    // let load_op = if self.clear_render_texture {
                    //     wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                    // } else {
                    //     wgpu::LoadOp::Load
                    // };
                    let zclearop = if self.clear_render_texture {
                         wgpu::LoadOp::Clear(1.0)
                    } else {
                         wgpu::LoadOp::Load
                    };
                    let mut rpass = render_scope.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("render_rpass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &self.render_view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                            depth_slice: None,
                        })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &self.depth_texture_view,
                            depth_ops: Some(wgpu::Operations {
                                load: zclearop,
                                // load: wgpu::LoadOp::Clear(1.0),
                                // load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            }),
                        }),
                        ..Default::default()
                    });
                    let pipeline = match (&self.current_blending) {
                        (Blending::Add) => &self.addictive_pipeline,
                        (Blending::Subtract) => &self.subtractive_pipeline,
                    };
                    // let pipeline = match (&self.current_blending, self.global_masking_enabled) {
                    //     (Blending::Add, true) => &self.addictive_pipeline,
                    //     (Blending::Add, false) => &self.addictive_pipeline_mask_disabled,
                    //     (Blending::Subtract, true) => &self.subtractive_pipeline,
                    //     (Blending::Subtract, false) => &self.subtractive_pipeline_mask_disabled,
                    // };
                    rpass.set_pipeline(pipeline);
                    // Update your uniform buffer so WGSL knows to only pass `STP == 0` pixels
                    rpass.set_immediates(
                        0,
                        bytemuck::cast_slice(&[0u32, self.force_set_mask_bit as u32]),
                    );

                        // test for 0, clear for 0
                        rpass.set_stencil_reference(0);
                    // match self.current_blending {
                    //     Blending::Average => rpass.set_blend_constant(wgpu::Color { r: 0.5, g: 0.5, b: 0.5, a: 1.0 }),
                    //     Blending::Quarter => rpass.set_blend_constant(wgpu::Color { r: 0.25, g: 0.25, b: 0.25, a: 1.0 }),
                    //     _ => {}
                    // }
                    rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                    rpass.set_scissor_rect(
                        self.drawing_area_top_left.0 as u32,
                        self.drawing_area_top_left.1 as u32,
                        min(
                            1024 * self.scale,
                            (1 + self.drawing_area_bottom_right.0 - self.drawing_area_top_left.0)
                                as u32,
                        ),
                        min(
                            512 * self.scale,
                            (1 + self.drawing_area_bottom_right.1 - self.drawing_area_top_left.1)
                                as u32,
                        ),
                    );
                    // rpass.set_bind_group(0, &self.camera_bind_group, &[]);

                    rpass.set_bind_group(0, &self.texture_bind_group, &[]);
                    rpass.draw(0..self.vertices.len() as u32, 0..1);
                }
                self.profiler.resolve_queries(&mut render_encoder);
                let error_scope = self.device.push_error_scope(
                    wgpu::ErrorFilter::Validation, // | wgpu::ErrorFilter::Internal
                                                   // | wgpu::ErrorFilter::OutOfMemory,
                );
                let idx = self.queue.submit(vec![render_encoder.finish()]);

                let maybe_error = pollster::block_on(error_scope.pop());
                if let Some(error) = maybe_error {
                    match error {
                        wgpu::Error::Validation { description, .. } => {
                            println!("Validation error caught: {description}");
                        }
                        wgpu::Error::OutOfMemory { .. } => {
                            println!("Out of memory!");
                        }
                        wgpu::Error::Internal { description, .. } => {
                            println!("Internal error: {description}");
                        }
                    }
                }

                // TODO: do we need sync here?
                // self.device
                //     .poll(wgpu::PollType::Wait {
                //         submission_index: Some(idx),
                //         timeout: None,
                //     })
                //     .expect("ok");
                // self.queue.on_submitted_work_done(|| {
                // println!("rendering done..");
                // });
            }

            // --- PASS 2: Process and write pixels that have STP = 1 ---

            // for v in &mut self.vertices {
            //     v.z += 0.5;
            // }
            // self.queue
            //     .write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.vertices));
            let mut render_encoder =
                self.device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("render_encoder2"),
                    });
            {
                let mut render_scope = self.profiler.scope("render", &mut render_encoder);

                // self.clear_render_texture = false;
                // let load_op = if self.clear_render_texture {
                //     wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                // } else {
                //     wgpu::LoadOp::Load
                // };
                let mut rpass = render_scope.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("render_rpass2"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.render_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.depth_texture_view,
                            depth_ops: Some(wgpu::Operations {
                                // load: wgpu::LoadOp::Clear(1.0),
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            }),
                        stencil_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                    }),
                    ..Default::default()
                });

                    let pipeline = match (&self.current_blending) {
                        (Blending::Add) => &self.addictive_pipeline_mask_disabled,
                        (Blending::Subtract) => &self.subtractive_pipeline_mask_disabled,
                    };
                // let pipeline = match (&self.current_blending, self.global_masking_enabled) {
                //     (Blending::Add, true) => &self.addictive_pipeline,
                //     (Blending::Add, false) => &self.addictive_pipeline_mask_disabled,
                //     (Blending::Subtract, true) => &self.subtractive_pipeline,
                //     (Blending::Subtract, false) => &self.subtractive_pipeline_mask_disabled,
                // };
                rpass.set_pipeline(pipeline);
            // Update your uniform buffer so WGSL knows to only pass `STP == 1` pixels
                rpass.set_immediates(
                    0,
                    bytemuck::cast_slice(&[1u32, self.force_set_mask_bit as u32]),
                );

                // println!("masking enabled?: {}", self.global_masking_enabled);
                if self.global_masking_enabled {
                    // If masking is on, we still only draw if the existing VRAM stencil bit EQUALS 0...
                    rpass.set_stencil_reference(0);
                    // ...but because pass_op is REPLACE, if it succeeds, it will overwrite VRAM with 0!
                    // Wait: We want VRAM to update to 1. WebGPU's stencil reference controls BOTH the comparison
                    // value AND the replace value.
                    // To fix this constraint under Masking = Enabled, you need a slightly split logic:
                } else {
                    // Masking off: Replace VRAM stencil with 1
                    rpass.set_stencil_reference(1);
                }
                // match self.current_blending {
                //     Blending::Average => rpass.set_blend_constant(wgpu::Color { r: 0.5, g: 0.5, b: 0.5, a: 1.0 }),
                //     Blending::Quarter => rpass.set_blend_constant(wgpu::Color { r: 0.25, g: 0.25, b: 0.25, a: 1.0 }),
                //     _ => {}
                // }
                rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                rpass.set_scissor_rect(
                    self.drawing_area_top_left.0 as u32,
                    self.drawing_area_top_left.1 as u32,
                    min(
                        1024 * self.scale,
                        (1 + self.drawing_area_bottom_right.0 - self.drawing_area_top_left.0)
                            as u32,
                    ),
                    min(
                        512 * self.scale,
                        (1 + self.drawing_area_bottom_right.1 - self.drawing_area_top_left.1)
                            as u32,
                    ),
                );
                // rpass.set_bind_group(0, &self.camera_bind_group, &[]);

                rpass.set_bind_group(0, &self.texture_bind_group, &[]);
                rpass.draw(0..self.vertices.len() as u32, 0..1);
            }
            self.profiler.resolve_queries(&mut render_encoder);
            let error_scope = self.device.push_error_scope(
                wgpu::ErrorFilter::Validation, // | wgpu::ErrorFilter::Internal
                                               // | wgpu::ErrorFilter::OutOfMemory,
            );
            let idx = self.queue.submit(vec![render_encoder.finish()]);
            // self.device
            //     .poll(wgpu::PollType::Wait {
            //         submission_index: Some(idx),
            //         timeout: None,
            //     })
            //     .expect("ok");
            //
            let maybe_error = pollster::block_on(error_scope.pop());
            if let Some(error) = maybe_error {
                match error {
                    wgpu::Error::Validation { description, .. } => {
                        println!("Validation error caught: {description}");
                    }
                    wgpu::Error::OutOfMemory { .. } => {
                        println!("Out of memory!");
                    }
                    wgpu::Error::Internal { description, .. } => {
                        println!("Internal error: {description}");
                    }
                }
            }

            self.vertices.clear();
            self.current_z =0.0;
        }
    }

    pub fn ensure_vertex_room(&mut self, n: usize) {
        if self.vertices.len() + n >= VERT_BUFFER_SIZE {
            self.flush();
        }
    }

    pub fn clip_rect(
        &self,
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
        ctx: &RenderingContext,
    ) -> Option<(i32, i32, i32, i32)> {
        if x1 - x0 > (1024 * self.scale as i32 -  1) || y1 - y0 > (512 * self.scale as i32 - 1) {
            return None;
        }
        let l = ctx.drawing_area_left as i32 * self.scale as i32;
        let t = ctx.drawing_area_top as i32 * self.scale as i32;
        let b = ctx.drawing_area_bottom as i32 * self.scale as i32;
        let r = ctx.drawing_area_right as i32 * self.scale as i32;

        if x1 < l || x0 > r || y1 < t || y0 > b {
            return None;
        }
        Some((x0.max(l), y0.max(t), x1.min(r), y1.min(b)))
    }

    pub fn fill_rect(&mut self, v: Vertex, side: Vertex, color: Colour) {
        let Vertex {
            x: width,
            y: height,
        } = side;
        let min_x = (v.x.max(0) as usize) * self.scale as usize;
        let min_y = (v.y.max(0) as usize) * self.scale as usize;
        let max_x = ((v.x + width).min(0x400) as usize) * self.scale as usize;
        let max_y = ((v.y + height).min(0x200) as usize) * self.scale as usize;

        self.prepare_draw(
            false,
            min_x as i32,
            min_y as i32,
            max_x as i32,
            max_y as i32,
            0,
        );
        self.dirty_region
            .merge(min_x as i32, min_y as i32, max_x as i32, max_y as i32);

        self.ensure_vertex_room(6);
        let flags = Flags(0);
        let v0 = (Vert {
            position: [min_x as i16, min_y as i16],
            color: [color.r, color.g, color.b],
            texture_depth: 0,
            uv: [0, 0],
            flags: flags.0,
            clut: [0, 0],
            texpage_base: [0, 0],
            // _pad: 0,
            texture_window_mask: [0, 0],
            texture_window_offset: [0, 0],
            z: self.current_z
        });
        let v1 = (Vert {
            position: [min_x as i16, max_y as i16],
            color: [color.r, color.g, color.b],
            texture_depth: 0,
            uv: [0, 0],
            flags: flags.0,
            clut: [0, 0],
            texpage_base: [0, 0],
            texture_window_mask: [0, 0],
            texture_window_offset: [0, 0],
            z: self.current_z
            // _pad: 0,
        });
        let v2 = (Vert {
            position: [max_x as i16, min_y as i16],
            color: [color.r, color.g, color.b],
            texture_depth: 0,
            uv: [0, 0],
            flags: flags.0,
            clut: [0, 0],
            texpage_base: [0, 0],
            texture_window_mask: [0, 0],
            texture_window_offset: [0, 0],
            z: self.current_z
            // _pad: 0,
        });
        let v3 = (Vert {
            position: [min_x as i16, max_y as i16],
            color: [color.r, color.g, color.b],
            texture_depth: 0,
            uv: [0, 0],
            flags: flags.0,
            clut: [0, 0],
            texpage_base: [0, 0],
            texture_window_mask: [0, 0],
            texture_window_offset: [0, 0],
            z: self.current_z
            // _pad: 0,
        });
        let v4 = (Vert {
            position: [max_x as i16, max_y as i16],
            color: [color.r, color.g, color.b],
            texture_depth: 0,
            uv: [0, 0],
            flags: flags.0,
            clut: [0, 0],
            texpage_base: [0, 0],
            texture_window_mask: [0, 0],
            texture_window_offset: [0, 0],
            z: self.current_z
            // _pad: 0,
        });
        let v5 = (Vert {
            position: [max_x as i16, min_y as i16],
            color: [color.r, color.g, color.b],
            texture_depth: 0,
            uv: [0, 0],
            flags: flags.0,
            clut: [0, 0],
            texpage_base: [0, 0],
            texture_window_mask: [0, 0],
            texture_window_offset: [0, 0],
            z: self.current_z
            // _pad: 0,
        });
        let (v0, v1, v2) = ensure_vertex_order(v0, v1, v2);
        let (v3, v4, v5) = ensure_vertex_order(v3, v4, v5);
        self.vertices.extend(&[v0, v1, v2, v3, v4, v5]);
    }

    pub fn draw_line(
        &mut self,
        mut v0: Vertex,
        mut v1: Vertex,
        c0: Colour,
        c1: Colour,
        semi_trans: bool,
        shaded: bool,
        ctx: &RenderingContext,
    ) {
        v0.x *= self.scale as i32;
        v0.y *= self.scale as i32;
        v1.x *= self.scale as i32;
        v1.y *= self.scale as i32;
        v0.x += ctx.drawing_x_offset as i32 * self.scale as i32;
        v0.y += ctx.drawing_y_offset as i32 * self.scale as i32;
        v1.x += ctx.drawing_x_offset as i32 * self.scale as i32;
        v1.y += ctx.drawing_y_offset as i32 * self.scale as i32;

        let Some((x0, y0, x1, y1)) = self.clip_rect(v0.x, v0.y, v1.x, v1.y, ctx) else {
            return;
        };
    }

    pub fn render_polygon(
        &mut self,
        colors: [Colour; 4],
        clut: u16,
        page: u16,
        vs: &mut [Vertex; 4],
        uv: &[[u16; 2]; 4],
        textured: bool,
        shaded: bool,
        semi_trans: bool,
        blend: bool,
        is_triangle: bool,
        ctx: &RenderingContext,
    ) {
        vs[0].x *= self.scale as i32;
        vs[0].y *= self.scale as i32;
        vs[1].x *= self.scale as i32;
        vs[1].y *= self.scale as i32;
        vs[2].x *= self.scale as i32;
        vs[2].y *= self.scale as i32;
        vs[3].x *= self.scale as i32;
        vs[3].y *= self.scale as i32;
        vs[0].x += ctx.drawing_x_offset as i32 * self.scale as i32;
        vs[0].y += ctx.drawing_y_offset as i32 * self.scale as i32;
        vs[1].x += ctx.drawing_x_offset as i32 * self.scale as i32;
        vs[1].y += ctx.drawing_y_offset as i32 * self.scale as i32;
        vs[2].x += ctx.drawing_x_offset as i32 * self.scale as i32;
        vs[2].y += ctx.drawing_y_offset as i32 * self.scale as i32;
        vs[3].x += ctx.drawing_x_offset as i32 * self.scale as i32;
        vs[3].y += ctx.drawing_y_offset as i32 * self.scale as i32;

        // bounding box
        // FIXME: culling should apply individually to each triangle
        let min_x = cmp::min(cmp::min(vs[0].x, cmp::min(vs[1].x, vs[2].x)), vs[3].x);
        let max_x = cmp::max(cmp::max(vs[0].x, cmp::max(vs[1].x, vs[2].x)), vs[3].x);
        let min_y = cmp::min(cmp::min(vs[0].y, cmp::min(vs[1].y, vs[2].y)), vs[3].y);
        let max_y = cmp::max(cmp::max(vs[0].y, cmp::max(vs[1].y, vs[2].y)), vs[3].y);

        let Some((min_x, min_y, max_x, max_y)) = self.clip_rect(min_x, min_y, max_x, max_y, ctx)
        else {
            return;
        };

        let transparency = if textured {
            let clut = Clut::new(clut);
            let texture = Texture::new(page, clut);
            texture.semi_transparency
        } else {
            ctx.semi_transparency
        };
        self.prepare_draw(semi_trans, min_x, min_y, max_x, max_y, transparency);

        self.dirty_region
            .merge(min_x as i32, min_y as i32, max_x as i32, max_y as i32);
        self.ensure_vertex_room(if is_triangle { 3 } else { 6 });

        self.render_triangle(
            &colors[0..3],
            clut,
            page,
            &vs[0..3],
            &uv[0..3],
            textured,
            semi_trans,
            blend,
            ctx,
        );

        if !is_triangle {
            self.render_triangle(
                &colors[1..4],
                clut,
                page,
                &vs[1..4],
                &uv[1..4],
                textured,
                semi_trans,
                blend,
                ctx,
            );
        }
    }

    pub fn render_triangle(
        &mut self,
        colors: &[Colour],
        clut: u16,
        page: u16,
        vs: &[Vertex],
        uv: &[[u16; 2]],
        textured: bool,
        semi_trans: bool,
        blend: bool,
        ctx: &RenderingContext,
    ) {
        // println!("trying to render triangle");

        let clut = Clut::new(clut);
        let texture = Texture::new(page, clut);
        let texture_depth = match texture.depth {
            TextureDepth::T4Bit => 0,
            TextureDepth::T8Bit => 1,
            TextureDepth::T15Bit => 2,
        };
        let clut = [clut.base_x as u16, clut.base_y as u16];
        let texpage_base = texture.get_texpage_base();

        let mut flags = Flags(0);
        flags.set_textured(textured);
        flags.set_blend(blend);
        flags.set_semitrans(semi_trans);
        flags.set_dither(texture.dithering);
        flags.set_transparency(texture.semi_transparency);

        let v0 = (Vert {
            position: [vs[0].x as i16, vs[0].y as i16],
            color: [colors[0].r, colors[0].g, colors[0].b],
            texture_depth,
            uv: uv[0],
            flags: flags.0,
            clut,
            texpage_base,
            // _pad: 0,
            texture_window_mask: [ctx.texture_window_x_mask, ctx.texture_window_y_mask],
            texture_window_offset: [ctx.texture_window_x_offset, ctx.texture_window_y_offset],
            z: self.current_z
        });
        let v1 = (Vert {
            position: [vs[1].x as i16, vs[1].y as i16],
            color: [colors[1].r, colors[1].g, colors[1].b],
            texture_depth,
            uv: uv[1],
            flags: flags.0,
            clut,
            texpage_base,
            texture_window_mask: [ctx.texture_window_x_mask, ctx.texture_window_y_mask],
            texture_window_offset: [ctx.texture_window_x_offset, ctx.texture_window_y_offset],
            z: self.current_z
            // _pad: 0,
        });
        let v2 = (Vert {
            position: [vs[2].x as i16, vs[2].y as i16],
            color: [colors[2].r, colors[2].g, colors[2].b],
            texture_depth,
            uv: uv[2],
            flags: flags.0,
            clut,
            texpage_base,
            texture_window_mask: [ctx.texture_window_x_mask, ctx.texture_window_y_mask],
            texture_window_offset: [ctx.texture_window_x_offset, ctx.texture_window_y_offset],
            z: self.current_z
            // _pad: 0,
        });
        let (v0, v1, v2) = ensure_vertex_order(v0, v1, v2);
        self.vertices.push(v0);
        self.vertices.push(v1);
        self.vertices.push(v2);
        // self.flush();
    }

    pub fn render_rectangle(
        &mut self,
        mut v: Vertex,
        mut side: Vertex,
        color: Colour,
        clut: u16,
        uv: [u16; 2],
        semi_trans: bool,
        blend: bool,
        textured: bool,
        ctx: &RenderingContext,
    ) {
        let tex_size_x = (side.x) as u16;
        let tex_size_y = (side.y) as u16;

        v.x *= self.scale as i32;
        v.y *= self.scale as i32;
        side.x *= self.scale as i32;
        side.y *= self.scale as i32;
        v.x += ctx.drawing_x_offset as i32 * self.scale as i32;
        v.y += ctx.drawing_y_offset as i32 * self.scale as i32;

        let Some((min_x, min_y, max_x, max_y)) =
            self.clip_rect(v.x, v.y, v.x + side.x - 1, v.y + side.y - 1, ctx)
        else {
            return;
        };
        // let side = Vertex {x: side.x , y: side.y - 1};
        //

        self.prepare_draw(
            semi_trans,
            min_x,
            min_y,
            max_x,
            max_y,
            ctx.semi_transparency,
        );
        self.dirty_region
            .merge(min_x as i32, min_y as i32, max_x as i32, max_y as i32);

        self.ensure_vertex_room(6);
        let mut flags = Flags(0);
        flags.set_textured(textured);
        flags.set_semitrans(semi_trans);
        flags.set_blend(blend);
        flags.set_dither(false);
        flags.set_transparency(ctx.semi_transparency);

        let clut = Clut::new(clut);
        let clut = [clut.base_x as u16, clut.base_y as u16];

        let texpage_base = [ctx.page_base_x, ctx.page_base_y];

        let texture_depth = match ctx.texture_depth {
            TextureDepth::T4Bit => 0,
            TextureDepth::T8Bit => 1,
            TextureDepth::T15Bit => 2,
        };


        // if textured {
        //    println!("render textured rectangle, uv: {}x{}, size: {}x{}, depth: {}", uv[0], uv[1], side.x, side.y, texture_depth);
        // }

        let v0 = (Vert {
            position: [v.x as i16, v.y as i16],
            uv,
            color: [color.r, color.g, color.b],
            texture_depth,
            flags: flags.0,
            clut,
            texpage_base,
            // _pad: 0,
            texture_window_mask: [ctx.texture_window_x_mask, ctx.texture_window_y_mask],
            texture_window_offset: [ctx.texture_window_x_offset, ctx.texture_window_y_offset],
            z: self.current_z
        });
        let v1 = (Vert {
            position: [v.x as i16, v.y as i16 + side.y as i16],
            uv: [uv[0], (uv[1] + tex_size_y)],
            color: [color.r, color.g, color.b],
            texture_depth,
            flags: flags.0,
            clut,
            texpage_base,
            texture_window_mask: [ctx.texture_window_x_mask, ctx.texture_window_y_mask],
            texture_window_offset: [ctx.texture_window_x_offset, ctx.texture_window_y_offset],
            z: self.current_z
        });
        let v2 = (Vert {
            position: [v.x as i16 + side.x as i16, v.y as i16 + side.y as i16],
            uv: [(uv[0] + tex_size_x), (uv[1] + tex_size_y)],
            color: [color.r, color.g, color.b],
            texture_depth,
            flags: flags.0,
            clut,
            texpage_base,
            // _pad: 0,
            texture_window_mask: [ctx.texture_window_x_mask, ctx.texture_window_y_mask],
            texture_window_offset: [ctx.texture_window_x_offset, ctx.texture_window_y_offset],
            z: self.current_z
        });

        let v3 = (Vert {
            position: [v.x as i16 + side.x as i16, v.y as i16 + side.y as i16],
            uv: [(uv[0] + tex_size_x), (uv[1] + tex_size_y)],
            color: [color.r, color.g, color.b],
            texture_depth,
            flags: flags.0,
            clut,
            texpage_base,
            // _pad: 0,
            texture_window_mask: [ctx.texture_window_x_mask, ctx.texture_window_y_mask],
            texture_window_offset: [ctx.texture_window_x_offset, ctx.texture_window_y_offset],
            z: self.current_z
        });
        let v4 = (Vert {
            position: [v.x as i16 + side.x as i16, v.y as i16],
            uv: [(uv[0] + tex_size_x), uv[1]],
            color: [color.r, color.g, color.b],
            texture_depth,
            flags: flags.0,
            clut,
            texpage_base,
            // _pad: 0,
            texture_window_mask: [ctx.texture_window_x_mask, ctx.texture_window_y_mask],
            texture_window_offset: [ctx.texture_window_x_offset, ctx.texture_window_y_offset],
            z: self.current_z
        });
        let v5 = (Vert {
            position: [v.x as i16, v.y as i16],
            uv: [uv[0], uv[1]],
            color: [color.r, color.g, color.b],
            texture_depth,
            flags: flags.0,
            clut,
            texpage_base,
            // _pad: 0,
            texture_window_mask: [ctx.texture_window_x_mask, ctx.texture_window_y_mask],
            texture_window_offset: [ctx.texture_window_x_offset, ctx.texture_window_y_offset],
            z: self.current_z
        });
        let (v0, v1, v2) = ensure_vertex_order(v0, v1, v2);
        let (v3, v4, v5) = ensure_vertex_order(v3, v4, v5);
        self.vertices.extend(&[v0, v1, v2, v3, v4, v5]);
    }

    // TODO: The transfer is affected by Mask setting.
    pub fn vram2vram_blit(&mut self, src: Vertex, dst: Vertex, size: Vertex) {
        // self.flush();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("vram_to_vram_blit_encoder"),
            });

        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.vram_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: src.x as u32,
                    y: src.y as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.blit_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: dst.x as u32,
                    y: dst.y as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: size.x as u32,
                height: size.y as u32,
                depth_or_array_layers: 1,
            },
        );

        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.blit_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: dst.x as u32,
                    y: dst.y as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.vram_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: dst.x as u32,
                    y: dst.y as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: size.x as u32,
                height: size.y as u32,
                depth_or_array_layers: 1,
            },
        );

        let idx = self.queue.submit(vec![encoder.finish()]);
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(idx),
                timeout: None,
            })
            .expect("ok");
    }

    // TODO: calculate dirty region and sync only it
    pub fn prepare_draw(
        &mut self,
        semi_transparent: bool,
        sx: i32,
        sy: i32,
        end_x: i32,
        end_y: i32,
        transparency: u8,
    ) {
        let blending = get_blending(transparency, semi_transparent);
        if semi_transparent {
            if self.dirty_region.intersects(sx, sy, end_x, end_y) {
                // self.flush();
                // self.sync_readback_buffer(sx as u32, sy as u32, end_x as u32, end_y as u32);
                self.dirty_region.clear();
            } else {
            }
        }
        if self.current_blending != blending {
            self.flush();
            self.current_blending = blending;
        }
        self.current_z += 1.0 / (VERT_BUFFER_SIZE as f32);
    }

    pub fn sync_readback_buffer(&mut self, sx: u32, sy: u32, end_x: u32, end_y: u32) {
        // let sx = 0;
        // let sy = 0;
        // let end_x = 64;
        // let end_y = 64;
        let height = end_y - sy;
        let width = end_x - sx; // in half words..
        //
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("sync_readback_buffer_encoder"),
            });

        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.render_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: sx as u32,
                    y: sy as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.vram_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: sx as u32,
                    y: sy as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: width as u32,
                height: height as u32,
                depth_or_array_layers: 1,
            },
        );

        let idx = self.queue.submit(vec![encoder.finish()]);

        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(idx),
                timeout: None,
            })
            .expect("ok");
    }

    // pub fn wait_for_render_to_finish(&mut self) {
    //     let mut encoder = self
    //         .device
    //         .create_command_encoder(&wgpu::CommandEncoderDescriptor {
    //             label: Some("sync_readback_buffer_encoder"),
    //         });
    //     let idx = self.queue.submit(vec![encoder.finish()]);
    //     self.device.poll(wgpu::PollType::Wait { submission_index: Some(idx), timeout: None }).expect("ok");
    // }

    #[allow(dead_code)]
    pub fn print_profiling_results(&self) {
        print!("\x1B[2J\x1B[1;1H"); // Clear terminal and put cursor to first row first column
        println!("Welcome to wgpu_profiler demo!");
        println!();
        // println!("Enabled device features: {enabled_features:?}");
        // println!();
        match &self.results {
            Some(results) => {
                let mut results: Vec<_> = results.iter().filter(|x| x.time.is_some()).collect();
                results.sort_by(|x, y| x.label.cmp(&y.label));
                let iter = results.chunk_by(|x, y| x.label.eq(&y.label));
                for xs in iter {
                    let label = xs[0].label.clone();
                    let ys: Vec<f64> = xs
                        .iter()
                        .map(|x| {
                            (x.time.clone().unwrap().end - x.time.clone().unwrap().start)
                                * 1000.0
                                * 1000.0
                        })
                        .collect(); // TODO: think smth better
                    let min = ys.iter().min_by(|a, b| a.total_cmp(b)).unwrap();
                    let max = ys.iter().max_by(|a, b| a.total_cmp(b)).unwrap();
                    let avg = ys.iter().sum::<f64>() / ys.len() as f64;
                    let total = ys.iter().sum::<f64>();

                    println!(
                        "min: {:.3}μs, max: {:.3}μs, avg: {:.3}μs, total: {:.3}μs  - {} ",
                        min, max, avg, total, label
                    );

                    // if let Some(time) = &scope.time {
                    //     println!(
                    //         "{:.3}μs - {}",
                    //         (time.end - time.start) * 1000.0 * 1000.0,
                    //         scope.label
                    //     );
                    // } else {
                    //     println!("n/a - {}", scope.label);
                    // }
                }
                println!("invocations: {}", results.len());
            }
            None => println!("No profiling results available yet!"),
        }
    }

    pub fn update_results(&mut self, res: Option<Vec<GpuTimerQueryResult>>) {
        let Some(res) = res else {
            return;
        };
        let results: Vec<_> = res.iter().filter(|x| x.time.is_some()).collect();
        if results.len() > 0 {
            self.results = Some(res);
        }
    }

    pub fn render_fb(
        &mut self,
        _framebuffer: &Mutex<Vec<u32>>,
        full_ram: bool,
        ctx: &RenderingContext,
    ) -> (usize, usize, usize, usize, DisplayDepth) {
        // here is a good place to copy our render texture to vram texture...

        self.flush();
        self.clear_render_texture = true;
        self.profiler.end_frame().unwrap();
        let results = self
            .profiler
            .process_finished_frame(self.queue.get_timestamp_period());

        self.update_results(results);
        if self.profile_gpu {
            if self.frame_num == 60 {
                self.print_profiling_results();
                self.frame_num = 0;
            }
        }

        self.frame_num += 1;
        // self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("ok");
        // self.sync_readback_buffer(0, 0, 1024, 512);

        if full_ram {
            (1024 * self.scale as usize, 512 * self.scale as usize, 0, 0, ctx.display_depth)
        } else {
            let (sx, sy, width, height, interlaced) = (
                ctx.display_vram_x_start as usize,
                ctx.display_vram_y_start as usize,
                ctx.hres.into_pixels(),
                ctx.vres.into_pixels(),
                ctx.interlaced,
            );
            (width, height, sx, sy, ctx.display_depth)
        }
        // let mut mutex = framebuffer.lock().unwrap();
        // let output_frame_buffer: &mut [Color] = mutex.as_mut();
        //
        // if ctx.display_disabled {
        //     for y in 0..16 {
        //         for x in 0..16 {
        //             output_frame_buffer[1024 * y + x] = Color { r: 0, g: 0, b: 0, a: 255 };
        //         }
        //     }
        //     return (16, 16);
        // }
        // if full_ram {
        //     for y in 0..512 {
        //         for x in 0..1024 {
        //             let vram_addr = 2 * (1024 * y + x);
        //             let pixel =
        //                 u16::from_le_bytes([self.vram[vram_addr], self.vram[vram_addr + 1]]);
        //
        //             let r = convert_5bit_to_8bit(pixel & 0x1F);
        //             let g = convert_5bit_to_8bit((pixel >> 5) & 0x1F);
        //             let b = convert_5bit_to_8bit((pixel >> 10) & 0x1F);
        //
        //             output_frame_buffer[1024 * y + x] = Color { r, g, b, a: 255 };
        //         }
        //     }
        //     (1024, 512)
        // } else {
        //     let (sx, sy, width, height, interlaced) = (
        //         ctx.display_vram_x_start as usize,
        //         ctx.display_vram_y_start as usize,
        //         ctx.hres.into_pixels(),
        //         ctx.vres.into_pixels(),
        //         ctx.interlaced,
        //     );
        //     match ctx.display_depth {
        //         DisplayDepth::D15Bits => {
        //             for y in 0..height {
        //                 for x in 0..width {
        //                     let vram_addr = 2 * (1024 * (sy + y) + (sx + x));
        //                     let pixel = u16::from_le_bytes([
        //                         self.vram[vram_addr],
        //                         self.vram[vram_addr + 1],
        //                     ]);
        //
        //                     let r = convert_5bit_to_8bit(pixel & 0x1F);
        //                     let g = convert_5bit_to_8bit((pixel >> 5) & 0x1F);
        //                     let b = convert_5bit_to_8bit((pixel >> 10) & 0x1F);
        //
        //                 output_frame_buffer[1024 * y + x] = Color { r, g, b, a: 255 };
        //             }
        //         }
        //     }
        //     DisplayDepth::D24Bits => {
        //         for y in 0..height {
        //             for x in 0..width {
        //                 let vram_addr = 2 * (1024 * (y + sy)) + 3 * sx + 3 * x;
        //                 let r = self.vram[vram_addr];
        //                 let g = self.vram[vram_addr + 1];
        //                 let b = self.vram[vram_addr + 2];
        //
        //                 output_frame_buffer[1024 * y + x] = Color { r, g, b, a: 255 };
        //             }
        //         }
        //     }
        // };
        // // black out lines outside of display field (wrong impl)
        // let vrange = (ctx.display_line_end - ctx.display_line_start) as usize;
        // if height >= vrange {
        //     let starting_row = vrange * 1024 * if interlaced { 2 } else { 1 };
        //     output_frame_buffer[starting_row..].fill(Color { r: 0, g: 0, b: 0, a: 255});
        // }
        // (width, height)
        // }
    }

    pub fn cpu_to_vram_copy(
        &mut self,
        top_left: (u16, u16),
        size: (u16, u16),
        unaligned_data: &Vec<u32>,
    ) {
        if top_left.0 > 1024 || top_left.1 > 512 {
            return;
        }
        if top_left.0 + size.0 > 1024 || top_left.1 + size.1 > 512 {
            return;
        }
        let cols = size.0 as usize;
        let rows = size.1 as usize;
        // self.flush();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("cpu_to_vram_copy_encoder"),
            });
        // align to 256 bytes... or 64 words
        let num_extents = (cols / 64) + 1;
        let mut data: Vec<u32> = vec![];
        {
            let padding_words = 64 - ((cols) % 64);
            let zeroes = vec![0; padding_words];
            let mut i: usize = 0;
            for _ in 0..rows {
                for j in 0..cols / 2 {
                    let word = unaligned_data[i + j];
                    data.push(word & 0xFFFF);
                    data.push((word >> 16) & 0xFFFF);
                }
                data.extend_from_slice(&zeroes[0..]);
                i += cols / 2;
            }
        }

        let data_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&data[0..]),
                usage: wgpu::BufferUsages::COPY_SRC,
            });
        // ok, now we need to align to 256 bytes per row...
        encoder.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer: &data_buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(num_extents as u32 * 256),
                    rows_per_image: Some(size.1 as u32),
                }, // layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(2 * 1024), rows_per_image: Some(512) }
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.render_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: top_left.0 as u32,
                    y: top_left.1 as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: size.0 as u32,
                height: size.1 as u32,
                depth_or_array_layers: 1,
            },
        );

        // now copy it to vram
        encoder.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer: &data_buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(num_extents as u32 * 256),
                    rows_per_image: Some(size.1 as u32),
                }, // layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(2 * 1024), rows_per_image: Some(512) }
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.vram_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: (top_left.0) as u32,
                    y: top_left.1 as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: size.0 as u32,
                height: size.1 as u32,
                depth_or_array_layers: 1,
            },
        );

        let idx = self.queue.submit(vec![encoder.finish()]);

        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(idx),
                timeout: None,
            })
            .expect("ok");

        // TODO: do we need sync here?
        // self.device.poll(wgpu::PollType::Wait { submission_index: Some(idx), timeout: None }).expect("ok");
        // self.queue.on_submitted_work_done(|| {
        // self.encoder
        // });
    }

    pub fn vram_to_cpu_copy(&mut self, top_left: (u16, u16), size: (u16, u16)) -> Vec<u32> {
        // self.flush();
        if top_left.0 > 1023 || top_left.1 > 511 {
            return vec![];
        }
        let imgsize = size.0 as u32 * size.1 as u32; // size in half-words
        // rounding so we have 16 bit of padding in last word
        // let imgsize = (imgsize + 1) & !1;

        // align to 256 bytes...
        let num_extents = (4 * (size.0 as usize) / 256) + 1;

        // println!(">> vram-to-cpu copy..");
        // println!("top_left: {:?}, size: {:?}", top_left, size);
        // println!(">> ");

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("vram_to_cpu_copy_encoder"),
            });
        let output_buffer = self.device.create_buffer(&wgpu::wgt::BufferDescriptor {
            label: Some("output*"),
            size: num_extents as u64 * 256 * (size.1 as u64),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.render_texture, // r16uint, so in half words, same as params
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: top_left.0 as u32,
                    y: top_left.1 as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &output_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(num_extents as u32 * 256), // 256... alignment
                    rows_per_image: Some(size.1 as u32),
                    // bytes_per_row: Some(2 * 1024),
                    // rows_per_image: Some(512),
                },
            },
            wgpu::Extent3d {
                width: size.0 as u32,
                height: size.1 as u32,
                depth_or_array_layers: 1,
            },
        );
        encoder.map_buffer_on_submit(&output_buffer, wgpu::MapMode::Read, .., |res| {
            match res {
                Ok(_) => {}
                Err(x) => println!(">>> error mapping buffer {}", x),
            };
        });
        let idx = self.queue.submit(vec![encoder.finish()]);
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(idx),
                timeout: None,
            })
            .expect("ok");

        let buffer_view = output_buffer.get_mapped_range(..).expect("success");
        let vram_data: &[u32] = bytemuck::cast_slice(&buffer_view[..]);
        let mut data = Vec::with_capacity(vram_data.len() / 2);

        let width_in_words = (((size.0 + 1) & !1) / 2) as usize;
        // FIXME: need to pack two words into one
        for i in 0..size.1 as usize {
            let start = i * num_extents * 256 / 4;
            data.extend_from_slice(&vram_data[start..start + width_in_words]);
        }
        // println!("expected len: {}, actual: {}", imgsize / 2, data.len());

        data
    }
    /*  pub fn clear_render_texture(&mut self) {
        let mut render_encoder =
            self.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("clear_encoder"),
                });
        {
            let mut render_scope = self.profiler.scope("render", &mut render_encoder);
            let mut rpass = render_scope.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("render_rpass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.render_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // load: wgpu::LoadOp::Load,
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            let pipeline = match self.current_blending {
                // Blending::None => &self.draw_pipeline,
                // Blending::Average => &self.avg_pipeline,
                Blending::Add => &self.addictive_pipeline,
                Blending::Subtract => &self.subtractive_pipeline,
                // Blending::Quarter => &self.quarter_pipeline,
            };
            rpass.set_pipeline(pipeline);
            // match self.current_blending {
            //     Blending::Average => rpass.set_blend_constant(wgpu::Color { r: 0.5, g: 0.5, b: 0.5, a: 1.0 }),
            //     Blending::Quarter => rpass.set_blend_constant(wgpu::Color { r: 0.25, g: 0.25, b: 0.25, a: 1.0 }),
            //     _ => {}
            // }
            rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            rpass.set_scissor_rect(
                self.drawing_area_top_left.0 as u32,
                self.drawing_area_top_left.1 as u32,
                min(
                    1024,
                    (1 + self.drawing_area_bottom_right.0 - self.drawing_area_top_left.0) as u32,
                ),
                min(
                    512,
                    (1 + self.drawing_area_bottom_right.1 - self.drawing_area_top_left.1) as u32,
                ),
            );
            // rpass.set_bind_group(0, &self.camera_bind_group, &[]);

            rpass.set_bind_group(0, &self.texture_bind_group, &[]);
            rpass.draw(0..0, 0..1);
        }
        self.profiler.resolve_queries(&mut render_encoder);
        let error_scope = self.device.push_error_scope(
            wgpu::ErrorFilter::Validation, // | wgpu::ErrorFilter::Internal
                                           // | wgpu::ErrorFilter::OutOfMemory,
        );
        let idx = self.queue.submit(vec![render_encoder.finish()]);
    }*/
}

pub struct DirtyRegion {
    ax: i32,
    ay: i32,
    bx: i32,
    by: i32,
    empty: bool,
}

impl DirtyRegion {
    pub fn empty() -> Self {
        DirtyRegion {
            ax: 0,
            ay: 0,
            bx: 0,
            by: 0,
            empty: true,
        }
    }

    pub fn clear(&mut self) {
        self.empty = true;
    }

    pub fn intersects(&self, ax: i32, ay: i32, bx: i32, by: i32) -> bool {
        if self.empty {
            false
        } else {
            (ax <= self.bx) && (bx >= self.ax) && (ay <= self.by) && (by >= self.ay)
        }
    }

    pub fn merge(&mut self, ax: i32, ay: i32, bx: i32, by: i32) {
        if self.empty {
            self.ax = ax;
            self.ay = ay;
            self.bx = bx;
            self.by = by;
            self.empty = false;
        } else {
            self.ax = min(self.ax, ax);
            self.ay = min(self.ay, ay);
            self.bx = max(self.bx, bx);
            self.by = max(self.by, by);
        }
    }
}

fn get_blending(transparency: u8, enabled: bool) -> Blending {
    if !enabled {
        Blending::Add
    } else {
        match transparency {
            0 => Blending::Add,
            1 => Blending::Add,
            2 => Blending::Subtract,
            _ => Blending::Add,
            // _ => Blending::Addictive,
        }
    }
}

#[derive(PartialEq)]
pub enum Blending {
    // None,
    // Average,
    Add,
    Subtract,
    // Quarter
}
fn ensure_vertex_order(v0: Vert, v1: Vert, v2: Vert) -> (Vert, Vert, Vert) {
    let cross_product_z: i64 = (v2.position[0] as i64 - v0.position[0] as i64)
        * (v1.position[1] as i64 - v0.position[1] as i64)
        - (v1.position[0] as i64 - v0.position[0] as i64)
            * (v2.position[1] as i64 - v0.position[1] as i64);

    // println!("cross product: {}", cross_product_z);
    // let cross_product_z =
    //     (v1.position[0] as i32 - v0.position[0] as i32) * (v2.position[1] as i32 - v0.position[1] as i32) - (v1.position[1] as i32 - v0.position[1] as i32) * (v2.position[0] as i32 - v0.position[0] as i32);
    if cross_product_z <= 0 {
        (v0, v1, v2)
    } else {
        (v1, v0, v2)
    }
}
