use std::borrow::Cow;

use crate::{VERTICES, Vertex};
use wgpu::util::DeviceExt;




fn create_draw_pipeline(
    device: &wgpu::Device,
    // queue: &wgpu::Queue,
    display_format: wgpu::TextureFormat,
) -> (
    wgpu::RenderPipeline,
    wgpu::Texture,
    wgpu::Buffer,
    wgpu::BindGroup,
) {


    let vram_texture = device.create_texture(&wgpu::TextureDescriptor {
        size: wgpu::Extent3d {
            width: 1024,
            height: 512,
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

    let vram_texture_view= vram_texture.create_view(&wgpu::TextureViewDescriptor::default());

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

    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertex_buffer"),
            contents: bytemuck::cast_slice(VERTICES),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
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
                // wgpu::BindGroupLayoutEntry {
                //     binding: 1,
                //     visibility: wgpu::ShaderStages::FRAGMENT,
                //     ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                //     count: None,
                // },
            ],
        });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("render_pipeline_layout"),
        bind_group_layouts: &[Some(&texture_bind_group_layout)],
        immediate_size: 0,
        // push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("render_pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader_module,
            entry_point: Some(&"vert_main"),
            buffers: &[Some(Vertex::desc())],
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
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader_module,
            entry_point: Some(&"frag_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::R32Uint,
                blend: None,
                // blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: None,
        // multiview: None,
    });

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
    let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("texture bind_group"),
        layout: &texture_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&vram_texture_view),
            },
            // wgpu::BindGroupEntry {
            //     binding: 1,
            //     resource: wgpu::BindingResource::Sampler(&diffuse_sampler),
            // },
        ],
    });

        // let output_buffer = device.create_buffer(&wgpu::wgt::BufferDescriptor {
        //     label: Some("output"),
        //     size: 4 * 1024 * 512,
        //     usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        //     mapped_at_creation: false,
        // });

    (pipeline, vram_texture, vertex_buffer, texture_bind_group)
}

