use std::{borrow::Cow, num::NonZeroU64};

use crate::{DisplayUniforms, TextureBuffer, VERTICES, Vertex};
use wgpu::util::DeviceExt;

pub fn create_draw_pipeline(
    device: &wgpu::Device,
    // queue: &wgpu::Queue,
    texture_format: wgpu::TextureFormat, // config.format..
    enable_crt_shader: bool,
) -> (
    wgpu::RenderPipeline,
    wgpu::Texture,
    wgpu::Buffer,
    wgpu::BindGroup,
) {
    let render_texture = device.create_texture(&wgpu::TextureDescriptor {
        size: wgpu::Extent3d {
            width: 1920,
            height: 1080,
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

    let depth_stencil: Option<wgpu::DepthStencilState> = None;

    let shader_source = if enable_crt_shader {
        Cow::Borrowed(include_str!("crt.wgsl"))
    } else {
        Cow::Borrowed(include_str!("empty.wgsl"))
    };
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("crt-shaders"),
        source: wgpu::ShaderSource::Wgsl(shader_source),
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
            label: Some("draw_texture_bind_group_layout"),
            entries: &[
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
        });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("draw_pipeline_layout"),
        bind_group_layouts: &[Some(&texture_bind_group_layout)],
        immediate_size: 0,
        // push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("draw_pipeline"),
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
                format: texture_format,
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
    let texture_view = render_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
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

    (pipeline, render_texture, vertex_buffer, texture_bind_group)
}
