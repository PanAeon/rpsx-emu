use std::{

    borrow::Cow,
    cmp::{self, max, min},
    num::{NonZeroU32, NonZeroU64},
    sync::Mutex,
    thread::{self, JoinHandle},
};

use bytemuck::Zeroable;
use crossbeam::channel::{Receiver, Sender};
use wgpu::{VERTEX_ALIGNMENT, util::DeviceExt};

use crate::{
    gpu::{Colour, DisplayDepth, HorizontalRes, TextureDepth, Vertex, VerticalRes},
    renderer::{Clut, RendererMsg, RendererResponse, RenderingContext, Texture},
};

const VERT_BUFFER_SIZE: usize = 3*64;
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    drawing_area_offset_x: u32,
    drawing_area_offset_y: u32,
    drawing_area_width: u32,
    drawing_area_height: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Vert {
    position: [u16; 2],

    color: [u8; 3],
    texture_depth: u8,

    uv: [u16; 2],

    clut: [u16; 2],

    texture_window_mask: [u8; 2],
    texture_window_offset: [u8; 2],

    texpage_base: [u8; 2],
    flags: u16,
    // _pad: u16, // draw_area_top_left: [u16;2],
    // draw_area_bottom_right: [u16;2],
}
impl Vert {
    // pub fn empty() -> Self {
    // }
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
) -> (
    wgpu::ComputePipeline,
    wgpu::Texture,
    wgpu::Buffer,
    wgpu::Buffer,
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

    let bins_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bins_buffer"),
        size: (64 * 64 * 64 * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let vram_texture_view = vram_texture.create_view(&wgpu::TextureViewDescriptor::default());

    let shader_source = Cow::Borrowed(include_str!("compute.wgsl"));
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("render shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source),
    });



    let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vertex_buffer"),
        size: (VERT_BUFFER_SIZE * size_of::<Vert>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let uniforms_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("uniforms_buffer"),
        size: (size_of::<Uniforms>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let compute_bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("compute_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::ReadWrite,
                        format: wgpu::TextureFormat::R32Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("compute_pipeline_layout"),
        bind_group_layouts: &[Some(&compute_bind_group_layout)],
        immediate_size: 0,
        // push_constant_ranges: &[],
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("compute pipeline"),
        layout: Some(&pipeline_layout),
        module: &shader_module,
        entry_point: Some("main"),
        compilation_options: Default::default(), // constants, which is cool...
        cache: None,
    });

    let compute_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("compute bind_group"),
        layout: &compute_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&vram_texture_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: vertex_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniforms_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: bins_buffer.as_entire_binding()
            },
        ],
    });






    // let bins_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
    //     label: Some("merge pipeline"),
    //     layout: Some(&pipeline_layout),
    //     module: &shader_module,
    //     entry_point: Some("bin"),
    //     compilation_options: Default::default(), // constants, which is cool...
    //     cache: None,
    // });

    // let clear_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
    //     label: Some("clear pipeline"),
    //     layout: Some(&pipeline_layout),
    //     module: &shader_module,
    //     entry_point: Some("clear_bins"),
    //     compilation_options: Default::default(), // constants, which is cool...
    //     cache: None,
    // });




    (
        pipeline,
        // bins_pipeline,
        // clear_pipeline,
        vram_texture,
        vertex_buffer,
        uniforms_buffer,
        bins_buffer,
        compute_bind_group,
    )
}

pub struct ComputeRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    draw_pipeline: wgpu::ComputePipeline,
    // bins_pipeline: wgpu::ComputePipeline,
    // clear_pipeline: wgpu::ComputePipeline,
    render_texture: wgpu::Texture,
    vram_texture: wgpu::Texture,
    vertex_buffer: wgpu::Buffer,
    uniforms_buffer: wgpu::Buffer,
    compute_bind_group: wgpu::BindGroup,
    vertices: Vec<Vert>,
    bins_buffer: wgpu::Buffer,
    // render_view: wgpu::TextureView,
    drawing_area_top_left: (u16, u16),
    drawing_area_bottom_right: (u16, u16),
    dirty_region: DirtyRegion,
    // blit_texture: wgpu::Texture,
    // vram_blit_texture: wgpu::Texture,
    // output_buffer: wgpu::Buffer,
}

impl ComputeRenderer {
    pub fn create(
        device: wgpu::Device,
        queue: wgpu::Queue,
        display_format: wgpu::TextureFormat,
        render_texture: wgpu::Texture,
    ) -> (
        Sender<RendererMsg>,
        Receiver<RendererResponse>,
        JoinHandle<()>,
    ) {
        let (to_gpu_sender, gpu_receiver) = crossbeam::channel::bounded(1024);
        let (to_renderer_sender, receiver) = crossbeam::channel::bounded(1024);

        let (draw_pipeline,  vram_texture, vertex_buffer, uniforms_buffer, bins_buffer, compute_bind_group) =
            create_draw_pipeline(&device, &queue, display_format);
        // let render_view = render_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let handle = thread::spawn(move || {
            let mut renderer = ComputeRenderer {
                device,
                queue,
                draw_pipeline,
                render_texture,
                vram_texture,
                vertex_buffer,
                uniforms_buffer,
                bins_buffer,
                compute_bind_group,
                // merge_bind_group,
                vertices: vec![],
                // render_view,
                drawing_area_top_left: (0, 0),
                drawing_area_bottom_right: (0, 0),
                dirty_region: DirtyRegion::empty(),
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
                    RendererMsg::FillRect { v, side, c, ctx } => renderer.fill_rect(*v, *side, *c),
                    RendererMsg::Vram2VramBlit { src, dst, size } => {
                        renderer.vram2vram_blit(*src, *dst, *size)
                    }
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
                        renderer.drawing_area_top_left = *top_left;
                        renderer.drawing_area_bottom_right = *bottom_right;
                    }
                }
            }
        });

        (to_renderer_sender, gpu_receiver, handle)
    }

    pub fn flush(&mut self) {
        // {
        // self.vertices.clear();
        // push two triangles instead...
        // let c1 = Colour { r: 255, g: 255, b: 255, m: 0 };
        // self.render_triangle(&[c1;4], 0, 0, vs, uv, false, false, false, ctx);
        // }
        if !self.vertices.is_empty() {
            // now we need to partition vertices into bins;
            let mut bins = [0xFFFFu32;64*64*64];
            let mut bin_indices = [0;64*64];
            for (i, vs) in self.vertices.chunks_exact(3).enumerate() {
                let min_x = cmp::min(vs[0].position[0], cmp::min(vs[1].position[0], vs[2].position[0]));
                let max_x = cmp::min(1023,cmp::max(vs[0].position[0], cmp::max(vs[1].position[0], vs[2].position[0])));
                let min_y =  cmp::min(vs[0].position[1], cmp::min(vs[1].position[1], vs[2].position[1]));
                let max_y = cmp::min( 511, (cmp::max(vs[0].position[1], cmp::max(vs[1].position[1], vs[2].position[1]))));

                
                
                let start_bin_x = min_x as usize / 16;
                let end_bin_x = max_x as usize / 16;
                let start_bin_y = min_y as usize / 8;
                let end_bin_y = max_y as usize / 8;

                for y in start_bin_y..=end_bin_y {
                    for x in start_bin_x..=end_bin_x {
                        let bin_idx = bin_indices[y*64+x];
                        bins[64*(y*64 + x) + bin_idx] = 3 * i as u32;
                        bin_indices[y*64+x] += 1;
                    }
                }
            }
            
            self.queue
                .write_buffer(&self.bins_buffer, 0, bytemuck::cast_slice(&bins));
            self.queue
                .write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.vertices));
            // let idx = self.queue.submit([]);
            // self.device.poll(wgpu::PollType::Wait { submission_index: Some(idx), timeout: None }).expect("ok");
            // TODO: upload uniforms...

            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("render_encoder"),
                });
            {
                let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("compute pass"),
                    timestamp_writes: None,
                });
                cpass.set_pipeline(&self.draw_pipeline);

                cpass.set_bind_group(0, &self.compute_bind_group, &[]);
                // let num_workgroups = (self.vertices.len().div_ceil(3)) as u32;
                cpass.dispatch_workgroups(8, 8, 1);
                // rpass.draw(0..self.vertices.len() as u32, 0..1);
            }
            let idx = self.queue.submit(vec![encoder.finish()]);

            // TODO: do we need sync here?
            // self.device.poll(wgpu::PollType::Wait { submission_index: Some(idx), timeout: None }).expect("ok");
            // self.queue.on_submitted_work_done(|| {
            // println!("rendering done..");
            // });

            self.vertices.clear();
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
        if x1 - x0 > 1023 || y1 - y0 > 511 {
            return None;
        }
        let l = ctx.drawing_area_left as i32;
        let t = ctx.drawing_area_top as i32;
        let b = ctx.drawing_area_bottom as i32;
        let r = ctx.drawing_area_right as i32;

        if x1 < l || x0 > r || y1 < t || y0 > b {
            return None;
        }
        Some((x0.max(l), y0.max(t), x1.min(r), y1.min(b)))
    }

    //  need to use texture copy for this.. ??
    pub fn fill_rect(&mut self, v: Vertex, side: Vertex, color: Colour) {
        let Vertex {
            x: width,
            y: height,
        } = side;
        let min_x = v.x.max(0) as usize;
        let min_y = v.y.max(0) as usize;
        let max_x = (v.x + width).min(0x400) as usize;
        let max_y = (v.y + height).min(0x200) as usize;


        // self.dirty_region
        //     .merge(min_x as i32, min_y as i32, max_x as i32, max_y as i32);

        let flags = Flags(1);
        let v0 =(Vert {
            position: [min_x as u16, min_y as u16],
            color: [color.r, color.g, color.b],
            texture_depth: 0,
            uv: [0, 0],
            flags: flags.0,
            clut: [0, 0],
            texpage_base: [0, 0],
            // _pad: 0,
            texture_window_mask: [0, 0],
            texture_window_offset: [0, 0],
        });
        let v1 = (Vert {
            position: [min_x as u16, max_y as u16],
            color: [color.r, color.g, color.b],
            texture_depth: 0,
            uv: [0, 0],
            flags: flags.0,
            clut: [0, 0],
            texpage_base: [0, 0],
            texture_window_mask: [0, 0],
            texture_window_offset: [0, 0],
            // _pad: 0,
        });
        let v2 = (Vert {
            position: [max_x as u16, min_y as u16],
            color: [color.r, color.g, color.b],
            texture_depth: 0,
            uv: [0, 0],
            flags: flags.0,
            clut: [0, 0],
            texpage_base: [0, 0],
            texture_window_mask: [0, 0],
            texture_window_offset: [0, 0],
            // _pad: 0,
        });
        let v3 = (Vert {
            position: [min_x as u16, max_y as u16],
            color: [color.r, color.g, color.b],
            texture_depth: 0,
            uv: [0, 0],
            flags: flags.0,
            clut: [0, 0],
            texpage_base: [0, 0],
            texture_window_mask: [0, 0],
            texture_window_offset: [0, 0],
            // _pad: 0,
        });
        let v4 = (Vert {
            position: [max_x as u16, max_y as u16],
            color: [color.r, color.g, color.b],
            texture_depth: 0,
            uv: [0, 0],
            flags: flags.0,
            clut: [0, 0],
            texpage_base: [0, 0],
            texture_window_mask: [0, 0],
            texture_window_offset: [0, 0],
            // _pad: 0,
        });
        let v5 = (Vert {
            position: [max_x as u16, min_y as u16],
            color: [color.r, color.g, color.b],
            texture_depth: 0,
            uv: [0, 0],
            flags: flags.0,
            clut: [0, 0],
            texpage_base: [0, 0],
            texture_window_mask: [0, 0],
            texture_window_offset: [0, 0],
            // _pad: 0,
        });
        let (v0, v1, v2) = ensure_vertex_order(v0, v1, v2);
        let (v3, v4, v5) = ensure_vertex_order(v3, v4, v5);

        self.ensure_vertex_room(3);
        self.vertices.extend_from_slice(&[v0, v1, v2]);
        self.ensure_vertex_room(3);
        self.vertices.extend_from_slice(&[v0, v1, v2]);
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
        v0.x += ctx.drawing_x_offset as i32;
        v0.y += ctx.drawing_y_offset as i32;
        v1.x += ctx.drawing_x_offset as i32;
        v1.y += ctx.drawing_y_offset as i32;

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
        vs[0].x += ctx.drawing_x_offset as i32;
        vs[0].y += ctx.drawing_y_offset as i32;
        vs[1].x += ctx.drawing_x_offset as i32;
        vs[1].y += ctx.drawing_y_offset as i32;
        vs[2].x += ctx.drawing_x_offset as i32;
        vs[2].y += ctx.drawing_y_offset as i32;
        vs[3].x += ctx.drawing_x_offset as i32;
        vs[3].y += ctx.drawing_y_offset as i32;

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
        self.prepare_draw(semi_trans, min_x, min_y, max_x, max_y);

        // self.dirty_region
        //     .merge(min_x as i32, min_y as i32, max_x as i32, max_y as i32);

        // self.ensure_vertex_room(6);
        // let c1 = Colour {
        //     r: 255,
        //     b: 255,
        //     g: 255,
        //     m: 0,
        // };
        // // cw
        // let vs = [
        //     Vertex { x: 36, y: 36 },
        //     Vertex { x: 150, y: 86 },
        //     Vertex { x: 72, y: 136 },
        // ];
        // self.render_triangle(
        //     &[c1; 3],
        //     clut,
        //     page,
        //     &vs,
        //     &uv[0..3],
        //     textured,
        //     semi_trans,
        //     blend,
        //     ctx,
        // );
        //
        // // ccw
        // let c1 = Colour {
        //     r: 0,
        //     b: 0,
        //     g: 255,
        //     m: 0,
        // };
        // let vs = [
        //     Vertex { x: 250, y: 180 },
        //     Vertex { x: 190, y: 120 },
        //     Vertex { x: 225, y: 236 },
        // ];
        // self.render_triangle(
        //     &[c1; 3],
        //     clut,
        //     page,
        //     &vs,
        //     &uv[0..3],
        //     textured,
        //     semi_trans,
        //     blend,
        //     ctx,
        // );
        self.ensure_vertex_room(3);
        self.render_triangle(&colors[0..3], clut, page, &vs[0..3], &uv[0..3], textured, semi_trans, blend, ctx);

        if !is_triangle {
           self.ensure_vertex_room(3);
           self.render_triangle(&colors[1..4], clut, page, &vs[1..4], &uv[1..4], textured, semi_trans, blend, ctx);
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
            position: [vs[0].x as u16, vs[0].y as u16],
            color: [colors[0].r, colors[0].g, colors[0].b],
            texture_depth,
            uv: uv[0],
            flags: flags.0,
            clut,
            texpage_base,
            // _pad: 0,
            texture_window_mask: [ctx.texture_window_x_mask, ctx.texture_window_y_mask],
            texture_window_offset: [ctx.texture_window_x_offset, ctx.texture_window_y_offset],
        });
        let v1 = (Vert {
            position: [vs[1].x as u16, vs[1].y as u16],
            color: [colors[1].r, colors[1].g, colors[1].b],
            texture_depth,
            uv: uv[1],
            flags: flags.0,
            clut,
            texpage_base,
            texture_window_mask: [ctx.texture_window_x_mask, ctx.texture_window_y_mask],
            texture_window_offset: [ctx.texture_window_x_offset, ctx.texture_window_y_offset],
            // _pad: 0,
        });
        let v2 = (Vert {
            position: [vs[2].x as u16, vs[2].y as u16],
            color: [colors[2].r, colors[2].g, colors[2].b],
            texture_depth,
            uv: uv[2],
            flags: flags.0,
            clut,
            texpage_base,
            texture_window_mask: [ctx.texture_window_x_mask, ctx.texture_window_y_mask],
            texture_window_offset: [ctx.texture_window_x_offset, ctx.texture_window_y_offset],
            // _pad: 0,
        });
        let (v0, v1, v2) = ensure_vertex_order(v0, v1, v2);
        self.vertices.push(v0);
        self.vertices.push(v1);
        self.vertices.push(v2);
    }

    pub fn render_rectangle(
        &mut self,
        mut v: Vertex,
        side: Vertex,
        color: Colour,
        clut: u16,
        uv: [u16; 2],
        semi_trans: bool,
        blend: bool,
        textured: bool,
        ctx: &RenderingContext,
    ) {
        v.x += ctx.drawing_x_offset as i32;
        v.y += ctx.drawing_y_offset as i32;

        let Some((min_x, min_y, max_x, max_y)) =
            self.clip_rect(v.x, v.y, v.x + side.x - 1, v.y + side.y - 1, ctx)
        else {
            return;
        };
        // let side = Vertex {x: side.x , y: side.y - 1};
        //

        self.prepare_draw(semi_trans, min_x, min_y, max_x, max_y);
        self.dirty_region
            .merge(min_x as i32, min_y as i32, max_x as i32, max_y as i32);

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

        let tex_size_x = (side.x) as u16;
        let tex_size_y = (side.y) as u16;

        // if textured {
        //    println!("render textured rectangle, uv: {}x{}, size: {}x{}, depth: {}", uv[0], uv[1], side.x, side.y, texture_depth);
        // }

        let v0 = (Vert {
            position: [v.x as u16, v.y as u16],
            uv,
            color: [color.r, color.g, color.b],
            texture_depth,
            flags: flags.0,
            clut,
            texpage_base,
            // _pad: 0,
            texture_window_mask: [ctx.texture_window_x_mask, ctx.texture_window_y_mask],
            texture_window_offset: [ctx.texture_window_x_offset, ctx.texture_window_y_offset],
        });
        let v1 = (Vert {
            position: [v.x as u16, v.y as u16 + side.y as u16],
            uv: [uv[0], (uv[1] + tex_size_y)],
            color: [color.r, color.g, color.b],
            texture_depth,
            flags: flags.0,
            clut,
            texpage_base,
            texture_window_mask: [ctx.texture_window_x_mask, ctx.texture_window_y_mask],
            texture_window_offset: [ctx.texture_window_x_offset, ctx.texture_window_y_offset],
        });
        let v2 = (Vert {
            position: [v.x as u16 + side.x as u16, v.y as u16 + side.y as u16],
            uv: [(uv[0] + tex_size_x), (uv[1] + tex_size_y)],
            color: [color.r, color.g, color.b],
            texture_depth,
            flags: flags.0,
            clut,
            texpage_base,
            // _pad: 0,
            texture_window_mask: [ctx.texture_window_x_mask, ctx.texture_window_y_mask],
            texture_window_offset: [ctx.texture_window_x_offset, ctx.texture_window_y_offset],
        });

        let v3 = (Vert {
            position: [v.x as u16 + side.x as u16, v.y as u16 + side.y as u16],
            uv: [(uv[0] + tex_size_x), (uv[1] + tex_size_y)],
            color: [color.r, color.g, color.b],
            texture_depth,
            flags: flags.0,
            clut,
            texpage_base,
            // _pad: 0,
            texture_window_mask: [ctx.texture_window_x_mask, ctx.texture_window_y_mask],
            texture_window_offset: [ctx.texture_window_x_offset, ctx.texture_window_y_offset],
        });
        let v4 = (Vert {
            position: [v.x as u16 + side.x as u16, v.y as u16],
            uv: [(uv[0] + tex_size_x), uv[1]],
            color: [color.r, color.g, color.b],
            texture_depth,
            flags: flags.0,
            clut,
            texpage_base,
            // _pad: 0,
            texture_window_mask: [ctx.texture_window_x_mask, ctx.texture_window_y_mask],
            texture_window_offset: [ctx.texture_window_x_offset, ctx.texture_window_y_offset],
        });
        let v5 = (Vert {
            position: [v.x as u16, v.y as u16],
            uv: [uv[0], uv[1]],
            color: [color.r, color.g, color.b],
            texture_depth,
            flags: flags.0,
            clut,
            texpage_base,
            // _pad: 0,
            texture_window_mask: [ctx.texture_window_x_mask, ctx.texture_window_y_mask],
            texture_window_offset: [ctx.texture_window_x_offset, ctx.texture_window_y_offset],
        });
        let (v0, v1, v2) = ensure_vertex_order(v0, v1, v2);
        let (v3, v4, v5) = ensure_vertex_order(v3, v4, v5);
        self.ensure_vertex_room(3);
        self.vertices.extend_from_slice(&[v0, v1, v2]);
        self.ensure_vertex_room(3);
        self.vertices.extend_from_slice(&[v3, v4, v5]);
    }

    // TODO: The transfer is affected by Mask setting.
    pub fn vram2vram_blit(&mut self, src: Vertex, dst: Vertex, size: Vertex) {
        self.flush();
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
                texture: &self.render_texture,
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
                texture: &self.render_texture,
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
    }

    // TODO: calculate dirty region and sync only it
    pub fn prepare_draw(
        &mut self,
        semi_transparent: bool,
        sx: i32,
        sy: i32,
        end_x: i32,
        end_y: i32,
    ) {
        if semi_transparent {
            if self.dirty_region.intersects(sx, sy, end_x, end_y) {
                // self.flush();
                // self.sync_readback_buffer(sx as u32, sy as u32, end_x as u32, end_y as u32);
                // self.dirty_region.clear();
            } else {
            }
        }
    }

    pub fn sync_vram(&mut self) {
        //
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("sync_readback_buffer_encoder"),
            });

        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.vram_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0 as u32,
                    y: 0 as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.render_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0 as u32,
                    y: 0 as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: 1024 as u32,
                height: 512 as u32,
                depth_or_array_layers: 1,
            },
        );

        let idx = self.queue.submit(vec![encoder.finish()]);
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

    pub fn render_fb(
        &mut self,
        _framebuffer: &Mutex<Vec<u32>>,
        full_ram: bool,
        ctx: &RenderingContext,
    ) -> (usize, usize, usize, usize, DisplayDepth) {
        // here is a good place to copy our render texture to vram texture...

        self.flush();
        // self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("ok");
        self.sync_vram();
        // self.sync_readback_buffer(0, 0, 1024, 512);

        if full_ram {
            (1024, 512, 0, 0, ctx.display_depth)
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
        let cols = size.0 as usize;
        let rows = size.1 as usize;
        self.flush();
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

        // TODO: do we need sync here?
        // self.device.poll(wgpu::PollType::Wait { submission_index: Some(idx), timeout: None }).expect("ok");
        // self.queue.on_submitted_work_done(|| {
        // self.encoder
        // });
    }

    pub fn vram_to_cpu_copy(&mut self, top_left: (u16, u16), size: (u16, u16)) -> Vec<u32> {
        self.flush();
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
fn ensure_vertex_order(v0: Vert, v1: Vert, v2: Vert) -> (Vert, Vert, Vert) {
    // let cross_product_z =
    //     (vs[1].x - vs[0].x) * (vs[2].y - vs[0].y) - (vs[1].y - vs[0].y) * (vs[2].x - vs[0].x);
    // (v1, v0, v2)
    let cross_product_z =
        (v1.position[0] as i32 - v0.position[0] as i32) * (v2.position[1] as i32 - v0.position[1] as i32) - (v1.position[1] as i32 - v0.position[1] as i32) * (v2.position[0] as i32 - v0.position[0] as i32);
    if cross_product_z < 0 {
       (v0, v1, v2)
    } else {
        (v1, v0, v2)
    }
}
