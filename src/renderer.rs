use std::arch::x86_64::{
    _mm256_add_epi32, _mm256_cmpeq_epi16, _mm256_cmpeq_epi32, _mm256_cmpgt_epi32,
    _mm256_i32gather_epi32, _mm256_mullo_epi16, _mm256_mullo_epi32, _mm256_packus_epi32,
    _mm256_set1_epi32, _mm256_shuffle_epi8, _mm256_slli_epi16, _mm256_slli_epi32,
    _mm256_srli_epi32, _mm256_srlv_epi32, _mm256_sub_epi32,
};
use std::ops::{Deref, DerefMut};
use std::{
    arch::x86_64::{
        __m256i, _mm256_add_epi16, _mm256_and_si256, _mm256_andnot_si256, _mm256_max_epi16,
        _mm256_min_epi16, _mm256_or_si256, _mm256_set1_epi16, _mm256_setzero_si256,
        _mm256_srli_epi16, _mm256_sub_epi16,
    },
    cmp,
    ops::Sub,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};
use std::{collections, mem};

use crossbeam::channel::{Receiver, Sender};
use std::arch::x86_64::_mm256_loadu_si256;
use std::arch::x86_64::_mm256_setr_epi16;
use std::arch::x86_64::{
    _mm256_cmpgt_epi16, _mm256_packs_epi32, _mm256_setr_epi32, _mm256_testz_si256,
};

use crate::gpu::{Colour, DisplayDepth, HorizontalRes, TextureDepth, Vertex, VerticalRes};
use std::arch::x86_64::_mm256_load_si256;

use std::arch::x86_64::_mm256_storeu_si256;
pub enum RendererMsg {
    DrawLine {
        v0: Vertex,
        v1: Vertex,
        c0: Colour,
        c1: Colour,
        semi_transparent: bool,
        shaded: bool,
        ctx: RenderingContext,
    },
    DrawRectangle {
        v: Vertex,
        side: Vertex,
        c: Colour,
        clut: u16,
        _u: u16,
        _v: u16,
        semi_transparent: bool,
        blend: bool,
        textured: bool,
        ctx: RenderingContext,
    },
    DrawPolygon {
        is_triangle: bool,
        cs: [Colour; 4],
        clut: u16,
        page: u16,
        vs: [Vertex; 4],
        uvs: [[u16; 2]; 4],
        semi_transparent: bool,
        blend: bool,
        textured: bool,
        shaded: bool,
        ctx: RenderingContext,
    },
    FillRect {
        v: Vertex,
        side: Vertex,
        c: Colour,
    },
    Vram2VramBlit {
        src: Vertex,
        dst: Vertex,
        size: Vertex,
        ctx: RenderingContext,
    },
    RenderFB {
        framebuffer: Arc<Mutex<Vec<u32>>>,
        full_ram: bool,
        ctx: RenderingContext,
    },
    CpuToVramCopy {
        top_left: (u16, u16),
        size: (u16, u16),
        data: Vec<u32>,
        ctx: RenderingContext,
    },
    VramToCpuCopy {
        top_left: (u16, u16),
        size: (u16, u16),
    },
    DrawingAreaChange {
        top_left: (u16, u16),
        bottom_right: (u16, u16),
    },
    PreservedMaskedPixelsChanged {
        preserve_masked_pixels: bool,
        force_set_mask_bit: bool,
    },
    PoisonPill,
}
pub enum RendererResponse {
    FBUpdated {
        width: usize,
        height: usize,
        sx: usize,
        sy: usize,
        depth: DisplayDepth,
    },
    VramToCpuData {
        data: Vec<u32>,
    },
}

#[allow(dead_code)]
pub struct RenderingContext {
    // Texture page base X coordinate (4 bits, 64 byte increment)
    pub page_base_x: u8,
    // Texture page base Y coordinate (1 bit, 256 line increment)
    pub page_base_y: u8,
    pub semi_transparency: u8, // Semi Transparency     (0=B/2+F/2, 1=B+F, 2=B-F, 3=B+F/4)
    pub texture_depth: TextureDepth,
    // dithering from 24 to 16 bits RGB
    pub dithering: bool,
    pub draw_to_display: bool,
    // force "mask" bit of the pixel to 1 when writing to VRAM
    pub force_set_mask_bit: bool,
    // don't draw to pixels which have the "mask" bit set
    pub preserve_masked_pixels: bool,
    pub texture_disable: bool,
    pub hres: HorizontalRes,
    pub vres: VerticalRes,
    // gpu itself always draws 15bit RGB, 24bit output must use external assets
    pub display_depth: DisplayDepth,
    pub interlaced: bool,
    pub display_disabled: bool,
    pub rectange_texture_x_flip: bool,
    pub rectange_texture_y_flip: bool,
    pub texture_window_x_mask: u8,
    pub texture_window_y_mask: u8,
    pub texture_window_x_offset: u8,
    pub texture_window_y_offset: u8,
    pub drawing_area_left: u16,
    pub drawing_area_top: u16,
    pub drawing_area_right: u16,
    pub drawing_area_bottom: u16,
    pub drawing_x_offset: i16,
    pub drawing_y_offset: i16,
    pub display_vram_x_start: u16,
    pub display_vram_y_start: u16,
    pub display_horiz_start: u16,
    pub display_horiz_end: u16,
    pub display_line_start: u16,
    pub display_line_end: u16,
}

#[repr(align(32), C)]
#[derive(Debug, Clone)]
struct AlignedVram([u8; 2 * 1024 * 512]);

impl AlignedVram {
    fn new_on_heap() -> Box<Self> {
        let mut vram = Box::<Self>::new_uninit();

        // SAFETY: AlignedVram's only field is zerofilled before assuming initialized
        unsafe {
            (*vram.as_mut_ptr()).0.fill(0);
            vram.assume_init()
        }
    }
}

impl Deref for AlignedVram {
    type Target = [u8; 2 * 1024 * 512];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for AlignedVram {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub struct Renderer {
    vram: Box<AlignedVram>,
    profile: bool,
    frame_num: usize,
    frame_times: [Duration; 60],
    // to_gpu_sender: Sender<RendererResponse>,
}

impl Renderer {
    pub fn create(
        profile: bool,
    ) -> (
        Sender<RendererMsg>,
        Receiver<RendererResponse>,
        JoinHandle<()>,
    ) {
        let (to_gpu_sender, gpu_receiver) = crossbeam::channel::bounded(4096);
        let (to_renderer_sender, receiver) = crossbeam::channel::bounded(4096);

        // let aligned_vram =
        //        vec![0_u32; 1024 * 512 / 2].into_boxed_slice();

        // let aligned_vram: Box<u8> = bytemuck::cast(aligned_vram);

        let handle = thread::spawn(move || {
            let mut renderer = Renderer {
                vram: AlignedVram::new_on_heap(),
                profile,
                frame_num: 0,
                frame_times: [Duration::from_secs(0); 60],
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
                let start_time = std::time::Instant::now();
                match &mut msg {
                    RendererMsg::DrawLine {
                        v0,
                        v1,
                        c0,
                        c1,
                        semi_transparent,
                        shaded,
                        ctx,
                    } => match (*semi_transparent, *shaded) {
                        (true, true) => {
                            renderer.draw_line_shaded::<SEMI_TRANS>(*v0, *v1, *c0, *c1, ctx)
                        }
                        (false, true) => {
                            renderer.draw_line_shaded::<OPAQUE>(*v0, *v1, *c0, *c1, ctx)
                        }
                        (false, false) => renderer.draw_line::<OPAQUE>(*v0, *v1, *c0, ctx),
                        (true, false) => renderer.draw_line::<SEMI_TRANS>(*v0, *v1, *c0, ctx),
                    },
                    RendererMsg::DrawRectangle {
                        v,
                        side,
                        c,
                        clut,
                        _u,
                        _v,
                        semi_transparent,
                        textured,
                        blend,
                        ctx,
                    } => match (*semi_transparent, *blend, *textured) {
                        (true, true, true) => renderer
                            .draw_rectangle_textured::<SEMI_TRANS, BLEND>(
                                *v,
                                *side,
                                *c,
                                *clut,
                                [*_u, *_v],
                                ctx,
                            ),
                        (true, false, true) => renderer.draw_rectangle_textured::<SEMI_TRANS, RAW>(
                            *v,
                            *side,
                            *c,
                            *clut,
                            [*_u, *_v],
                            ctx,
                        ),
                        (false, true, true) => renderer.draw_rectangle_textured::<OPAQUE, BLEND>(
                            *v,
                            *side,
                            *c,
                            *clut,
                            [*_u, *_v],
                            ctx,
                        ),
                        (false, false, true) => renderer.draw_rectangle_textured::<OPAQUE, RAW>(
                            *v,
                            *side,
                            *c,
                            *clut,
                            [*_u, *_v],
                            ctx,
                        ),
                        (true, false, false) => {
                            renderer.draw_rectangle::<SEMI_TRANS>(*v, *side, *c, ctx)
                        }
                        (false, false, false) => {
                            renderer.draw_rectangle::<OPAQUE>(*v, *side, *c, ctx)
                        }
                        _ => unreachable!("wrong combo"),
                    },
                    RendererMsg::DrawPolygon {
                        cs,
                        clut,
                        page,
                        vs,
                        uvs,
                        semi_transparent,
                        blend,
                        is_triangle,
                        shaded,
                        textured,
                        ctx,
                    } => match (*semi_transparent, *blend, *shaded, *textured) {
                        (true, true, true, true) => renderer
                            .draw_polygon_textured_shaded::<SEMI_TRANS, BLEND>(
                                cs,
                                *clut,
                                *page,
                                vs,
                                uvs,
                                *is_triangle,
                                ctx,
                            ),
                        (true, false, true, true) => renderer
                            .draw_polygon_textured_shaded::<SEMI_TRANS, RAW>(
                                cs,
                                *clut,
                                *page,
                                vs,
                                uvs,
                                *is_triangle,
                                ctx,
                            ),
                        (false, true, true, true) => renderer
                            .draw_polygon_textured_shaded::<OPAQUE, BLEND>(
                                cs,
                                *clut,
                                *page,
                                vs,
                                uvs,
                                *is_triangle,
                                ctx,
                            ),
                        (false, false, true, true) => renderer
                            .draw_polygon_textured_shaded::<OPAQUE, RAW>(
                                cs,
                                *clut,
                                *page,
                                vs,
                                uvs,
                                *is_triangle,
                                ctx,
                            ),

                        (true, true, false, true) => renderer
                            .draw_polygon_textured::<SEMI_TRANS, BLEND>(
                                cs,
                                *clut,
                                *page,
                                vs,
                                uvs,
                                *is_triangle,
                                ctx,
                            ),
                        (true, false, false, true) => renderer
                            .draw_polygon_textured::<SEMI_TRANS, RAW>(
                                cs,
                                *clut,
                                *page,
                                vs,
                                uvs,
                                *is_triangle,
                                ctx,
                            ),
                        (false, true, false, true) => renderer
                            .draw_polygon_textured::<OPAQUE, BLEND>(
                                cs,
                                *clut,
                                *page,
                                vs,
                                uvs,
                                *is_triangle,
                                ctx,
                            ),
                        (false, false, false, true) => renderer
                            .draw_polygon_textured::<OPAQUE, RAW>(
                                cs,
                                *clut,
                                *page,
                                vs,
                                uvs,
                                *is_triangle,
                                ctx,
                            ),

                        (true, false, true, false) => {
                            renderer.draw_polygon_shaded::<SEMI_TRANS>(vs, cs, *is_triangle, ctx)
                        }
                        (false, false, true, false) => {
                            renderer.draw_polygon_shaded::<OPAQUE>(vs, cs, *is_triangle, ctx)
                        }

                        (true, false, false, false) => {
                            renderer.draw_polygon_mono::<SEMI_TRANS>(cs, vs, *is_triangle, ctx)
                        }
                        (false, false, false, false) => {
                            renderer.draw_polygon_mono::<OPAQUE>(cs, vs, *is_triangle, ctx)
                        }
                        _ => unreachable!("wrong combo"),
                    },
                    RendererMsg::FillRect { v, side, c, .. } => renderer.fill_rect(*v, *side, *c),
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
                    RendererMsg::DrawingAreaChange { .. } => {
                        // ignore for now..
                    }
                    RendererMsg::PreservedMaskedPixelsChanged { .. } => {
                        // ignore for now
                    }
                    RendererMsg::PoisonPill => {
                        return;
                    }
                }
                let end_time = std::time::Instant::now();
                let render_duration = end_time.sub(start_time);
                renderer.frame_times[renderer.frame_num] = render_duration;
                // match msg {
                //     GpuMsg::DataGP0(data) => gpu.gp0(data),
                //     GpuMsg::DataGP1(data) => gpu.gp1(data),
                //     GpuMsg::ReadGP0 => match from_gpu_sender.send(gpu.read()) {
                //         Ok(_) => (),
                //         Err(_) => return,
                //     },
                //     GpuMsg::ReadStatus => match from_gpu_sender.send(gpu.status()) {
                //         Ok(_) => (),
                //         Err(_) => return,
                //     },
                //     GpuMsg::EnterHSync => gpu.enter_hsync(),
                //     GpuMsg::EnterVSync => gpu.enter_vsync(),
                //     GpuMsg::ExitHSync => gpu.exit_hsync(),
                //     GpuMsg::ExitVSync => gpu.exit_vsync(),
                //     GpuMsg::ProduceFB(buffer, is_full_ram) => {
                //         let mut mutex = buffer.lock().unwrap();
                //         let fb = mutex.as_mut();
                //         let (w, h) = gpu.render_vram(fb, is_full_ram);
                //         match ctrl_sender.send((w, h)) {
                //             Ok(_) => (),
                //             Err(_) => return,
                //         };
                //     }
                // }
            }
        });

        (to_renderer_sender, gpu_receiver, handle)
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

    pub fn vram_write_color(&mut self, address: usize, color: Colour, ctx: &RenderingContext) {
        if ctx.preserve_masked_pixels && (self.vram[address + 1] & 0x80) != 0 {
            return;
        }
        let [lsb, msb] = color.to_le_bytes();
        let msb = msb | (ctx.force_set_mask_bit as u8) << 7;
        self.vram[address] = lsb;
        self.vram[address + 1] = msb;
    }

    pub fn fill_rect(&mut self, v: Vertex, side: Vertex, color: Colour) {
        let Vertex {
            x: width,
            y: height,
        } = side;
        let min_x = v.x.max(0) as usize;
        let min_y = v.y.max(0) as usize;
        let max_x = (v.x + width).min(0x400) as usize;
        let max_y = (v.y + height).min(0x200) as usize;

        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx2") {
            unsafe {
                self.fill_rect_avx2(min_x, min_y, max_x, max_y, color);
                return;
            }
        }

        panic!("not impl")
        // for x in min_x..max_x {
        //     for y in min_y..max_y {
        //         let vram_addr = 2 * (y * 1024 + x) as usize;
        //
        //         let [pixel_lsb, pixel_msb] = color.to_le_bytes();
        //         self.vram[vram_addr] = pixel_lsb;
        //         self.vram[vram_addr + 1] = pixel_msb & 0xEF;
        //     }
        // }
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    unsafe fn fill_rect_avx2(
        &mut self,
        min_x: usize,
        min_y: usize,
        max_x: usize,
        max_y: usize,
        color: Colour,
    ) {
        use std::arch::x86_64::_mm256_set1_epi16;
        use std::arch::x86_64::{__m256i, _mm256_storeu_si256};

        let color = color.to_u16();
        let color_vec = _mm256_set1_epi16(color as i16);
        let span_width = max_x - min_x;
        // println!("fill rect: {}", span_width);
        for y in min_y..max_y {
            let row_offset = y * 1024 + min_x;
            unsafe {
                let mut ptr = (self.vram.as_mut_ptr() as *mut u16).add(row_offset);
                let mut remaining = span_width;

                while remaining >= 16 {
                    _mm256_storeu_si256(ptr as *mut __m256i, color_vec);
                    ptr = ptr.add(16);
                    remaining -= 16;
                }

                while remaining > 0 {
                    *ptr = color;
                    ptr = ptr.add(1);
                    remaining -= 1;
                }
            }
        }
    }

    pub fn draw_line<const SEMI_TRANS: bool>(
        &mut self,
        mut v0: Vertex,
        mut v1: Vertex,
        mono: Colour,
        ctx: &RenderingContext,
    ) {
        v0.x += ctx.drawing_x_offset as i32;
        v0.y += ctx.drawing_y_offset as i32;
        v1.x += ctx.drawing_x_offset as i32;
        v1.y += ctx.drawing_y_offset as i32;

        let Some((x0, y0, x1, y1)) = self.clip_rect(v0.x, v0.y, v1.x, v1.y, ctx) else {
            return;
        };

        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();

        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };

        let mut err = dx + dy;
        let mut x = x0;
        let mut y = y0;

        loop {
            let mut pixel = mono;
            let vram_addr = 2 * (y * 1024 + x) as usize;

            if SEMI_TRANS {
                let background_lsb = self.vram[vram_addr];
                let background_msb = self.vram[vram_addr + 1];

                let background = Colour::from_bytes(background_msb, background_lsb);

                pixel.blend_with_background(background, ctx.semi_transparency);
            }
            pixel.apply_dithering(x, y);

            self.vram_write_color(vram_addr, pixel, ctx);
            // let [lsb, msb] = pixel.to_le_bytes();

            // self.vram[vram_addr] = lsb;
            // self.vram[vram_addr + 1] = msb;

            let e2 = 2 * err;
            if e2 >= dy {
                if x == x1 {
                    break;
                }
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                if y == y1 {
                    break;
                }
                err += dx;
                y += sy;
            }
        }
    }

    pub fn draw_line_shaded<const SEMI_TRANS: bool>(
        &mut self,
        mut v0: Vertex,
        mut v1: Vertex,
        color0: Colour,
        color1: Colour,
        ctx: &RenderingContext,
    ) {
        v0.x += ctx.drawing_x_offset as i32;
        v0.y += ctx.drawing_y_offset as i32;
        v1.x += ctx.drawing_x_offset as i32;
        v1.y += ctx.drawing_y_offset as i32;

        let Some((x0, y0, x1, y1)) = self.clip_rect(v0.x, v0.y, v1.x, v1.y, ctx) else {
            return;
        };

        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();

        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };

        let mut err = dx + dy;
        let mut x = x0;
        let mut y = y0;

        loop {
            let mut pixel = {
                let (num, denom) = if dx >= -dy {
                    ((x - x0).abs(), dx)
                } else {
                    ((y - y0).abs(), dy)
                };
                if denom == 0 {
                    color0
                } else {
                    let inv = denom - num;
                    let red = ((color0.r as i32) * inv + (color1.r as i32) * num) / denom;
                    let green = ((color0.g as i32) * inv + (color1.g as i32) * num) / denom;
                    let blue = ((color0.b as i32) * inv + (color1.b as i32) * num) / denom;
                    Colour {
                        r: red as u8,
                        g: green as u8,
                        b: blue as u8,
                        m: 0,
                    }
                }
            };
            let vram_addr = 2 * (y * 1024 + x) as usize;

            if SEMI_TRANS {
                let background_lsb = self.vram[vram_addr];
                let background_msb = self.vram[vram_addr + 1];

                let background = Colour::from_bytes(background_msb, background_lsb);

                pixel.blend_with_background(background, ctx.semi_transparency);
            }

            pixel.apply_dithering(x, y);

            self.vram_write_color(vram_addr, pixel, ctx);

            // let [lsb, msb] = pixel.to_le_bytes();

            // self.vram[vram_addr] = lsb;
            // self.vram[vram_addr + 1] = msb;

            let e2 = 2 * err;
            if e2 >= dy {
                if x == x1 {
                    break;
                }
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                if y == y1 {
                    break;
                }
                err += dx;
                y += sy;
            }
        }
    }
    pub fn draw_polygon_mono<const SEMI_TRANS: bool>(
        &mut self,
        cs: &mut [Colour; 4],
        vs: &mut [Vertex; 4],
        is_triangle: bool,
        ctx: &RenderingContext,
    ) {
        let mut vss = vs.clone();
        self.render_triangle_mono::<SEMI_TRANS>(cs[0], &mut vss[0..3], ctx);
        if !is_triangle {
            self.render_triangle_mono::<SEMI_TRANS>(cs[0], &mut vs[1..4], ctx);
        }
    }

    pub fn render_triangle_mono<const SEMI_TRANS: bool>(
        &mut self,
        mono: Colour,
        vs: &mut [Vertex],
        ctx: &RenderingContext,
    ) {
        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx2") {
            unsafe {
                self.render_triangle_mono_avx2::<SEMI_TRANS>(mono, vs, ctx);
                return;
            }
        }

        panic!("not impl")
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    pub fn render_triangle_mono_avx2<const SEMI_TRANS: bool>(
        &mut self,
        mono: Colour,
        vs: &mut [Vertex],
        ctx: &RenderingContext,
    ) {
        unsafe {
            // println!("render triangle mono");

            ensure_vertex_order(vs);
            vs[0].x += ctx.drawing_x_offset as i32;
            vs[0].y += ctx.drawing_y_offset as i32;
            vs[1].x += ctx.drawing_x_offset as i32;
            vs[1].y += ctx.drawing_y_offset as i32;
            vs[2].x += ctx.drawing_x_offset as i32;
            vs[2].y += ctx.drawing_y_offset as i32;

            let vram_ptr = self.vram.as_mut_ptr() as *mut u16;

            // let forced_mask_bit = i16::from(ctx.force_set_mask_bit) << 15;

            // let [pixel_lsb, pixel_msb] = color;
            let [v0, v1, v2] = vs else {
                unreachable!("");
            };

            let color_r = _mm256_set1_epi16(mono.r as i16);
            let color_g = _mm256_set1_epi16(mono.g as i16);
            let color_b = _mm256_set1_epi16(mono.b as i16);
            // let color = mono.to_u16();

            // bounding box
            let min_x = cmp::min(v0.x, cmp::min(v1.x, v2.x));
            let max_x = cmp::max(v0.x, cmp::max(v1.x, v2.x));
            let min_y = cmp::min(v0.y, cmp::min(v1.y, v2.y));
            let max_y = cmp::max(v0.y, cmp::max(v1.y, v2.y));

            let Some((min_x, min_y, max_x, max_y)) =
                self.clip_rect(min_x, min_y, max_x, max_y, ctx)
            else {
                return;
            };
            let min_x_aligned = min_x & !0xF;
            let max_x_aligned = (max_x + 15) & !0xF;

            // println!("minx_aligned: {}, max_x_aligned: {}", min_x_aligned, max_x_aligned);

            let x_bounds = (min_x, max_x);

            let v01_is_not_bottom_right = is_not_bottom_right_edge(vs[0], vs[1]);
            let v12_is_not_bottom_right = is_not_bottom_right_edge(vs[1], vs[2]);
            let v20_is_not_bottom_right = is_not_bottom_right_edge(vs[2], vs[0]);

            for y in min_y..=max_y {
                let py = _mm256_set1_epi32(y);

                for x in (min_x_aligned..=max_x_aligned).step_by(16) {
                    // Determine which X coordinates are inside the triangle.
                    // The 16 X coordinates are split up such that vectors can later be converted from
                    // two i32x8 vectors to a single i16x16 vector using _mm256_packs_epi32

                    let px1 =
                        _mm256_setr_epi32(x, x + 1, x + 2, x + 3, x + 8, x + 9, x + 10, x + 11);
                    let inside_mask_1 = compute_write_mask(
                        vs,
                        px1,
                        py,
                        x_bounds,
                        v01_is_not_bottom_right,
                        v12_is_not_bottom_right,
                        v20_is_not_bottom_right,
                    );

                    let px2 = _mm256_setr_epi32(
                        x + 4,
                        x + 5,
                        x + 6,
                        x + 7,
                        x + 12,
                        x + 13,
                        x + 14,
                        x + 15,
                    );
                    let inside_mask_2 = compute_write_mask(
                        vs,
                        px2,
                        py,
                        x_bounds,
                        v01_is_not_bottom_right,
                        v12_is_not_bottom_right,
                        v20_is_not_bottom_right,
                    );

                    let mut inside_mask = _mm256_packs_epi32(inside_mask_1, inside_mask_2);

                    // If no points are inside the triangle, bail out early
                    if _mm256_testz_si256(inside_mask, _mm256_set1_epi16(!0)) != 0 {
                        continue;
                    }

                    let row_offset = y as usize * 1024 + x as usize;
                    let vram_addr = vram_ptr.add(row_offset).cast::<__m256i>();

                    let existing = _mm256_load_si256(vram_addr);

                    let mut r = color_r;
                    let mut g = color_g;
                    let mut b = color_b;

                    let mask_bits = _mm256_set1_epi16((ctx.force_set_mask_bit as i16) << 15);
                    let semi_transparency_bits = _mm256_set1_epi16(1 << 15);

                    // Truncate color from from 8-bit RGB components to 5-bit
                    r = _mm256_srli_epi16::<3>(r);
                    g = _mm256_srli_epi16::<3>(g);
                    b = _mm256_srli_epi16::<3>(b);

                    // If semi-transparency is enabled, blend existing colors with new colors
                    if SEMI_TRANS {
                        let (existing_r, existing_g, existing_b) = split_15bit_color(existing);
                        let semi_transparency_mask =
                            _mm256_cmpeq_epi16(semi_transparency_bits, _mm256_setzero_si256());

                        (r, g, b) = apply_semi_transparency(
                            (existing_r, existing_g, existing_b),
                            (r, g, b),
                            semi_transparency_mask,
                            ctx.semi_transparency,
                        );
                    }
                    let x = x as i16;
                    let px = _mm256_setr_epi16(
                        x,
                        x + 1,
                        x + 2,
                        x + 3,
                        x + 4,
                        x + 5,
                        x + 6,
                        x + 7,
                        x + 8,
                        x + 9,
                        x + 10,
                        x + 11,
                        x + 12,
                        x + 13,
                        x + 14,
                        x + 15,
                    );

                    // mask out padding pixels
                    let mut write_mask = _mm256_and_si256(
                        inside_mask,
                        _mm256_andnot_si256(
                            _mm256_cmpgt_epi16(_mm256_set1_epi16(min_x as i16 - 1), px),
                            _mm256_cmpgt_epi16(_mm256_set1_epi16((max_x + 1) as i16), px),
                        ),
                    );
                    if ctx.preserve_masked_pixels {
                        write_mask = _mm256_and_si256(
                            write_mask,
                            _mm256_cmpeq_epi16(
                                _mm256_and_si256(existing, _mm256_set1_epi16(1 << 15)),
                                _mm256_setzero_si256(),
                            ),
                        )
                    };
                    let color = _mm256_or_si256(combine_rgb_components(r, g, b), mask_bits);
                    _mm256_storeu_si256(
                        vram_addr,
                        _mm256_or_si256(
                            _mm256_and_si256(write_mask, color),
                            _mm256_andnot_si256(write_mask, existing),
                        ),
                    );
                    // _mm256_storeu_si256(
                    //     vram_addr,
                    //          color
                    // );
                }
            }
        }
    }

    pub fn draw_polygon_shaded<const SEMI_TRANS: bool>(
        &mut self,
        vs: &mut [Vertex; 4],
        colors: &mut [Colour; 4],
        is_triangle: bool,
        ctx: &RenderingContext,
    ) {
        let mut vss = vs.clone();
        let mut css = colors.clone();
        self.draw_triangle_shaded::<SEMI_TRANS>(&mut vss[0..3], &mut css[0..3], ctx);
        if !is_triangle {
            self.draw_triangle_shaded::<SEMI_TRANS>(&mut vs[1..4], &mut colors[1..4], ctx);
        }
    }
    pub fn draw_triangle_shaded<const SEMI_TRANS: bool>(
        &mut self,
        vs: &mut [Vertex],
        colors: &mut [Colour],
        ctx: &RenderingContext,
    ) {
        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx2") {
            unsafe {
                self.render_triangle_shaded_avx2::<SEMI_TRANS>(vs, colors, ctx);
                return;
            }
        }

        panic!("not impl")

        // for y in min_y..max_y {
        //     for x in min_x..max_x {
        //         let p = Vertex { x, y };
        //         if is_inside_triangle(p, *v0, *v1, *v2) {
        //             let lambda = compute_barycentric_coordinates(p, *v0, *v1, *v2);
        //             let color = interpolate_color(&lambda, &[*c0, *c1, *c2]);
        //             let mut color = apply_dithering(color, p);
        //             let vram_addr = 2 * (y * 1024 + x) as usize;
        //
        //             if SEMI_TRANS {
        //                 let background_lsb = self.vram[vram_addr];
        //                 let background_msb = self.vram[vram_addr + 1];
        //
        //                 let bg = Colour::from_bytes(background_msb, background_lsb);
        //
        //                 color.blend_with_background(bg, ctx.semi_transparency);
        //             }
        //
        //             self.vram_write_color(vram_addr, color, ctx);
        //         }
        //     }
        // }
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    pub fn render_triangle_shaded_avx2<const SEMI_TRANS: bool>(
        &mut self,
        vs: &mut [Vertex],
        colors: &mut [Colour],
        ctx: &RenderingContext,
    ) {
        unsafe {
            println!("render triangle shaded");

            ensure_vertex_order2(vs, colors);
            vs[0].x += ctx.drawing_x_offset as i32;
            vs[0].y += ctx.drawing_y_offset as i32;
            vs[1].x += ctx.drawing_x_offset as i32;
            vs[1].y += ctx.drawing_y_offset as i32;
            vs[2].x += ctx.drawing_x_offset as i32;
            vs[2].y += ctx.drawing_y_offset as i32;

            let vram_ptr = self.vram.as_mut_ptr() as *mut u16;

            // let forced_mask_bit = i16::from(ctx.force_set_mask_bit) << 15;

            // let [pixel_lsb, pixel_msb] = color;
            let [v0, v1, v2] = vs else {
                unreachable!("");
            };


            // bounding box
            let min_x = cmp::min(v0.x, cmp::min(v1.x, v2.x));
            let max_x = cmp::max(v0.x, cmp::max(v1.x, v2.x));
            let min_y = cmp::min(v0.y, cmp::min(v1.y, v2.y));
            let max_y = cmp::max(v0.y, cmp::max(v1.y, v2.y));

            let Some((min_x, min_y, max_x, max_y)) =
                self.clip_rect(min_x, min_y, max_x, max_y, ctx)
            else {
                return;
            };
            let min_x_aligned = min_x & !0xF;
            let max_x_aligned = (max_x + 15) & !0xF;

            let x_bounds = (min_x, max_x);

            let interpolation_denominator = cross_product_z(vs[0], vs[1], vs[2]);

            if interpolation_denominator == 0 {
                // colinear points
                return;
            }

            let v01_is_not_bottom_right = is_not_bottom_right_edge(vs[0], vs[1]);
            let v12_is_not_bottom_right = is_not_bottom_right_edge(vs[1], vs[2]);
            let v20_is_not_bottom_right = is_not_bottom_right_edge(vs[2], vs[0]);

            let interpolator = Interpolator::new(vs, interpolation_denominator, true, colors, None);
            for y in min_y..=max_y {
                let py = _mm256_set1_epi32(y);

                for x in (min_x_aligned..=max_x_aligned).step_by(16) {
                    // Determine which X coordinates are inside the triangle.
                    // The 16 X coordinates are split up such that vectors can later be converted from
                    // two i32x8 vectors to a single i16x16 vector using _mm256_packs_epi32

                    let px1 =
                        _mm256_setr_epi32(x, x + 1, x + 2, x + 3, x + 8, x + 9, x + 10, x + 11);
                    let inside_mask_1 = compute_write_mask(
                        vs,
                        px1,
                        py,
                        x_bounds,
                        v01_is_not_bottom_right,
                        v12_is_not_bottom_right,
                        v20_is_not_bottom_right,
                    );

                    let px2 = _mm256_setr_epi32(
                        x + 4,
                        x + 5,
                        x + 6,
                        x + 7,
                        x + 12,
                        x + 13,
                        x + 14,
                        x + 15,
                    );
                    let inside_mask_2 = compute_write_mask(
                        vs,
                        px2,
                        py,
                        x_bounds,
                        v01_is_not_bottom_right,
                        v12_is_not_bottom_right,
                        v20_is_not_bottom_right,
                    );

                    let inside_mask = _mm256_packs_epi32(inside_mask_1, inside_mask_2);

                    // If no points are inside the triangle, bail out early
                    if _mm256_testz_si256(inside_mask, _mm256_set1_epi16(!0)) != 0 {
                        continue;
                    }

                    let row_offset = y as usize * 1024 + x as usize;
                    let vram_addr = vram_ptr.add(row_offset).cast::<__m256i>();

                    let existing = _mm256_load_si256(vram_addr);

                    let (r1, g1, b1) = interpolator.interpolate_color(px1, py);
                    let (r2, g2, b2) = interpolator.interpolate_color(px2, py);
                    let mut r = _mm256_packs_epi32(r1, r2);
                    let mut g = _mm256_packs_epi32(g1, g2);
                    let mut b = _mm256_packs_epi32(b1, b2);

                    let mask_bits = _mm256_set1_epi16((ctx.force_set_mask_bit as i16) << 15);
                    let semi_transparency_bits = _mm256_set1_epi16(1 << 15);

                    // Truncate color from from 8-bit RGB components to 5-bit
                    r = _mm256_srli_epi16::<3>(r);
                    g = _mm256_srli_epi16::<3>(g);
                    b = _mm256_srli_epi16::<3>(b);

                    // If semi-transparency is enabled, blend existing colors with new colors
                    if SEMI_TRANS {
                        let (existing_r, existing_g, existing_b) = split_15bit_color(existing);
                        let semi_transparency_mask =
                            _mm256_cmpeq_epi16(semi_transparency_bits, _mm256_setzero_si256());

                        (r, g, b) = apply_semi_transparency(
                            (existing_r, existing_g, existing_b),
                            (r, g, b),
                            semi_transparency_mask,
                            ctx.semi_transparency,
                        );
                    }
                    let x = x as i16;
                    let px = _mm256_setr_epi16(
                        x,
                        x + 1,
                        x + 2,
                        x + 3,
                        x + 4,
                        x + 5,
                        x + 6,
                        x + 7,
                        x + 8,
                        x + 9,
                        x + 10,
                        x + 11,
                        x + 12,
                        x + 13,
                        x + 14,
                        x + 15,
                    );

                    // mask out padding pixels
                    let mut write_mask = _mm256_and_si256(
                        inside_mask,
                        _mm256_andnot_si256(
                            _mm256_cmpgt_epi16(_mm256_set1_epi16(min_x as i16-1), px),
                            _mm256_cmpgt_epi16(_mm256_set1_epi16((max_x + 1) as i16), px),
                        ),
                    );
                    if ctx.preserve_masked_pixels {
                        write_mask = _mm256_and_si256(
                            write_mask,
                            _mm256_cmpeq_epi16(
                                _mm256_and_si256(existing, _mm256_set1_epi16(1 << 15)),
                                _mm256_setzero_si256(),
                            ),
                        )
                    };
                    let color = _mm256_or_si256(combine_rgb_components(r, g, b), mask_bits);
                    _mm256_storeu_si256(
                        vram_addr,
                        _mm256_or_si256(
                            _mm256_and_si256(write_mask, color),
                            _mm256_andnot_si256(write_mask, existing),
                        ),
                    );
                    // _mm256_storeu_si256(
                    //     vram_addr,
                    //          color
                    // );
                }
            }
        }
    }

    /* GP0(E1h)
      0-3   Texture page X Base   (N*64) (ie. in 64-halfword steps)    ;GPUSTAT.0-3
      4     Texture page Y Base 1 (N*256) (ie. 0, 256, 512 or 768)     ;GPUSTAT.4
      5-6   Semi-transparency     (0=B/2+F/2, 1=B+F, 2=B-F, 3=B+F/4)   ;GPUSTAT.5-6
      7-8   Texture page colors   (0=4bit, 1=8bit, 2=15bit, 3=Reserved);GPUSTAT.7-8
      9     Dither 24bit to 15bit (0=Off/strip LSBs, 1=Dither Enabled) ;GPUSTAT.9
      10    Drawing to display area (0=Prohibited, 1=Allowed)          ;GPUSTAT.10
      11    Texture page Y Base 2 (N*512) (only for 2 MB VRAM)         ;GPUSTAT.15
      12    Textured Rectangle X-Flip   (BIOS does set this bit on power-up...?)
      13    Textured Rectangle Y-Flip   (BIOS does set it equal to GPUSTAT.13...?)
      14-23 Not used (should be 0)
      24-31 Command  (E1h)
    */
    pub fn compute_texel_offset(&self, x: usize, y: usize, ctx: &RenderingContext) -> [usize; 2] {
        let x_mask = ctx.texture_window_x_mask as usize;
        let y_mask = ctx.texture_window_y_mask as usize;
        let x_offset = ctx.texture_window_x_offset as usize;
        let y_offset = ctx.texture_window_y_offset as usize;

        [
            (x & (!(x_mask * 8))) | ((x_offset & x_mask) * 8),
            (y & (!(y_mask * 8))) | ((y_offset & y_mask) * 8),
        ]
    }

    pub fn draw_polygon_textured<const SEMI_TRANS: bool, const BLEND: bool>(
        &mut self,
        cs: &mut [Colour; 4],
        clut: u16,
        page: u16,
        vs: &mut [Vertex; 4],
        uv: &mut [[u16; 2]; 4],
        is_triangle: bool,
        ctx: &RenderingContext,
    ) {
        let mut vss = vs.clone();
        let mut uvs = uv.clone();
        self.draw_triangle_textured::<SEMI_TRANS, BLEND>(
            cs[0],
            clut,
            page,
            &mut vss[0..3],
            &mut uvs[0..3],
            ctx,
        );
        if !is_triangle {
            self.draw_triangle_textured::<SEMI_TRANS, BLEND>(
                cs[0],
                clut,
                page,
                &mut vs[1..4],
                &mut uv[1..4],
                ctx,
            );
        }
    }
    pub fn draw_triangle_textured<const SEMI_TRANS: bool, const BLEND: bool>(
        &mut self,
        mono: Colour,
        clut: u16,
        page: u16,
        vs: &mut [Vertex],
        uv: &mut [[u16; 2]],
        ctx: &RenderingContext,
    ) {
        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx2") {
            unsafe {
                self.render_triangle_textured_avx2::<SEMI_TRANS, BLEND>(
                    mono, clut, page, vs, uv, ctx,
                );
                return;
            }
        }

        panic!("not impl")

        // let clut = Clut::new(clut);
        // let texture = Texture::new(page, clut);

        // for y in min_y..max_y {
        //     for x in min_x..max_x {
        //         let p = Vertex { x, y };
        //         if is_inside_triangle(p, vs[0], vs[1], vs[2]) {
        //             let lambda = compute_barycentric_coordinates(p, vs[0], vs[1], vs[2]);
        //             let [uv_x, uv_y] = compute_normal_coordinates(lambda, uv);
        //
        //             let mut pixel = texture.get_texel(self, uv_x, uv_y, ctx);
        //
        //             if pixel.is_black() {
        //                 continue;
        //             }
        //
        //             if BLEND {
        //                 pixel.blend(mono);
        //             }
        //
        //             let vram_addr = 2 * (y * 1024 + x) as usize;
        //
        //             if SEMI_TRANS && pixel.m == 1 {
        //                 let background_lsb = self.vram[vram_addr];
        //                 let background_msb = self.vram[vram_addr + 1];
        //
        //                 let background = Colour::from_bytes(background_msb, background_lsb);
        //
        //                 pixel.blend_with_background(background, texture.semi_transparency);
        //             }
        //
        //             if texture.dithering {
        //                 pixel.apply_dithering(x, y);
        //             }
        //
        //             self.vram_write_color(vram_addr, pixel, ctx);
        //             // let [lsb, msb] = pixel.to_le_bytes();
        //
        //             // self.vram[vram_addr] = lsb;
        //             // self.vram[vram_addr + 1] = msb;
        //         }
        //     }
        // }

        // let [pixel_lsb, pixel_msb] = Self::gp0_color(self.gp0_command[0]);
    }
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    pub fn render_triangle_textured_avx2<const SEMI_TRANS: bool, const BLEND: bool>(
        &mut self,
        mono: Colour,
        clut: u16,
        page: u16,
        vs: &mut [Vertex],
        uv: &mut [[u16; 2]],
        ctx: &RenderingContext,
    ) {
        unsafe {
            // println!("render triangle textured");

            ensure_vertex_order2(vs, uv);
            vs[0].x += ctx.drawing_x_offset as i32;
            vs[0].y += ctx.drawing_y_offset as i32;
            vs[1].x += ctx.drawing_x_offset as i32;
            vs[1].y += ctx.drawing_y_offset as i32;
            vs[2].x += ctx.drawing_x_offset as i32;
            vs[2].y += ctx.drawing_y_offset as i32;

            let vram_ptr = self.vram.as_mut_ptr() as *mut u16;

            // let forced_mask_bit = i16::from(ctx.force_set_mask_bit) << 15;

            // let [pixel_lsb, pixel_msb] = color;
            let [v0, v1, v2] = vs else {
                unreachable!("");
            };
            let [uv0, uv1, uv2] = uv else {
                panic!("boo");
            };

            let colors = [mono, mono, mono];

            // let color_r = _mm256_set1_epi16(mono.r as i16);
            // let color_g = _mm256_set1_epi16(mono.g as i16);
            // let color_b = _mm256_set1_epi16(mono.b as i16);
            // let color = mono.to_u16();

            // bounding box
            let min_x = cmp::min(v0.x, cmp::min(v1.x, v2.x));
            let max_x = cmp::max(v0.x, cmp::max(v1.x, v2.x));
            let min_y = cmp::min(v0.y, cmp::min(v1.y, v2.y));
            let max_y = cmp::max(v0.y, cmp::max(v1.y, v2.y));

            let Some((min_x, min_y, max_x, max_y)) =
                self.clip_rect(min_x, min_y, max_x, max_y, ctx)
            else {
                return;
            };
            let min_x_aligned = min_x & !0xF;
            let max_x_aligned = (max_x + 15) & !0xF;

            let x_bounds = (min_x, max_x);

            let clut = Clut::new(clut);
            let texture = Texture::new_avx2(page, clut);

            let interpolation_denominator = cross_product_z(vs[0], vs[1], vs[2]);

            if interpolation_denominator == 0 {
                // colinear points
                return;
            }

            let v01_is_not_bottom_right = is_not_bottom_right_edge(vs[0], vs[1]);
            let v12_is_not_bottom_right = is_not_bottom_right_edge(vs[1], vs[2]);
            let v20_is_not_bottom_right = is_not_bottom_right_edge(vs[2], vs[0]);

            let interpolator =
                Interpolator::new(vs, interpolation_denominator, false, &colors, Some(uv));
            for y in min_y..=max_y {
                let py = _mm256_set1_epi32(y);

                for x in (min_x_aligned..=max_x_aligned).step_by(16) {
                    // Determine which X coordinates are inside the triangle.
                    // The 16 X coordinates are split up such that vectors can later be converted from
                    // two i32x8 vectors to a single i16x16 vector using _mm256_packs_epi32

                    let px1 =
                        _mm256_setr_epi32(x, x + 1, x + 2, x + 3, x + 8, x + 9, x + 10, x + 11);
                    let inside_mask_1 = compute_write_mask(
                        vs,
                        px1,
                        py,
                        x_bounds,
                        v01_is_not_bottom_right,
                        v12_is_not_bottom_right,
                        v20_is_not_bottom_right,
                    );

                    let px2 = _mm256_setr_epi32(
                        x + 4,
                        x + 5,
                        x + 6,
                        x + 7,
                        x + 12,
                        x + 13,
                        x + 14,
                        x + 15,
                    );
                    let inside_mask_2 = compute_write_mask(
                        vs,
                        px2,
                        py,
                        x_bounds,
                        v01_is_not_bottom_right,
                        v12_is_not_bottom_right,
                        v20_is_not_bottom_right,
                    );

                    let mut inside_mask = _mm256_packs_epi32(inside_mask_1, inside_mask_2);

                    // If no points are inside the triangle, bail out early
                    if _mm256_testz_si256(inside_mask, _mm256_set1_epi16(!0)) != 0 {
                        continue;
                    }

                    let row_offset = y as usize * 1024 + x as usize;
                    let vram_addr = vram_ptr.add(row_offset).cast::<__m256i>();

                    let existing = _mm256_load_si256(vram_addr);

                    // let (r1, g1, b1) = interpolator.interpolate_color(px1, py);
                    // let (r2, g2, b2) = interpolator.interpolate_color(px2, py);
                    let mut r = _mm256_set1_epi16(mono.r.into());
                    let mut g = _mm256_set1_epi16(mono.g.into());
                    let mut b = _mm256_set1_epi16(mono.b.into());

                    let mut mask_bits = _mm256_set1_epi16((ctx.force_set_mask_bit as i16) << 15);
                    // let semi_transparency_bits = _mm256_set1_epi16(1 << 15);

                    // Interpolate U/V coordinates
                    let (u1, v1) = interpolator.interpolate_uv(px1, py);
                    let (u2, v2) = interpolator.interpolate_uv(px2, py);
                    let (u, v) = (_mm256_packus_epi32(u1, u2), _mm256_packus_epi32(v1, v2));

                    // Read 16 texels from the texture
                    let texels = read_texture(vram_ptr, texture, u, v, ctx);

                    // Mask out any pixels where the texel value is $0000
                    inside_mask = _mm256_andnot_si256(
                        _mm256_cmpeq_epi16(texels, _mm256_setzero_si256()),
                        inside_mask,
                    );

                    // Texels are semi-transparent only if bit 15 is set
                    let semi_transparency_bits =
                        _mm256_and_si256(texels, _mm256_set1_epi16(1 << 15));
                    mask_bits = _mm256_or_si256(mask_bits, semi_transparency_bits);

                    let (tr, tg, tb) = convert_15bit_to_24bit(texels);

                    if BLEND {
                        r = modulate_texture_color(tr, r);
                        g = modulate_texture_color(tg, g);
                        b = modulate_texture_color(tb, b);
                    } else {
                        r = tr;
                        g = tg;
                        b = tb;
                    }

                    if texture.dithering {
                        let dither_vector: __m256i = mem::transmute(VECTOR_DITHER_TABLE[(y & 3) as usize]);

                        let u8_max = _mm256_set1_epi16(255);
                        r = _mm256_min_epi16(
                            u8_max,
                            _mm256_max_epi16(
                                _mm256_setzero_si256(),
                                _mm256_add_epi16(r, dither_vector),
                            ),
                        );
                        g = _mm256_min_epi16(
                            u8_max,
                            _mm256_max_epi16(
                                _mm256_setzero_si256(),
                                _mm256_add_epi16(g, dither_vector),
                            ),
                        );
                        b = _mm256_min_epi16(
                            u8_max,
                            _mm256_max_epi16(
                                _mm256_setzero_si256(),
                                _mm256_add_epi16(b, dither_vector),
                            ),
                        );
                    }

                    // Truncate color from from 8-bit RGB components to 5-bit
                    r = _mm256_srli_epi16::<3>(r);
                    g = _mm256_srli_epi16::<3>(g);
                    b = _mm256_srli_epi16::<3>(b);

                    // If semi-transparency is enabled, blend existing colors with new colors
                    if SEMI_TRANS {
                        let (existing_r, existing_g, existing_b) = split_15bit_color(existing);
                        let semi_transparency_mask =
                            _mm256_cmpeq_epi16(semi_transparency_bits, _mm256_setzero_si256());

                        (r, g, b) = apply_semi_transparency(
                            (existing_r, existing_g, existing_b),
                            (r, g, b),
                            semi_transparency_mask,
                            ctx.semi_transparency,
                        );
                    }
                    let x = x as i16;
                    let px = _mm256_setr_epi16(
                        x,
                        x + 1,
                        x + 2,
                        x + 3,
                        x + 4,
                        x + 5,
                        x + 6,
                        x + 7,
                        x + 8,
                        x + 9,
                        x + 10,
                        x + 11,
                        x + 12,
                        x + 13,
                        x + 14,
                        x + 15,
                    );

                    // mask out padding pixels
                    let mut write_mask = _mm256_and_si256(
                        inside_mask,
                        _mm256_andnot_si256(
                            _mm256_cmpgt_epi16(_mm256_set1_epi16(min_x as i16 - 1), px),
                            _mm256_cmpgt_epi16(_mm256_set1_epi16((max_x + 1) as i16), px),
                        ),
                    );
                    if ctx.preserve_masked_pixels {
                        write_mask = _mm256_and_si256(
                            write_mask,
                            _mm256_cmpeq_epi16(
                                _mm256_and_si256(existing, _mm256_set1_epi16(1 << 15)),
                                _mm256_setzero_si256(),
                            ),
                        )
                    };
                    let color = _mm256_or_si256(combine_rgb_components(r, g, b), mask_bits);
                    _mm256_storeu_si256(
                        vram_addr,
                        _mm256_or_si256(
                            _mm256_and_si256(write_mask, color),
                            _mm256_andnot_si256(write_mask, existing),
                        ),
                    );
                    // _mm256_storeu_si256(
                    //     vram_addr,
                    //          color
                    // );
                }
            }
        }
    }

    pub fn draw_polygon_textured_shaded<const SEMI_TRANS: bool, const BLEND: bool>(
        &mut self,
        colors: &mut [Colour; 4],
        clut: u16,
        page: u16,
        vs: &mut [Vertex; 4],
        uv: &mut [[u16; 2]; 4],
        is_triangle: bool,
        ctx: &RenderingContext,
    ) {
        let mut vss = vs.clone();
        let mut uvs = uv.clone();
        let mut css = colors.clone();
        self.draw_triangle_textured_shaded::<SEMI_TRANS, BLEND>(
            &mut css[0..3],
            clut,
            page,
            &mut vss[0..3],
            &mut uvs[0..3],
            ctx,
        );
        if !is_triangle {
            self.draw_triangle_textured_shaded::<SEMI_TRANS, BLEND>(
                &mut colors[1..4],
                clut,
                page,
                &mut vs[1..4],
                &mut uv[1..4],
                ctx,
            );
        }
    }
    pub fn draw_triangle_textured_shaded<const SEMI_TRANS: bool, const BLEND: bool>(
        &mut self,
        colors: &mut [Colour],
        clut: u16,
        page: u16,
        vs: &mut [Vertex],
        uv: &mut [[u16; 2]],
        ctx: &RenderingContext,
    ) {
        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx2") {
            unsafe {
                self.render_triangle_textured_shaded_avx2::<SEMI_TRANS, BLEND>(
                    colors, clut, page, vs, uv, ctx,
                );
                return;
            }
        }

        panic!("not impl");




        //
        // for y in min_y..max_y {
        //     for x in min_x..max_x {
        //         let p = Vertex { x, y };
        //         if is_inside_triangle(p, vs[0], vs[1], vs[2]) {
        //             let lambda = compute_barycentric_coordinates(p, vs[0], vs[1], vs[2]);
        //             let [uv_x, uv_y] = compute_normal_coordinates(lambda, uv);
        //
        //             let mut pixel = texture.get_texel(self, uv_x, uv_y, ctx);
        //
        //             if pixel.is_black() {
        //                 continue;
        //             }
        //
        //             if BLEND {
        //                 let color = interpolate_color(&lambda, colors);
        //                 pixel.blend(color);
        //             }
        //
        //             let vram_addr = 2 * (y * 1024 + x) as usize;
        //
        //             if SEMI_TRANS && pixel.m == 1 {
        //                 let background_lsb = self.vram[vram_addr];
        //                 let background_msb = self.vram[vram_addr + 1];
        //
        //                 let background = Colour::from_bytes(background_msb, background_lsb);
        //
        //                 pixel.blend_with_background(background, texture.semi_transparency);
        //             }
        //
        //             if texture.dithering {
        //                 pixel = apply_dithering(pixel, p);
        //             }
        //
        //             self.vram_write_color(vram_addr, pixel, ctx);
        //             // let [lsb, msb] = pixel.to_le_bytes();
        //             //
        //             // self.vram[vram_addr] = lsb;
        //             // self.vram[vram_addr + 1] = msb;
        //         }
        //     }
        // }

        // let [pixel_lsb, pixel_msb] = Self::gp0_color(self.gp0_command[0]);
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    pub fn render_triangle_textured_shaded_avx2<const SEMI_TRANS: bool, const BLEND: bool>(
        &mut self,
        colors: &mut [Colour],
        clut: u16,
        page: u16,
        vs: &mut [Vertex],
        uv: &mut [[u16; 2]],
        ctx: &RenderingContext,
    ) {
        unsafe {
            // println!("render triangle textured shaded");

            ensure_vertex_order3(vs, uv, colors);
            vs[0].x += ctx.drawing_x_offset as i32;
            vs[0].y += ctx.drawing_y_offset as i32;
            vs[1].x += ctx.drawing_x_offset as i32;
            vs[1].y += ctx.drawing_y_offset as i32;
            vs[2].x += ctx.drawing_x_offset as i32;
            vs[2].y += ctx.drawing_y_offset as i32;

            let vram_ptr = self.vram.as_mut_ptr() as *mut u16;

            // let forced_mask_bit = i16::from(ctx.force_set_mask_bit) << 15;

            // let [pixel_lsb, pixel_msb] = color;
            let [v0, v1, v2] = vs else {
                unreachable!("");
            };
            let [uv0, uv1, uv2] = uv else {
                panic!("boo");
            };

            // let colors = [mono, mono, mono];

            // let color_r = _mm256_set1_epi16(mono.r as i16);
            // let color_g = _mm256_set1_epi16(mono.g as i16);
            // let color_b = _mm256_set1_epi16(mono.b as i16);
            // let color = mono.to_u16();

            // bounding box
            let min_x = cmp::min(v0.x, cmp::min(v1.x, v2.x));
            let max_x = cmp::max(v0.x, cmp::max(v1.x, v2.x));
            let min_y = cmp::min(v0.y, cmp::min(v1.y, v2.y));
            let max_y = cmp::max(v0.y, cmp::max(v1.y, v2.y));

            let Some((min_x, min_y, max_x, max_y)) =
                self.clip_rect(min_x, min_y, max_x, max_y, ctx)
            else {
                return;
            };
            let min_x_aligned = min_x & !0xF;
            let max_x_aligned = (max_x + 15) & !0xF;

            let x_bounds = (min_x, max_x);

            let clut = Clut::new(clut);
            let texture = Texture::new_avx2(page, clut);

            let interpolation_denominator = cross_product_z(vs[0], vs[1], vs[2]);

            if interpolation_denominator == 0 {
                // colinear points
                return;
            }

            let v01_is_not_bottom_right = is_not_bottom_right_edge(vs[0], vs[1]);
            let v12_is_not_bottom_right = is_not_bottom_right_edge(vs[1], vs[2]);
            let v20_is_not_bottom_right = is_not_bottom_right_edge(vs[2], vs[0]);

            let interpolator =
                Interpolator::new(vs, interpolation_denominator, false, &colors, Some(uv));
            for y in min_y..=max_y {
                let py = _mm256_set1_epi32(y);

                for x in (min_x_aligned..=max_x_aligned).step_by(16) {
                    // Determine which X coordinates are inside the triangle.
                    // The 16 X coordinates are split up such that vectors can later be converted from
                    // two i32x8 vectors to a single i16x16 vector using _mm256_packs_epi32

                    let px1 =
                        _mm256_setr_epi32(x, x + 1, x + 2, x + 3, x + 8, x + 9, x + 10, x + 11);
                    let inside_mask_1 = compute_write_mask(
                        vs,
                        px1,
                        py,
                        x_bounds,
                        v01_is_not_bottom_right,
                        v12_is_not_bottom_right,
                        v20_is_not_bottom_right,
                    );

                    let px2 = _mm256_setr_epi32(
                        x + 4,
                        x + 5,
                        x + 6,
                        x + 7,
                        x + 12,
                        x + 13,
                        x + 14,
                        x + 15,
                    );
                    let inside_mask_2 = compute_write_mask(
                        vs,
                        px2,
                        py,
                        x_bounds,
                        v01_is_not_bottom_right,
                        v12_is_not_bottom_right,
                        v20_is_not_bottom_right,
                    );

                    let mut inside_mask = _mm256_packs_epi32(inside_mask_1, inside_mask_2);

                    // If no points are inside the triangle, bail out early
                    if _mm256_testz_si256(inside_mask, _mm256_set1_epi16(!0)) != 0 {
                        continue;
                    }

                    let row_offset = y as usize * 1024 + x as usize;
                    let vram_addr = vram_ptr.add(row_offset).cast::<__m256i>();

                    let existing = _mm256_load_si256(vram_addr);

                    let (r1, g1, b1) = interpolator.interpolate_color(px1, py);
                    let (r2, g2, b2) = interpolator.interpolate_color(px2, py);
                    let mut r = _mm256_packs_epi32(r1, r2);
                    let mut g = _mm256_packs_epi32(g1, g2);
                    let mut b = _mm256_packs_epi32(b1, b2);

                    let mut mask_bits = _mm256_set1_epi16((ctx.force_set_mask_bit as i16) << 15);
                    // let semi_transparency_bits = _mm256_set1_epi16(1 << 15);

                    // Interpolate U/V coordinates
                    let (u1, v1) = interpolator.interpolate_uv(px1, py);
                    let (u2, v2) = interpolator.interpolate_uv(px2, py);
                    let (u, v) = (_mm256_packus_epi32(u1, u2), _mm256_packus_epi32(v1, v2));

                    // Read 16 texels from the texture
                    let texels = read_texture(vram_ptr, texture, u, v, ctx);

                    // Mask out any pixels where the texel value is $0000
                    inside_mask = _mm256_andnot_si256(
                        _mm256_cmpeq_epi16(texels, _mm256_setzero_si256()),
                        inside_mask,
                    );

                    // Texels are semi-transparent only if bit 15 is set
                    let semi_transparency_bits =
                        _mm256_and_si256(texels, _mm256_set1_epi16(1 << 15));
                    mask_bits = _mm256_or_si256(mask_bits, semi_transparency_bits);

                    let (tr, tg, tb) = convert_15bit_to_24bit(texels);

                    if BLEND {
                        r = modulate_texture_color(tr, r);
                        g = modulate_texture_color(tg, g);
                        b = modulate_texture_color(tb, b);
                    } else {
                        r = tr;
                        g = tg;
                        b = tb;
                    }

                    if texture.dithering {
                        let dither_vector: __m256i = mem::transmute(VECTOR_DITHER_TABLE[(y & 3) as usize]);

                        let u8_max = _mm256_set1_epi16(255);
                        r = _mm256_min_epi16(
                            u8_max,
                            _mm256_max_epi16(
                                _mm256_setzero_si256(),
                                _mm256_add_epi16(r, dither_vector),
                            ),
                        );
                        g = _mm256_min_epi16(
                            u8_max,
                            _mm256_max_epi16(
                                _mm256_setzero_si256(),
                                _mm256_add_epi16(g, dither_vector),
                            ),
                        );
                        b = _mm256_min_epi16(
                            u8_max,
                            _mm256_max_epi16(
                                _mm256_setzero_si256(),
                                _mm256_add_epi16(b, dither_vector),
                            ),
                        );
                    }

                    // Truncate color from from 8-bit RGB components to 5-bit
                    r = _mm256_srli_epi16::<3>(r);
                    g = _mm256_srli_epi16::<3>(g);
                    b = _mm256_srli_epi16::<3>(b);

                    // If semi-transparency is enabled, blend existing colors with new colors
                    if SEMI_TRANS {
                        let (existing_r, existing_g, existing_b) = split_15bit_color(existing);
                        let semi_transparency_mask =
                            _mm256_cmpeq_epi16(semi_transparency_bits, _mm256_setzero_si256());

                        (r, g, b) = apply_semi_transparency(
                            (existing_r, existing_g, existing_b),
                            (r, g, b),
                            semi_transparency_mask,
                            ctx.semi_transparency,
                        );
                    }
                    let x = x as i16;
                    let px = _mm256_setr_epi16(
                        x,
                        x + 1,
                        x + 2,
                        x + 3,
                        x + 4,
                        x + 5,
                        x + 6,
                        x + 7,
                        x + 8,
                        x + 9,
                        x + 10,
                        x + 11,
                        x + 12,
                        x + 13,
                        x + 14,
                        x + 15,
                    );

                    // mask out padding pixels
                    let mut write_mask = _mm256_and_si256(
                        inside_mask,
                        _mm256_andnot_si256(
                            _mm256_cmpgt_epi16(_mm256_set1_epi16(min_x as i16 - 1), px),
                            _mm256_cmpgt_epi16(_mm256_set1_epi16((max_x + 1) as i16), px),
                        ),
                    );
                    if ctx.preserve_masked_pixels {
                        write_mask = _mm256_and_si256(
                            write_mask,
                            _mm256_cmpeq_epi16(
                                _mm256_and_si256(existing, _mm256_set1_epi16(1 << 15)),
                                _mm256_setzero_si256(),
                            ),
                        )
                    };
                    let color = _mm256_or_si256(combine_rgb_components(r, g, b), mask_bits);
                    _mm256_storeu_si256(
                        vram_addr,
                        _mm256_or_si256(
                            _mm256_and_si256(write_mask, color),
                            _mm256_andnot_si256(write_mask, existing),
                        ),
                    );
                    // _mm256_storeu_si256(
                    //     vram_addr,
                    //          color
                    // );
                }
            }
        }
    }

    pub fn draw_rectangle<const SEMI_TRANS: bool>(
        &mut self,
        mut v: Vertex,
        side: Vertex,
        color: Colour,
        ctx: &RenderingContext,
    ) {
        v.x += ctx.drawing_x_offset as i32;
        v.y += ctx.drawing_y_offset as i32;

        let Some((min_x, min_y, max_x, max_y)) =
            self.clip_rect(v.x, v.y, v.x + side.x - 1, v.y + side.y - 1, ctx)
        else {
            return;
        };
        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx2") {
            unsafe {
                self.draw_rectangle_avx2::<SEMI_TRANS>(
                    min_x as usize,
                    min_y as usize,
                    max_x as usize,
                    max_y as usize,
                    color,
                    ctx,
                );
                return;
            }
        }

        panic!("not impl")
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    unsafe fn draw_rectangle_avx2<const SEMI_TRANS: bool>(
        &mut self,
        min_x: usize,
        min_y: usize,
        max_x: usize,
        max_y: usize,
        color: Colour,
        ctx: &RenderingContext,
    ) {
        let vram_ptr = self.vram.as_mut_ptr() as *mut u16;

        // let forced_mask_bit = i16::from(ctx.force_set_mask_bit) << 15;

        let min_x_aligned = min_x & !0xF;
        let max_x_aligned = (max_x + 15) & !0xF;

        // println!(
        //     "minx aligned {}, max x aligned: {}",
        //     min_x_aligned, max_x_aligned
        // );

        // let color_vec = _mm256_set1_epi16(color.to_u16() as i16);
        let color_r = _mm256_set1_epi16(color.r as i16);
        let color_g = _mm256_set1_epi16(color.g as i16);
        let color_b = _mm256_set1_epi16(color.b as i16);
        let color = color.to_u16();

        let span_width = max_x - min_x + 1;
        // println!("draw rect: {}, semi-trans? {}", span_width, SEMI_TRANS);
        for y in min_y..=max_y {
            let row_offset = y * 1024 + min_x_aligned;
            unsafe {
                let mut ptr = vram_ptr.add(row_offset);

                for x in (min_x_aligned..=max_x_aligned).step_by(16) {
                    let x = x as i16;
                    let px = _mm256_setr_epi16(
                        x,
                        x + 1,
                        x + 2,
                        x + 3,
                        x + 4,
                        x + 5,
                        x + 6,
                        x + 7,
                        x + 8,
                        x + 9,
                        x + 10,
                        x + 11,
                        x + 12,
                        x + 13,
                        x + 14,
                        x + 15,
                    );
                    use std::arch::x86_64::{_mm256_cmpgt_epi16, _mm256_setr_epi16};

                    let existing = _mm256_load_si256(ptr as *const __m256i);

                    // mask out padding pixels
                    let mut write_mask = _mm256_andnot_si256(
                        _mm256_cmpgt_epi16(_mm256_set1_epi16(min_x as i16 - 1), px),
                        _mm256_cmpgt_epi16(_mm256_set1_epi16((max_x + 1) as i16), px),
                    );
                    if ctx.preserve_masked_pixels {
                        write_mask = _mm256_and_si256(
                            write_mask,
                            _mm256_cmpeq_epi16(
                                _mm256_and_si256(existing, _mm256_set1_epi16(1 << 15)),
                                _mm256_setzero_si256(),
                            ),
                        )
                    };
                    let mask_bits = _mm256_set1_epi16((ctx.force_set_mask_bit as i16) << 15);
                    let semi_transparency_bits = _mm256_set1_epi16(1 << 15);

                    let mut r = color_r;
                    let mut g = color_g;
                    let mut b = color_b;

                    // Truncate color from from 8-bit RGB components to 5-bit
                    r = _mm256_srli_epi16::<3>(r);
                    g = _mm256_srli_epi16::<3>(g);
                    b = _mm256_srli_epi16::<3>(b);

                    if SEMI_TRANS {
                        let semi_transparency_mask =
                            _mm256_cmpeq_epi16(semi_transparency_bits, _mm256_setzero_si256());
                        let (existing_r, existing_g, existing_b) = split_15bit_color(existing);
                        (r, g, b) = apply_semi_transparency(
                            (existing_r, existing_g, existing_b),
                            (r, g, b),
                            semi_transparency_mask,
                            ctx.semi_transparency,
                        )
                    }

                    let color = _mm256_or_si256(combine_rgb_components(r, g, b), mask_bits);

                    _mm256_storeu_si256(
                        ptr as *mut __m256i,
                        _mm256_or_si256(
                            _mm256_and_si256(write_mask, color),
                            _mm256_andnot_si256(write_mask, existing),
                        ),
                    );
                    ptr = ptr.add(16);
                }
            }
        }
    }

    pub fn draw_rectangle_textured<const SEMI_TRANS: bool, const BLEND: bool>(
        &mut self,
        v: Vertex,
        side: Vertex,
        color: Colour,
        clut: u16,
        uv: [u16; 2],
        ctx: &RenderingContext,
    ) {
        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx2") {
            unsafe {
                self.draw_rectangle_textured_avx2::<SEMI_TRANS, BLEND>(
                    v, side, color, clut, uv, ctx,
                );
                return;
            }
        }

        panic!("not impl")
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    unsafe fn draw_rectangle_textured_avx2<const SEMI_TRANS: bool, const BLEND: bool>(
        &mut self,
        mut v: Vertex,
        side: Vertex,
        color: Colour,
        clut: u16,
        uv: [u16; 2],
        ctx: &RenderingContext,
    ) {
        use std::arch::x86_64::{__m256i, _mm256_storeu_si256};
        use std::arch::x86_64::{_mm256_set1_epi16, _mm256_set1_epi32};

        v.x += ctx.drawing_x_offset as i32;
        v.y += ctx.drawing_y_offset as i32;

        let Some((min_x, min_y, max_x, max_y)) =
            self.clip_rect(v.x, v.y, v.x + side.x - 1, v.y + side.y - 1, ctx)
        else {
            return;
        };
        let vram_ptr = self.vram.as_mut_ptr() as *mut u16;

        let min_x_aligned = min_x & !0xF;
        let max_x_aligned = (max_x + 15) & !0xF;
        let padding = min_x - min_x_aligned;

        // let color_vec = _mm256_set1_epi16(color.to_u16() as i16);
        let color_r = _mm256_set1_epi16(color.r as i16);
        let color_g = _mm256_set1_epi16(color.g as i16);
        let color_b = _mm256_set1_epi16(color.b as i16);
        let color_u16 = color.to_u16();

        let clut = Clut::new(clut);
        let texture = Texture {
            base_x: (ctx.page_base_x as usize), //* 64,
            base_y: (ctx.page_base_y as usize) * 256,
            semi_transparency: ctx.semi_transparency,
            depth: ctx.texture_depth,
            clut,
            dithering: ctx.dithering,
            draw_to_display: ctx.draw_to_display,
        };

        for y in min_y as usize..=max_y as usize {
            let row_offset = y * 1024 + min_x_aligned as usize;
            unsafe {
                let mut ptr = vram_ptr.add(row_offset);

                for x in (min_x_aligned..=max_x_aligned).step_by(16) {
                    let x = x as i16;
                    let px = _mm256_setr_epi16(
                        x,
                        x + 1,
                        x + 2,
                        x + 3,
                        x + 4,
                        x + 5,
                        x + 6,
                        x + 7,
                        x + 8,
                        x + 9,
                        x + 10,
                        x + 11,
                        x + 12,
                        x + 13,
                        x + 14,
                        x + 15,
                    );
                    use std::arch::x86_64::{_mm256_cmpgt_epi16, _mm256_setr_epi16};

                    let existing = _mm256_load_si256(ptr as *const __m256i);

                    // mask out padding pixels
                    let mut write_mask = _mm256_andnot_si256(
                        _mm256_cmpgt_epi16(_mm256_set1_epi16(min_x as i16 - 1), px),
                        _mm256_cmpgt_epi16(_mm256_set1_epi16((max_x + 1) as i16), px),
                    );
                    if ctx.preserve_masked_pixels {
                        write_mask = _mm256_and_si256(
                            write_mask,
                            _mm256_cmpeq_epi16(
                                _mm256_and_si256(existing, _mm256_set1_epi16(1 << 15)),
                                _mm256_setzero_si256(),
                            ),
                        )
                    };
                    let mask_bits = _mm256_set1_epi16((ctx.force_set_mask_bit as i16) << 15);
                    let semi_transparency_bits = _mm256_set1_epi16(1 << 15);

                    let u = _mm256_and_si256(
                        _mm256_set1_epi16(0x00FF),
                        _mm256_add_epi16(
                            px,
                            _mm256_set1_epi16(
                                uv[0].wrapping_sub(min_x_aligned as u16 + padding as u16) as i16,
                            ), // TODO: investigate what's with this uv offsets
                        ),
                    );
                    let v = _mm256_set1_epi16(uv[1].wrapping_add(y as u16 - min_y as u16) as i16);
                    // let v = _mm256_set1_epi16(uv[1] as i16);

                    // {
                    // println!("u0: {}, v0: {}", uv[0], uv[1]);
                    // let arr: [u16; 16] = std::mem::transmute(u);
                    // println!("us: {:?}", arr);
                    // let arr: [u16; 16] = std::mem::transmute(v);
                    // println!("vs: {:?}", arr);
                    // }

                    let mut r = color_r;
                    let mut g = color_g;
                    let mut b = color_b;

                    let texels = read_texture(vram_ptr, texture, u, v, ctx);
                    // Mask out any pixels where the texel value is $0000
                    write_mask = _mm256_andnot_si256(
                        _mm256_cmpeq_epi16(texels, _mm256_setzero_si256()),
                        write_mask,
                    );

                    // let arr: [u16; 16] = std::mem::transmute(texels);
                    // println!("texels: {:?}", arr);

                    let (tr, tg, tb) = convert_15bit_to_24bit(texels);

                    // Optionally apply texture color modulation
                    if BLEND {
                        r = modulate_texture_color(tr, r);
                        g = modulate_texture_color(tg, g);
                        b = modulate_texture_color(tb, b);
                    } else {
                        r = tr;
                        g = tg;
                        b = tb;
                    }

                    // Truncate color from from 8-bit RGB components to 5-bit
                    r = _mm256_srli_epi16::<3>(r);
                    g = _mm256_srli_epi16::<3>(g);
                    b = _mm256_srli_epi16::<3>(b);

                    if SEMI_TRANS {
                        let semi_transparency_mask =
                            _mm256_cmpeq_epi16(semi_transparency_bits, _mm256_setzero_si256());
                        let (existing_r, existing_g, existing_b) = split_15bit_color(existing);
                        (r, g, b) = apply_semi_transparency(
                            (existing_r, existing_g, existing_b),
                            (r, g, b),
                            semi_transparency_mask,
                            ctx.semi_transparency,
                        )
                    }

                    let color = _mm256_or_si256(combine_rgb_components(r, g, b), mask_bits);

                    _mm256_storeu_si256(
                        ptr as *mut __m256i,
                        _mm256_or_si256(
                            _mm256_and_si256(write_mask, color),
                            _mm256_andnot_si256(write_mask, existing),
                        ),
                    );
                    ptr = ptr.add(16);
                }
            }
        }
    }

    // affected by blit settings... but ... what .. it's not this
    pub fn vram2vram_blit(&mut self, src: Vertex, dst: Vertex, size: Vertex) {
        let width = size.x as usize;
        let height = size.y as usize;

        for y in 0..height {
            for x in 0..width {
                let vram_addr = 2 * ((y + src.y as usize) * 1024 + x + (src.x as usize));
                let pixel_lsb = self.vram[vram_addr];
                let pixel_msb = self.vram[vram_addr + 1];

                let vram_addr = 2 * ((y + dst.y as usize) * 1024 + x + (dst.x as usize));
                self.vram[vram_addr] = pixel_lsb;
                self.vram[vram_addr + 1] = pixel_msb;
            }
        }
    }

    pub fn print_profiling_results(&mut self) {
        print!("\x1B[2J\x1B[1;1H"); // Clear terminal and put cursor to first row first column
        println!();
        let total: Duration = self.frame_times.iter().sum();
        let avg = total / 60;
        let max = self.frame_times.iter().max().unwrap();
        let min = self.frame_times.iter().min().unwrap();

        println!(
            "frame time, avg: {}, min: {}, max: {}",
            avg.as_micros(),
            min.as_micros(),
            max.as_micros()
        )
    }

    pub fn render_fb(
        &mut self,
        framebuffer: &Mutex<Vec<u32>>,
        full_ram: bool,
        ctx: &RenderingContext,
    ) -> (usize, usize, usize, usize, DisplayDepth) {
        let mut mutex = framebuffer.lock().unwrap();
        let output_frame_buffer: &mut [u32] = mutex.as_mut();

        self.frame_num += 1;

        if self.frame_num == 60 {
            self.frame_num = 0;
            if self.profile {
                self.print_profiling_results();
            }
            for d in &mut self.frame_times {
                *d = Duration::from_secs(0);
            }
        }

        if ctx.display_disabled {
            for y in 0..16 {
                for x in 0..16 {
                    output_frame_buffer[1024 * y + x] = 0;
                }
            }
            return (16, 16, 0, 0, ctx.display_depth);
        }
        if full_ram {
            for i in 0..1024 * 512 {
                output_frame_buffer[i] = self.vram[2 * i] as u32;
                output_frame_buffer[i] |= (self.vram[2 * i + 1] as u32) << 8;
            }
            // output_frame_buffer.copy_from_slice(bytemuck::cast_slice(&self.vram[0..]));
            // for y in 0..512 {
            //     for x in 0..1024 {
            //         let vram_addr = 2 * (1024 * y + x);
            //         let pixel =
            //             u16::from_le_bytes([self.vram[vram_addr], self.vram[vram_addr + 1]]);
            //
            //         let r = convert_5bit_to_8bit(pixel & 0x1F);
            //         let g = convert_5bit_to_8bit((pixel >> 5) & 0x1F);
            //         let b = convert_5bit_to_8bit((pixel >> 10) & 0x1F);
            //
            //         output_frame_buffer[1024 * y + x] = Color { r, g, b, a: 255 };
            //     }
            // }
            (1024, 512, 0, 0, ctx.display_depth)
        } else {
            let (sx, sy, width, height, _interlaced) = (
                ctx.display_vram_x_start as usize,
                ctx.display_vram_y_start as usize,
                ctx.hres.into_pixels(),
                ctx.vres.into_pixels(),
                ctx.interlaced,
            );
            for i in 0..1024 * 512 {
                output_frame_buffer[i] = self.vram[2 * i] as u32;
                output_frame_buffer[i] |= (self.vram[2 * i + 1] as u32) << 8;
            }
            // output_frame_buffer.copy_from_slice(bytemuck::cast_slice(&self.vram[0..]));
            // match ctx.display_depth {
            //     DisplayDepth::D15Bits => {
            //         for y in 0..height {
            //             for x in 0..width {
            //                 let vram_addr = 2 * (1024 * (sy + y) + (sx + x));
            //                 let pixel = u16::from_le_bytes([
            //                     self.vram[vram_addr],
            //                     self.vram[vram_addr + 1],
            //                 ]);
            //
            //                 let r = convert_5bit_to_8bit(pixel & 0x1F);
            //                 let g = convert_5bit_to_8bit((pixel >> 5) & 0x1F);
            //                 let b = convert_5bit_to_8bit((pixel >> 10) & 0x1F);
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
            // black out lines outside of display field (wrong impl)
            // let vrange = (ctx.display_line_end - ctx.display_line_start) as usize;
            // if height >= vrange {
            //     let starting_row = vrange * 1024 * if interlaced { 2 } else { 1 };
            //     output_frame_buffer[starting_row..].fill(Color { r: 0, g: 0, b: 0, a: 255});
            // }
            (width, height, sx, sy, ctx.display_depth)
        }
    }

    pub fn cpu_to_vram_copy(&mut self, top_left: (u16, u16), size: (u16, u16), data: &Vec<u32>) {
        let mut current_row = 0;
        let mut current_col = 0;
        // 2 halfwords per GP0 write
        data.iter().for_each(|&word| {
            for i in 0..2 {
                let halfword = (word >> (16 * i)) as u16;

                let vram_row = ((top_left.1 + current_row) & 0x1FF) as usize;
                let vram_col = ((top_left.0 + current_col) & 0x3FF) as usize;

                let [lsb, msb] = halfword.to_le_bytes();
                let vram_addr = 2 * (1024 * vram_row + vram_col);

                self.vram[vram_addr] = lsb;
                self.vram[vram_addr + 1] = msb;

                current_col += 1;
                if current_col == size.0 {
                    current_col = 0;
                    current_row += 1;
                }
            }
        });
    }

    pub fn vram_to_cpu_copy(&self, top_left: (u16, u16), size: (u16, u16)) -> Vec<u32> {
        let mut data = vec![];
        let imgsize = size.0 as u32 * size.1 as u32;
        // rounding so we have 16 bit of padding in last word
        let imgsize = (imgsize + 1) & !1;
        let mut words_remaining = imgsize / 2;
        let mut current_row = 0;
        let mut current_col = 0;
        // 2 halfwords per GP0 read..
        loop {
            let mut word = 0_u32;
            for i in 0..2 {
                // let halfword = (word >> (16 * i)) as u16;

                let vram_row = ((top_left.1 + current_row) & 0x1FF) as usize;
                let vram_col = ((top_left.0 + current_col) & 0x3FF) as usize;

                // let [lsb, msb] = halfword.to_le_bytes();
                let vram_addr = 2 * (1024 * vram_row + vram_col);

                let lsb = self.vram[vram_addr];
                let msb = self.vram[vram_addr + 1];
                let halfword = u32::from_le_bytes([lsb, msb, 0, 0]);
                word |= halfword << (i * 16);

                current_col += 1;
                if current_col == size.0 {
                    current_col = 0;
                    current_row += 1;
                }
            }
            data.push(word);
            words_remaining -= 1;
            if words_remaining == 0 {
                return data;
            }
        }
    }
}

// clockwise order (psx has inverted y coord)
fn ensure_vertex_order(vs: &mut [Vertex]) {
    let cross_product_z =
        (vs[1].x - vs[0].x) * (vs[2].y - vs[0].y) - (vs[1].y - vs[0].y) * (vs[2].x - vs[0].x);
    if cross_product_z < 0 {
        vs.swap(0, 1);
        // std::mem::swap(v0, v1);
    }
}

fn ensure_vertex_order2<T: Clone>(vs: &mut [Vertex], attrs: &mut [T]) {
    let cross_product_z =
        (vs[1].x - vs[0].x) * (vs[2].y - vs[0].y) - (vs[1].y - vs[0].y) * (vs[2].x - vs[0].x);
    if cross_product_z < 0 {
        let tmp = vs[0];
        vs[0] = vs[1];
        vs[1] = tmp;
        let tmp = attrs[0].clone();
        attrs[0] = attrs[1].clone();
        attrs[1] = tmp;
        // std::mem::swap(v0, v1);
    }
}
fn ensure_vertex_order3<T: Clone, E: Clone>(vs: &mut [Vertex], attrs: &mut [T], attrs1: &mut [E]) {
    let cross_product_z =
        (vs[1].x - vs[0].x) * (vs[2].y - vs[0].y) - (vs[1].y - vs[0].y) * (vs[2].x - vs[0].x);
    if cross_product_z < 0 {
        let tmp = vs[0];
        vs[0] = vs[1];
        vs[1] = tmp;
        let tmp = attrs[0].clone();
        attrs[0] = attrs[1].clone();
        attrs[1] = tmp;
        let tmp = attrs1[0].clone();
        attrs1[0] = attrs1[1].clone();
        attrs1[1] = tmp;
        // std::mem::swap(v0, v1);
    }
}

// fn ensure_vertex_and_color_order(
//     v0: &mut Vertex,
//     v1: &mut Vertex,
//     v2: Vertex,
//     c0: &mut Colour,
//     c1: &mut Colour,
//     _: Colour,
// ) {
//     let cross_product_z = (v1.x - v0.x) * (v2.y - v0.y) - (v1.y - v0.y) * (v2.x - v0.x);
//     if cross_product_z < 0 {
//         std::mem::swap(v0, v1);
//         std::mem::swap(c0, c1);
//     }
// }

fn cross_product_z(v0: Vertex, v1: Vertex, v2: Vertex) -> i32 {
    (v1.x - v0.x) * (v2.y - v0.y) - (v1.y - v0.y) * (v2.x - v0.x)
}

fn is_inside_triangle(p: Vertex, v0: Vertex, v1: Vertex, v2: Vertex) -> bool {
    for (va, vb) in [(v0, v1), (v1, v2), (v2, v0)] {
        let cpz = cross_product_z(va, vb, p);
        if cpz < 0 {
            return false;
        }
        if cpz == 0 {
            // If the cross product Z component is 0, this point lies exactly on an edge.
            // Per the top-left rule, this pixel should be rasterized only if it does not lie
            // on a bottom or right edge.

            // Assuming clockwise order and an inverted Y axis, if the Y value increases from Va to Vb,
            // this edge is right-oriented
            if vb.y > va.y {
                // Pixel lies on a right-oriented edge, skip
                return false;
            }

            // If the Y coordinates are equal and the X coordinate decreases, this is a horizontal bottom edge
            if vb.y == va.y && vb.x < va.x {
                // Pixel lies on a horizontal bottom edge, skip
                return false;
            }

            // Otherwise, this is either a horizontal top edge or a left-oriented edge; rasterize it
            // (If it doesn't also lie on a different edge that is bottom/right)
        }
    }

    true
}

fn compute_barycentric_coordinates(p: Vertex, v0: Vertex, v1: Vertex, v2: Vertex) -> [f64; 3] {
    let denominator = cross_product_z(v0, v1, v2);
    if denominator == 0 {
        return [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0];
    }
    let denominator: f64 = denominator.into();
    let lambda0 = f64::from(cross_product_z(v1, v2, p)) / denominator;
    let lambda1 = f64::from(cross_product_z(v2, v0, p)) / denominator;
    let lambda2 = 1.0 - lambda0 - lambda1;
    [lambda0, lambda1, lambda2]
}
fn compute_normal_coordinates(p: [f64; 3], vs: &[[u16; 2]]) -> [usize; 2] {
    let x = p[0] * (vs[0][0] as f64) + p[1] * (vs[1][0] as f64) + p[2] * (vs[2][0] as f64);
    let y = p[0] * (vs[0][1] as f64) + p[1] * (vs[1][1] as f64) + p[2] * (vs[2][1] as f64);
    [x.round() as usize, y.round() as usize]
}

#[derive(Debug, Clone, Copy)]
pub struct Clut {
    pub base_x: usize,
    pub base_y: usize,
}

impl Clut {
    pub fn new(data: u16) -> Self {
        Self {
            base_x: ((data & 0x3f) as usize) * 16, // in 16 halfword steps
            base_y: (((data >> 6) & 0x1ff) as usize), // in 1 line steps
        }
    }

    pub fn get_color(&self, renderer: &Renderer, index: u8) -> (u8, u8) {
        let pixel_lsb = renderer.vram[2 * (self.base_y * 1024 + self.base_x + index as usize)];
        let pixel_msb = renderer.vram[2 * (self.base_y * 1024 + self.base_x + index as usize) + 1];
        (pixel_lsb, pixel_msb)
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct Texture {
    base_x: usize,
    base_y: usize,
    pub semi_transparency: u8,
    pub dithering: bool,
    draw_to_display: bool,
    pub depth: TextureDepth,
    clut: Clut,
}

impl Texture {
    pub fn new_avx2(data: u16, clut: Clut) -> Self {
        let base_x = ((data & 0xf) as usize); // n * 64
        let base_y = (((data >> 4) & 1) as usize) * 256; // n * 256
        let semi_transparency = ((data >> 5) & 3) as u8;
        let dithering = (data >> 9) & 1 != 0; // not used!!!
        let draw_to_display = (data >> 10) & 1 != 0; // not used!!!
        // let depth = ((data >> 7) & 3) as u8;

        let depth = match (data >> 7) & 3 {
            0 => TextureDepth::T4Bit,
            1 => TextureDepth::T8Bit,
            2 => TextureDepth::T15Bit,
            3 => TextureDepth::T4Bit,
            n => unreachable!("Unhandled texture depth: {n}"),
        };
        Texture {
            base_x,
            base_y,
            semi_transparency,
            dithering,
            draw_to_display,
            depth,
            clut,
        }
    }
    pub fn new(data: u16, clut: Clut) -> Self {
        let base_x = ((data & 0xf) as usize) * 64; // n * 64
        let base_y = (((data >> 4) & 1) as usize) * 256; // n * 256
        let semi_transparency = ((data >> 5) & 3) as u8;
        let dithering = (data >> 9) & 1 != 0; // not used!!!
        let draw_to_display = (data >> 10) & 1 != 0; // not used!!!
        // let depth = ((data >> 7) & 3) as u8;

        let depth = match (data >> 7) & 3 {
            0 => TextureDepth::T4Bit,
            1 => TextureDepth::T8Bit,
            2 => TextureDepth::T15Bit,
            3 => TextureDepth::T4Bit,
            n => unreachable!("Unhandled texture depth: {n}"),
        };
        Texture {
            base_x,
            base_y,
            semi_transparency,
            dithering,
            draw_to_display,
            depth,
            clut,
        }
    }

    pub fn get_texpage_base(&self) -> [u8; 2] {
        [(self.base_x / 64) as u8, (self.base_y / 256) as u8]
    }

    pub fn get_texel(
        &self,
        renderer: &Renderer,
        uv_x: usize,
        uv_y: usize,
        ctx: &RenderingContext,
    ) -> Colour {
        let [uv_x, uv_y] = renderer.compute_texel_offset(uv_x, uv_y, ctx);

        let (pixel_lsb, pixel_msb) = match self.depth {
            TextureDepth::T4Bit => {
                let pixel =
                    renderer.vram[self.base_y * 2048 + uv_y * 2048 + 2 * self.base_x + uv_x / 2];
                let index = (pixel >> 4 * (uv_x & 1)) & 0xF;

                self.clut.get_color(renderer, index)
            }
            TextureDepth::T8Bit => {
                // Width 2048...
                let index =
                    renderer.vram[self.base_y * 2048 + uv_y * 2048 + 2 * self.base_x + uv_x];

                self.clut.get_color(renderer, index)
            }
            TextureDepth::T15Bit => {
                let texture_x = self.base_x + uv_x;
                let texture_y = self.base_y + uv_y;
                let pixel_lsb = renderer.vram[2 * (texture_y * 1024 + texture_x)];
                let pixel_msb = renderer.vram[2 * (texture_y * 1024 + texture_x) + 1];
                (pixel_lsb, pixel_msb)
                // let vram_addr = 2 * (y * 1024 + x) as usize;

                // self.vram[vram_addr] = pixel_lsb;
                // self.vram[vram_addr + 1] = pixel_msb;
            }
        };

        Colour::from_bytes(pixel_msb, pixel_lsb)
    }
}

fn interpolate_color(lambda: &[f64], colors: &[Colour]) -> Colour {
    let colors_r: Vec<f64> = colors.iter().map(|c| f64::from(c.r)).collect();
    let colors_g: Vec<f64> = colors.iter().map(|c| f64::from(c.g)).collect();
    let colors_b: Vec<f64> = colors.iter().map(|c| f64::from(c.b)).collect();

    let r =
        (lambda[0] * colors_r[0] + lambda[1] * colors_r[1] + lambda[2] * colors_r[2]).round() as u8;
    let g =
        (lambda[0] * colors_g[0] + lambda[1] * colors_g[1] + lambda[2] * colors_g[2]).round() as u8;
    let b =
        (lambda[0] * colors_b[0] + lambda[1] * colors_b[1] + lambda[2] * colors_b[2]).round() as u8;

    Colour { r, g, b, m: 0 }
}

const DITHER_TABLE: &[[i8; 4]; 4] = &[
    [-4, 0, -3, 1],
    [2, -2, 3, 1],
    [-3, 1, -4, 0],
    [3, -1, 2, -2],
];

fn apply_dithering(color: Colour, p: Vertex) -> Colour {
    let offset = DITHER_TABLE[(p.y & 3) as usize][(p.x & 3) as usize];
    Colour {
        r: color.r.saturating_add_signed(offset),
        g: color.g.saturating_add_signed(offset),
        b: color.b.saturating_add_signed(offset),
        m: color.m,
    }
}

pub const OPAQUE: bool = false;
pub const SEMI_TRANS: bool = true;
pub const BLEND: bool = true;
pub const RAW: bool = false;
pub const TEXTURED: bool = true;
pub const UNTEXTURED: bool = false;

// https://codeberg.org/jsgroth/CoffeePSX/src/commit/ed7c266bf43cf9ffd4211be6d8cf15c8424033f2/crates/ps1-core/src/gpu/rasterizer/simd/avx2.rs
// input vectors: i16x16
#[target_feature(enable = "avx2")]
unsafe fn apply_semi_transparency(
    (existing_r, existing_g, existing_b): (__m256i, __m256i, __m256i),
    (r, g, b): (__m256i, __m256i, __m256i),
    semi_transparency_mask: __m256i,
    semi_transparency_mode: u8,
) -> (__m256i, __m256i, __m256i) {
    unsafe {
        let (blended_r, blended_g, blended_b) = match semi_transparency_mode {
            0 => (
                blend_average(existing_r, r),
                blend_average(existing_g, g),
                blend_average(existing_b, b),
            ),
            1 => (
                blend_add(existing_r, r),
                blend_add(existing_g, g),
                blend_add(existing_b, b),
            ),
            2 => (
                blend_subtract(existing_r, r),
                blend_subtract(existing_g, g),
                blend_subtract(existing_b, b),
            ),
            3 => (
                blend_add_quarter(existing_r, r),
                blend_add_quarter(existing_g, g),
                blend_add_quarter(existing_b, b),
            ),
            _ => unreachable!(),
        };

        let r = _mm256_or_si256(
            _mm256_andnot_si256(semi_transparency_mask, blended_r),
            _mm256_and_si256(semi_transparency_mask, r),
        );

        let g = _mm256_or_si256(
            _mm256_andnot_si256(semi_transparency_mask, blended_g),
            _mm256_and_si256(semi_transparency_mask, g),
        );

        let b = _mm256_or_si256(
            _mm256_andnot_si256(semi_transparency_mask, blended_b),
            _mm256_and_si256(semi_transparency_mask, b),
        );

        (r, g, b)
    }
}

// i16x16  (B + F) / 2
#[target_feature(enable = "avx2")]
unsafe fn blend_average(back: __m256i, front: __m256i) -> __m256i {
    _mm256_srli_epi16::<1>(_mm256_add_epi16(back, front))
}

#[target_feature(enable = "avx2")]
unsafe fn blend_add(back: __m256i, front: __m256i) -> __m256i {
    _mm256_min_epi16(_mm256_add_epi16(back, front), _mm256_set1_epi16(0x1F))
}

// i16x16  (B - F)
#[target_feature(enable = "avx2")]
unsafe fn blend_subtract(back: __m256i, front: __m256i) -> __m256i {
    _mm256_max_epi16(_mm256_sub_epi16(back, front), _mm256_setzero_si256())
}

#[target_feature(enable = "avx2")]
unsafe fn blend_add_quarter(back: __m256i, front: __m256i) -> __m256i {
    _mm256_min_epi16(
        _mm256_add_epi16(back, _mm256_srli_epi16::<2>(front)),
        _mm256_set1_epi16(0x1F),
    )
}

// i16x16
#[target_feature(enable = "avx2")]
unsafe fn split_15bit_color(texels: __m256i) -> (__m256i, __m256i, __m256i) {
    let mask = _mm256_set1_epi16(0x001F);
    let r = _mm256_and_si256(texels, mask);
    let g = _mm256_and_si256(_mm256_srli_epi16::<5>(texels), mask);
    let b = _mm256_and_si256(_mm256_srli_epi16::<10>(texels), mask);

    (r, g, b)
}

#[target_feature(enable = "avx2")]
unsafe fn combine_rgb_components(r: __m256i, g: __m256i, b: __m256i) -> __m256i {
    _mm256_or_si256(
        r,
        _mm256_or_si256(_mm256_slli_epi16::<5>(g), _mm256_slli_epi16::<10>(b)),
    )
}

// Unpack an i16x16 vector into two i32x8 vectors such that the two vectors can later be repacked
// using _mm256_packus_epi32 or _mm256_packs_epi32
#[target_feature(enable = "avx2")]
unsafe fn unpack_epi16_vector(v: __m256i) -> (__m256i, __m256i) {
    let low_shuffle_mask: __m256i = mem::transmute(*LOW_SHUFFLE_MASK);
    let high_shuffle_mask: __m256i = mem::transmute(*HIGH_SHUFFLE_MASK);

    let low = _mm256_shuffle_epi8(v, low_shuffle_mask);
    let high = _mm256_shuffle_epi8(v, high_shuffle_mask);

    (low, high)
}
const LOW_SHUFFLE_MASK: &[u8; 32] = &[
    0, 1, 0x80, 0x80, 2, 3, 0x80, 0x80, 4, 5, 0x80, 0x80, 6, 7, 0x80, 0x80, 16, 17, 0x80, 0x80, 18,
    19, 0x80, 0x80, 20, 21, 0x80, 0x80, 22, 23, 0x80, 0x80,
];

const HIGH_SHUFFLE_MASK: &[u8; 32] = &[
    8, 9, 0x80, 0x80, 10, 11, 0x80, 0x80, 12, 13, 0x80, 0x80, 14, 15, 0x80, 0x80, 24, 25, 0x80,
    0x80, 26, 27, 0x80, 0x80, 28, 29, 0x80, 0x80, 30, 31, 0x80, 0x80,
];

// Read a row of 16 texels from a texture in VRAM.
// U and V vectors should be i16x16.
// Return value is an i16x16 vector containing raw 16-bit texel values (RGB555 + semi-transparency bit).
#[target_feature(enable = "avx2")]
unsafe fn read_texture(
    vram: *mut u16,
    texture: Texture,
    u: __m256i,
    v: __m256i,
    ctx: &RenderingContext,
) -> __m256i {
    unsafe {
        let x_mask = _mm256_set1_epi16((ctx.texture_window_x_mask << 3) as i16);
        let y_mask = _mm256_set1_epi16((ctx.texture_window_y_mask << 3) as i16);

        let masked_u = _mm256_or_si256(
            _mm256_andnot_si256(x_mask, u),
            _mm256_and_si256(
                x_mask,
                _mm256_set1_epi16((ctx.texture_window_x_offset << 3) as i16),
            ),
        );

        let masked_v = _mm256_or_si256(
            _mm256_andnot_si256(y_mask, v),
            _mm256_and_si256(
                y_mask,
                _mm256_set1_epi16((ctx.texture_window_y_offset << 3) as i16),
            ),
        );

        let (masked_u0, masked_u1) = unpack_epi16_vector(masked_u);
        let (masked_v0, masked_v1) = unpack_epi16_vector(masked_v);

        let (texels0, texels1) = match texture.depth {
            TextureDepth::T4Bit => (
                read_4bpp_texture(vram, &texture, masked_u0, masked_v0),
                read_4bpp_texture(vram, &texture, masked_u1, masked_v1),
            ),
            TextureDepth::T8Bit => (
                read_8bpp_texture(vram, &texture, masked_u0, masked_v0),
                read_8bpp_texture(vram, &texture, masked_u1, masked_v1),
            ),
            TextureDepth::T15Bit => (
                read_15bpp_texture(vram, &texture, masked_u0, masked_v0),
                read_15bpp_texture(vram, &texture, masked_u1, masked_v1),
            ),
        };

        _mm256_packus_epi32(texels0, texels1)
    }
}

// Read a row of 8 texels from a 4bpp texture.
// U and V vectors should be i32x8.
// Return value is u16s stored in an i32x8 vector.
#[target_feature(enable = "avx2")]
unsafe fn read_4bpp_texture(vram: *mut u16, texture: &Texture, u: __m256i, v: __m256i) -> __m256i {
    let vram_y = _mm256_add_epi32(v, _mm256_set1_epi32(texture.base_y as i32));
    let vram_x_words = _mm256_add_epi32(
        _mm256_srli_epi32::<3>(u),
        _mm256_set1_epi32((32 * texture.base_x) as i32),
    );

    let vram_addr = _mm256_or_si256(_mm256_slli_epi32::<9>(vram_y), vram_x_words);
    let vram_shift = _mm256_slli_epi32::<2>(_mm256_and_si256(u, _mm256_set1_epi32(0x07)));

    let vram_words = _mm256_i32gather_epi32::<4>(vram.cast::<i32>(), vram_addr);
    let clut_indices = _mm256_and_si256(
        _mm256_srlv_epi32(vram_words, vram_shift),
        _mm256_set1_epi32(0xF),
    );

    let clut_offset = ((1024 * texture.clut.base_y) | (texture.clut.base_x)) as usize;
    let clut = vram.add(clut_offset);

    read_clut(clut, clut_indices)
}

#[target_feature(enable = "avx2")]
unsafe fn read_clut(clut: *mut u16, clut_indices: __m256i) -> __m256i {
    let addrs = _mm256_srli_epi32::<1>(clut_indices);
    let shifts = _mm256_slli_epi32::<4>(_mm256_and_si256(clut_indices, _mm256_set1_epi32(1)));

    let colors = _mm256_i32gather_epi32::<4>(clut.cast::<i32>(), addrs);
    _mm256_and_si256(_mm256_srlv_epi32(colors, shifts), _mm256_set1_epi32(0xFFFF))
}

// Read a row of 8 texels from an 8bpp texture.
// U and V coordinates should be i32x8.
// Return value is u16s stored in an i32x8 vector.
#[target_feature(enable = "avx2")]
unsafe fn read_8bpp_texture(vram: *mut u16, texture: &Texture, u: __m256i, v: __m256i) -> __m256i {
    let vram_y = _mm256_add_epi32(v, _mm256_set1_epi32(texture.base_y as i32));
    let vram_x_words = _mm256_and_si256(
        _mm256_add_epi32(
            _mm256_srli_epi32::<2>(u),
            _mm256_set1_epi32((32 * texture.base_x) as i32),
        ),
        _mm256_set1_epi32(0x1FF),
    );

    let vram_addr = _mm256_or_si256(_mm256_slli_epi32::<9>(vram_y), vram_x_words);
    let vram_shift = _mm256_slli_epi32::<3>(_mm256_and_si256(u, _mm256_set1_epi32(3)));

    let vram_words = _mm256_i32gather_epi32::<4>(vram.cast::<i32>(), vram_addr);
    let clut_indices = _mm256_and_si256(
        _mm256_srlv_epi32(vram_words, vram_shift),
        _mm256_set1_epi32(0xFF),
    );

    let clut_offset = ((1024 * texture.clut.base_y) | (texture.clut.base_x)) as usize;
    let clut = vram.add(clut_offset);

    read_clut(clut, clut_indices)
}

// Read a row of 8 texels from a 15bpp texture.
// U and V vectors should be i32x8.
// Return value is u16s stored in an i32x8 vector.
#[target_feature(enable = "avx2")]
unsafe fn read_15bpp_texture(vram: *mut u16, texture: &Texture, u: __m256i, v: __m256i) -> __m256i {
    unsafe {
        let vram_y = _mm256_add_epi32(v, _mm256_set1_epi32(texture.base_y as i32));
        let vram_x_words = _mm256_and_si256(
            _mm256_add_epi32(
                _mm256_srli_epi32::<1>(u),
                _mm256_set1_epi32((32 * texture.base_x) as i32),
            ),
            _mm256_set1_epi32(0x1FF),
        );

        let vram_addr = _mm256_or_si256(_mm256_slli_epi32::<9>(vram_y), vram_x_words);
        let vram_shift = _mm256_slli_epi32::<4>(_mm256_and_si256(u, _mm256_set1_epi32(1)));

        let vram_words = _mm256_i32gather_epi32::<4>(vram.cast::<i32>(), vram_addr);
        // let arr: [u32; 8] = std::mem::transmute(vram_addr);
        // println!("vram_addr: {:?}", arr);

        _mm256_and_si256(
            _mm256_srlv_epi32(vram_words, vram_shift),
            _mm256_set1_epi32(0xFFFF),
        )
    }
}

// Apply texture color modulation to a single color component.
// Input vectors should be i16x16 and the return value is i16x16.
#[target_feature(enable = "avx2")]
unsafe fn modulate_texture_color(tex_color: __m256i, shading_color: __m256i) -> __m256i {
    _mm256_min_epi16(
        _mm256_set1_epi16(255),
        _mm256_srli_epi16::<7>(_mm256_mullo_epi16(tex_color, shading_color)),
    )
}

// Convert a raw 15-bit color value from VRAM to individual 8-bit RGB color components
// Input vector should be i16x16 and the return values are i16x16
#[target_feature(enable = "avx2")]
unsafe fn convert_15bit_to_24bit(texels: __m256i) -> (__m256i, __m256i, __m256i) {
    let mask = _mm256_set1_epi16(0x00F8);
    let r = _mm256_and_si256(_mm256_slli_epi16::<3>(texels), mask);
    let g = _mm256_and_si256(_mm256_srli_epi16::<2>(texels), mask);
    let b = _mm256_and_si256(_mm256_srli_epi16::<7>(texels), mask);

    (r, g, b)
}

fn is_not_bottom_right_edge(v0: Vertex, v1: Vertex) -> i32 {
    let is_bottom_right = v1.y > v0.y || (v1.y == v0.y && v1.x < v0.x);
    if is_bottom_right { 0 } else { !0 }
}

// Determine which of the 8 points are inside the triangle.
// Input vectors should be i32x8.
// Return value is i32x8 where each lane is all 1s if inside the triangle and all 0s if outside.
#[target_feature(enable = "avx2")]
unsafe fn compute_write_mask(
    vs: &mut [Vertex],
    px: __m256i,
    py: __m256i,
    x_bounds: (i32, i32),
    v01_is_not_bottom_right: i32,
    v12_is_not_bottom_right: i32,
    v20_is_not_bottom_right: i32,
) -> __m256i {
    _mm256_and_si256(
        _mm256_and_si256(
            check_edge(vs[0], vs[1], px, py, v01_is_not_bottom_right),
            _mm256_and_si256(
                check_edge(vs[1], vs[2], px, py, v12_is_not_bottom_right),
                check_edge(vs[2], vs[0], px, py, v20_is_not_bottom_right),
            ),
        ),
        _mm256_andnot_si256(
            _mm256_cmpgt_epi32(_mm256_set1_epi32(x_bounds.0 + 1), px),
            _mm256_cmpgt_epi32(_mm256_set1_epi32(x_bounds.1 + 1), px),
        ),
    )
}

// Determine which of the 8 points are inside a single triangle edge.
// Input vectors should be i32x8.
// Return value is i32x8 where each lane is all 1s if inside the edge and all 0s if outside.
#[target_feature(enable = "avx2")]
unsafe fn check_edge(
    v0: Vertex,
    v1: Vertex,
    px: __m256i,
    py: __m256i,
    is_not_bottom_right: i32,
) -> __m256i {
    let cpz = cross_prod_z(v0, v1, px, py);
    let zero = _mm256_setzero_si256();

    _mm256_or_si256(
        _mm256_cmpgt_epi32(cpz, zero),
        _mm256_and_si256(
            _mm256_cmpeq_epi32(cpz, zero),
            _mm256_set1_epi32(is_not_bottom_right),
        ),
    )
}

// Compute the Z component of the cross product (v1 - v0) x (P - v0) for each point P.
// Input vectors should be i32x8 and return value is i32x8.
#[target_feature(enable = "avx2")]
unsafe fn cross_prod_z(v0: Vertex, v1: Vertex, px: __m256i, py: __m256i) -> __m256i {
    _mm256_sub_epi32(
        _mm256_mullo_epi32(
            _mm256_set1_epi32(v1.x - v0.x),
            _mm256_sub_epi32(py, _mm256_set1_epi32(v0.y)),
        ),
        _mm256_mullo_epi32(
            _mm256_set1_epi32(v1.y - v0.y),
            _mm256_sub_epi32(px, _mm256_set1_epi32(v0.x)),
        ),
    )
}

const VECTOR_DITHER_TABLE: &[[i16; 16]; 4] = &[
    [-4, 0, -3, 1, -4, 0, -3, 1, -4, 0, -3, 1, -4, 0, -3, 1],
    [2, -2, 3, -1, 2, -2, 3, -1, 2, -2, 3, -1, 2, -2, 3, -1],
    [-3, 1, -4, 0, -3, 1, -4, 0, -3, 1, -4, 0, -3, 1, -4, 0],
    [3, -1, 2, -2, 3, -1, 2, -2, 3, -1, 2, -2, 3, -1, 2, -2],
];

#[derive(Debug, Clone, Copy)]
struct Step {
    x: i32,
    y: i32,
}

impl Step {
    const ZERO: Self = Self { x: 0, y: 0 };

    fn new(v: &[Vertex], component: &[u8], denominator: i32) -> Self {
        Self {
            x: compute_x_step(v, component, denominator),
            y: compute_y_step(v, component, denominator),
        }
    }
}

#[derive(Debug, Clone)]
struct Interpolator {
    base_x: i32,
    base_y: i32,
    base_r: i32,
    base_g: i32,
    base_b: i32,
    base_u: i32,
    base_v: i32,
    r_step: Step,
    g_step: Step,
    b_step: Step,
    u_step: Step,
    v_step: Step,
}

impl Interpolator {
    // PS1 GPU appears to use fixed-point decimal with 12 fractional bits
    // U/V interpolation does not look correct otherwise
    const SHIFT: u8 = 12;

    #[target_feature(enable = "avx2")]
    unsafe fn new(
        v: &[Vertex],
        denominator: i32,
        gourad_shading: bool,
        colors: &[Colour],
        texture_mapping: Option<&[[u16; 2]]>,
    ) -> Self {
        let (base_vertex_idx, base_vertex) = v
            .into_iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.x.cmp(&b.x).then(a.y.cmp(&b.y)))
            .unwrap();

        let base_color = if gourad_shading {
            colors[base_vertex_idx]
        } else {
            colors[0]
        };

        let (r_step, g_step, b_step) = if gourad_shading {
            let r: Vec<_> = colors.iter().map(|color| color.r).collect();
            let g: Vec<_> = colors.iter().map(|color| color.g).collect();
            let b: Vec<_> = colors.iter().map(|color| color.b).collect();

            (
                Step::new(v, &r, denominator),
                Step::new(v, &g, denominator),
                Step::new(v, &b, denominator),
            )
        } else {
            (Step::ZERO, Step::ZERO, Step::ZERO)
        };

        let (base_tex_coords, u_step, v_step) = match texture_mapping {
            Some(mapping) => {
                let base_tex_coords = (
                    mapping[base_vertex_idx][0] as u8,
                    mapping[base_vertex_idx][1] as u8,
                );

                let us: Vec<u8> = mapping.iter().map(|x| x[0] as u8).collect();
                let vs: Vec<u8> = mapping.iter().map(|x| x[1] as u8).collect();

                // let us = us.try_into().expect("ok");
                // let vs = vs.try_into().expect("ok");

                (
                    base_tex_coords,
                    Step::new(v, &us, denominator),
                    Step::new(v, &vs, denominator),
                )
            }
            None => ((0, 0), Step::ZERO, Step::ZERO),
        };

        Self {
            base_x: base_vertex.x,
            base_y: base_vertex.y,
            base_r: shift_base_component(base_color.r),
            base_g: shift_base_component(base_color.g),
            base_b: shift_base_component(base_color.b),
            base_u: shift_base_component(base_tex_coords.0),
            base_v: shift_base_component(base_tex_coords.1),
            r_step,
            g_step,
            b_step,
            u_step,
            v_step,
        }
    }

    // Compute the interpolated colors for the given points.
    // Input vectors should be i32x8.
    // Return values are R/G/B color components as i32x8 vectors.
    #[target_feature(enable = "avx2")]
    unsafe fn interpolate_color(&self, px: __m256i, py: __m256i) -> (__m256i, __m256i, __m256i) {
        let r = interpolate_component(px, py, self.base_x, self.base_y, self.base_r, self.r_step);
        let g = interpolate_component(px, py, self.base_x, self.base_y, self.base_g, self.g_step);
        let b = interpolate_component(px, py, self.base_x, self.base_y, self.base_b, self.b_step);

        (r, g, b)
    }

    // Compute the interpolated U/V coordinates for the given points.
    // Input vectors should be i32x8.
    // Return values are U/V coordinates as i32x8 vectors.
    #[target_feature(enable = "avx2")]
    unsafe fn interpolate_uv(&self, px: __m256i, py: __m256i) -> (__m256i, __m256i) {
        let u = interpolate_component(px, py, self.base_x, self.base_y, self.base_u, self.u_step);
        let v = interpolate_component(px, py, self.base_x, self.base_y, self.base_v, self.v_step);

        (u, v)
    }
}

fn shift_base_component(component: u8) -> i32 {
    (i32::from(component) << Interpolator::SHIFT) | (1 << (Interpolator::SHIFT - 1))
}

fn compute_x_step(v: &[Vertex], component: &[u8], denominator: i32) -> i32 {
    let component: Vec<_> = component.iter().map(|x| *x as i32).collect();
    let raw = component[0] * (v[1].y - v[2].y)
        + component[1] * (v[2].y - v[0].y)
        + component[2] * (v[0].y - v[1].y);
    (raw << Interpolator::SHIFT) / denominator
}

fn compute_y_step(v: &[Vertex], component: &[u8], denominator: i32) -> i32 {
    let component: Vec<_> = component.iter().map(|x| *x as i32).collect();
    let raw = component[0] * (v[2].x - v[1].x)
        + component[1] * (v[0].x - v[2].x)
        + component[2] * (v[1].x - v[0].x);
    (raw << Interpolator::SHIFT) / denominator
}

#[target_feature(enable = "avx2")]
unsafe fn interpolate_component(
    px: __m256i,
    py: __m256i,
    base_x: i32,
    base_y: i32,
    base_component: i32,
    step: Step,
) -> __m256i {
    let dx = _mm256_sub_epi32(px, _mm256_set1_epi32(base_x));
    let dy = _mm256_sub_epi32(py, _mm256_set1_epi32(base_y));

    let value_fixedpoint = _mm256_add_epi32(
        _mm256_set1_epi32(base_component),
        _mm256_add_epi32(
            _mm256_mullo_epi32(dx, _mm256_set1_epi32(step.x)),
            _mm256_mullo_epi32(dy, _mm256_set1_epi32(step.y)),
        ),
    );

    mask_epi32_to_u8(_mm256_srli_epi32::<{ Interpolator::SHIFT as i32 }>(
        value_fixedpoint,
    ))
}

#[target_feature(enable = "avx2")]
unsafe fn mask_epi32_to_u8(v: __m256i) -> __m256i {
    _mm256_and_si256(v, _mm256_set1_epi32(0xFF))
}
