use std::{
    cmp,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};

use crossbeam::channel::{Receiver, Sender};

use crate::{
    Color,
    gpu::{
        Colour, DisplayDepth, HorizontalRes, TextureDepth, Vertex, VerticalRes,
        convert_5bit_to_8bit,
    },
};

pub enum RendererMsg {
    DrawLine {
        v0: Vertex,
        v1: Vertex,
        c0: Colour,
        semi_transparent: bool,
        ctx: RenderingContext,
    },
    DrawLineShaded {
        v0: Vertex,
        v1: Vertex,
        c0: Colour,
        c1: Colour,
        semi_transparent: bool,
        ctx: RenderingContext,
    },
    DrawRectangle {
        v: Vertex,
        side: Vertex,
        c: Colour,
        semi_transparent: bool,
        ctx: RenderingContext,
    },
    DrawRectangleTextured {
        v: Vertex,
        side: Vertex,
        c: Colour,
        clut: u16,
        _u: u16,
        _v: u16,
        semi_transparent: bool,
        blend: bool,
        ctx: RenderingContext,
    },
    DrawTriangleMono {
        c: Colour,
        v0: Vertex,
        v1: Vertex,
        v2: Vertex,
        semi_transparent: bool,
        ctx: RenderingContext,
    },
    DrawTriangleShaded {
        vs: [Vertex; 3],
        cs: [Colour; 3],
        semi_transparent: bool,
        ctx: RenderingContext,
    },
    DrawTriangleTextured {
        color: Colour,
        clut: u16,
        page: u16,
        vs: [Vertex; 3],
        uvs: [[u16; 2]; 3],
        semi_transparent: bool,
        blend: bool,
        ctx: RenderingContext,
    },
    DrawTriangleTexturedShaded {
        cs: [Colour; 3],
        clut: u16,
        page: u16,
        vs: [Vertex; 3],
        uvs: [[u16; 2]; 3],
        semi_transparent: bool,
        blend: bool,
        ctx: RenderingContext,
    },
    FillRect {
        v: Vertex,
        side: Vertex,
        c: Colour,
        ctx: RenderingContext,
    },
    Vram2VramBlit {
        src: Vertex,
        dst: Vertex,
        size: Vertex,
    },
    RenderFB {
        framebuffer: Arc<Mutex<Vec<Color>>>,
        full_ram: bool,
        ctx: RenderingContext,
    },
    CpuToVramCopy {
        top_left: (u16, u16),
        size: (u16, u16),
        data: Vec<u32>,
    },
    VramToCpuCopy {
        top_left: (u16, u16),
        size: (u16, u16),
    },
}
pub enum RendererResponse {
    FBUpdated { width: usize, height: usize },
    VramToCpuData { data: Vec<u32> },
}

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

pub struct Renderer {
    vram: Box<[u8]>,
    // to_gpu_sender: Sender<RendererResponse>,
}

impl Renderer {
    pub fn create() -> (
        Sender<RendererMsg>,
        Receiver<RendererResponse>,
        JoinHandle<()>,
    ) {
        let (to_gpu_sender, gpu_receiver) = crossbeam::channel::bounded(4096);
        let (to_renderer_sender, receiver) = crossbeam::channel::bounded(4096);

        let handle = thread::spawn(move || {
            let mut renderer = Renderer {
                vram: vec![0; 2 * 1024 * 512].into_boxed_slice(),
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
                        semi_transparent,
                        ctx,
                    } => {
                        if *semi_transparent {
                            renderer.draw_line::<SEMI_TRANS>(*v0, *v1, *c0, ctx)
                        } else {
                            renderer.draw_line::<OPAQUE>(*v0, *v1, *c0, ctx)
                        }
                    }
                    RendererMsg::DrawLineShaded {
                        v0,
                        v1,
                        c0,
                        c1,
                        semi_transparent,
                        ctx,
                    } => 
                        if *semi_transparent {
                            renderer.draw_line_shaded::<SEMI_TRANS>(*v0, *v1, *c0, *c1, ctx)
                        } else {
                            renderer.draw_line_shaded::<OPAQUE>(*v0, *v1, *c0, *c1, ctx)
                        },
                    RendererMsg::DrawRectangle {
                        v,
                        side,
                        c,
                        semi_transparent,
                        ctx,
                    } => if *semi_transparent {
                             renderer.draw_rectangle::<SEMI_TRANS>(*v, *side, *c, ctx);
                         } else {
                             renderer.draw_rectangle::<OPAQUE>(*v, *side, *c, ctx);
                         },
                    RendererMsg::DrawRectangleTextured {
                        v,
                        side,
                        c,
                        clut,
                        _u,
                        _v,
                        semi_transparent,
                        blend,
                        ctx,
                    } => 
                    match (*semi_transparent, *blend) {
                        (true, true) => renderer.draw_rectangle_textured::<SEMI_TRANS, BLEND>( *v, *side, *c, *clut,  [*_u, *_v],   ctx),
                        (true, false) => renderer.draw_rectangle_textured::<SEMI_TRANS, RAW>( *v, *side, *c, *clut,  [*_u, *_v],   ctx),
                        (false, true) => renderer.draw_rectangle_textured::<OPAQUE, BLEND>( *v, *side, *c, *clut,  [*_u, *_v],   ctx),
                        (false, false) => renderer.draw_rectangle_textured::<OPAQUE, RAW>( *v, *side, *c, *clut,  [*_u, *_v],   ctx),
                    },
                    RendererMsg::DrawTriangleMono {
                        c,
                        v0,
                        v1,
                        v2,
                        semi_transparent,
                        ctx,
                    } => if *semi_transparent {
                        renderer.render_triangle_mono::<SEMI_TRANS>(*c, &mut [*v0, *v1, *v2], ctx);
                    } else {
                        renderer.render_triangle_mono::<OPAQUE>(*c, &mut [*v0, *v1, *v2], ctx);
                    },
                    RendererMsg::DrawTriangleShaded {
                        vs,
                        cs,
                        semi_transparent,
                        ctx,
                    } => if *semi_transparent {
                        renderer.draw_triangle_shaded::<SEMI_TRANS>(vs, cs, ctx);
                    } else {
                        renderer.draw_triangle_shaded::<OPAQUE>(vs, cs, ctx);
                    }, 
                    RendererMsg::DrawTriangleTextured {
                        color,
                        clut,
                        page,
                        vs,
                        uvs,
                        semi_transparent,
                        blend,
                        ctx,
                    } => 
                    match (*semi_transparent, *blend) {
                        (true, true) => renderer.draw_triangle_textured::<SEMI_TRANS, BLEND>( *color, *clut, *page,  vs,  uvs, ctx),
                        (true, false) => renderer.draw_triangle_textured::<SEMI_TRANS, RAW>( *color, *clut, *page,  vs,  uvs, ctx),
                        (false, true) => renderer.draw_triangle_textured::<OPAQUE, BLEND>( *color, *clut, *page,  vs,  uvs, ctx),
                        (false, false) => renderer.draw_triangle_textured::<OPAQUE, RAW>( *color, *clut, *page,  vs,  uvs, ctx),
                    },
                    RendererMsg::DrawTriangleTexturedShaded {
                        cs,
                        clut,
                        page,
                        vs,
                        uvs,
                        semi_transparent,
                        blend,
                        ctx,
                    } => 
                    match (*semi_transparent, *blend) {
                        (true, true) => renderer.draw_triangle_textured_shaded::<SEMI_TRANS, BLEND>( cs, *clut, *page,  vs,  uvs, ctx),
                        (true, false) => renderer.draw_triangle_textured_shaded::<SEMI_TRANS, RAW>( cs, *clut, *page,  vs,  uvs, ctx),
                        (false, true) => renderer.draw_triangle_textured_shaded::<OPAQUE, BLEND>( cs, *clut, *page,  vs,  uvs, ctx),
                        (false, false) => renderer.draw_triangle_textured_shaded::<OPAQUE, RAW>( cs, *clut, *page,  vs,  uvs, ctx),
                    },
                    RendererMsg::FillRect { v, side, c, ctx } => renderer.fill_rect(*v, *side, *c),
                    RendererMsg::Vram2VramBlit { src, dst, size } => renderer.vram2vram_blit(*src, *dst, *size),
                    RendererMsg::RenderFB {
                        framebuffer,
                        full_ram,
                        ctx,
                    } => {
                        let (width,height) = renderer.render_fb(&framebuffer, *full_ram, ctx);
                        match to_gpu_sender.send(RendererResponse::FBUpdated { width, height }) {
                            Ok(_) => (),
                            Err(_) => return,
                        }
                    },
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
                    },
                }
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

        for x in min_x..max_x {
            for y in min_y..max_y {
                let vram_addr = 2 * (y * 1024 + x) as usize;

                let [pixel_lsb, pixel_msb] = color.to_le_bytes();
                self.vram[vram_addr] = pixel_lsb;
                self.vram[vram_addr + 1] = pixel_msb&0xEF;
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
    pub fn render_triangle_mono<const SEMI_TRANS: bool>(
        &mut self,
        mono: Colour,
        vs: &mut [Vertex; 3],
        ctx: &RenderingContext,
    ) {
        ensure_vertex_order(vs);
        // let [pixel_lsb, pixel_msb] = color;
        let [v0, v1, v2] = vs;

        // bounding box
        let mut min_x = cmp::min(v0.x, cmp::min(v1.x, v2.x));
        let mut max_x = cmp::max(v0.x, cmp::max(v1.x, v2.x));
        let mut min_y = cmp::min(v0.y, cmp::min(v1.y, v2.y));
        let mut max_y = cmp::max(v0.y, cmp::max(v1.y, v2.y));

        let Some((min_x, min_y, max_x, max_y)) = self.clip_rect(min_x, min_y, max_x, max_y, ctx) else {
            return;
        };

        // clip pixels outside of the drawing area
        // min_x = cmp::max(min_x, ctx.drawing_area_left as i32);
        // max_x = cmp::min(max_x, ctx.drawing_area_right as i32);
        // min_y = cmp::max(min_y, ctx.drawing_area_top as i32);
        // max_y = cmp::min(max_y, ctx.drawing_area_bottom as i32);

        for y in min_y..max_y {
            for x in min_x..max_x {
                let p = Vertex { x, y };
                if is_inside_triangle(p, *v0, *v1, *v2) {
                    let vram_addr = 2 * (y * 1024 + x) as usize;
                    let mut color = mono;

                    if SEMI_TRANS {
                        let background_lsb = self.vram[vram_addr];
                        let background_msb = self.vram[vram_addr + 1];

                        let bg = Colour::from_bytes(background_msb, background_lsb);

                        color.blend_with_background(bg, ctx.semi_transparency);
                    }
                    self.vram_write_color(vram_addr, color, ctx);
                }
            }
        }

    }

    pub fn draw_triangle_shaded<const SEMI_TRANS: bool>(
        &mut self,
        vs: &mut [Vertex; 3],
        colors: &mut [Colour; 3],
        ctx: &RenderingContext,
    ) {
        ensure_vertex_order2(vs, colors);
        vs[0].x += ctx.drawing_x_offset as i32;
        vs[0].y += ctx.drawing_y_offset as i32;
        vs[1].x += ctx.drawing_x_offset as i32;
        vs[1].y += ctx.drawing_y_offset as i32;
        vs[2].x += ctx.drawing_x_offset as i32;
        vs[2].y += ctx.drawing_y_offset as i32;
        let [v0, v1, v2] = vs;
        let [c0, c1, c2] = colors;

        // bounding box
        let mut min_x = cmp::min(v0.x, cmp::min(v1.x, v2.x));
        let mut max_x = cmp::max(v0.x, cmp::max(v1.x, v2.x));
        let mut min_y = cmp::min(v0.y, cmp::min(v1.y, v2.y));
        let mut max_y = cmp::max(v0.y, cmp::max(v1.y, v2.y));

        let Some((min_x, min_y, max_x, max_y)) = self.clip_rect(min_x, min_y, max_x, max_y, ctx) else {
            return;
        };

        // clip pixels outside of the drawing area
        // min_x = cmp::max(min_x, ctx.drawing_area_left as i32);
        // max_x = cmp::min(max_x, ctx.drawing_area_right as i32);
        // min_y = cmp::max(min_y, ctx.drawing_area_top as i32);
        // max_y = cmp::min(max_y, ctx.drawing_area_bottom as i32);

        for y in min_y..max_y {
            for x in min_x..max_x {
                let p = Vertex { x, y };
                if is_inside_triangle(p, *v0, *v1, *v2) {
                    let lambda = compute_barycentric_coordinates(p, *v0, *v1, *v2);
                    let color = interpolate_color(lambda, [*c0, *c1, *c2]);
                    let mut color = apply_dithering(color, p);
                    let vram_addr = 2 * (y * 1024 + x) as usize;

                    if SEMI_TRANS {
                        let background_lsb = self.vram[vram_addr];
                        let background_msb = self.vram[vram_addr + 1];

                        let bg = Colour::from_bytes(background_msb, background_lsb);

                        color.blend_with_background(bg, ctx.semi_transparency);
                    }

                    self.vram_write_color(vram_addr, color, ctx);
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

    pub fn draw_triangle_textured<const SEMI_TRANS: bool, const BLEND: bool>(
        &mut self,
        mono: Colour,
        clut: u16,
        page: u16,
        vs: &mut [Vertex; 3],
        uv: &mut [[u16; 2]; 3],
        ctx: &RenderingContext,
    ) {
        ensure_vertex_order2(vs, uv);
        vs[0].x += ctx.drawing_x_offset as i32;
        vs[0].y += ctx.drawing_y_offset as i32;
        vs[1].x += ctx.drawing_x_offset as i32;
        vs[1].y += ctx.drawing_y_offset as i32;
        vs[2].x += ctx.drawing_x_offset as i32;
        vs[2].y += ctx.drawing_y_offset as i32;

        // bounding box
        let mut min_x = cmp::min(vs[0].x, cmp::min(vs[1].x, vs[2].x));
        let mut max_x = cmp::max(vs[0].x, cmp::max(vs[1].x, vs[2].x));
        let mut min_y = cmp::min(vs[0].y, cmp::min(vs[1].y, vs[2].y));
        let mut max_y = cmp::max(vs[0].y, cmp::max(vs[1].y, vs[2].y));

        let Some((min_x, min_y, max_x, max_y)) = self.clip_rect(min_x, min_y, max_x, max_y, ctx) else {
            return;
        };

        // clip pixels outside of the drawing area
        // min_x = cmp::max(min_x, ctx.drawing_area_left as i32);
        // max_x = cmp::min(max_x, ctx.drawing_area_right as i32);
        // min_y = cmp::max(min_y, ctx.drawing_area_top as i32);
        // max_y = cmp::min(max_y, ctx.drawing_area_bottom as i32);

        let clut = Clut::new(clut);
        let texture = Texture::new(page, clut);

        for y in min_y..max_y {
            for x in min_x..max_x {
                let p = Vertex { x, y };
                if is_inside_triangle(p, vs[0], vs[1], vs[2]) {
                    let lambda = compute_barycentric_coordinates(p, vs[0], vs[1], vs[2]);
                    let [uv_x, uv_y] = compute_normal_coordinates(lambda, uv);

                    let mut pixel = texture.get_texel(self, uv_x, uv_y, ctx);

                    if pixel.is_black() {
                        continue;
                    }

                    if BLEND {
                        pixel.blend(mono);
                    }

                    let vram_addr = 2 * (y * 1024 + x) as usize;

                    if SEMI_TRANS && pixel.m == 1 {
                        let background_lsb = self.vram[vram_addr];
                        let background_msb = self.vram[vram_addr + 1];

                        let background = Colour::from_bytes(background_msb, background_lsb);

                        pixel.blend_with_background(background, texture.semi_transparency);
                    }

                    if texture.dithering {
                        pixel.apply_dithering(x, y);
                    }

                    self.vram_write_color(vram_addr, pixel, ctx);
                    // let [lsb, msb] = pixel.to_le_bytes();

                    // self.vram[vram_addr] = lsb;
                    // self.vram[vram_addr + 1] = msb;
                }
            }
        }

        // let [pixel_lsb, pixel_msb] = Self::gp0_color(self.gp0_command[0]);
    }

    pub fn draw_triangle_textured_shaded<const SEMI_TRANS: bool, const BLEND: bool>(
        &mut self,
        colors: &mut [Colour; 3],
        clut: u16,
        page: u16,
        vs: &mut [Vertex; 3],
        uv: &mut [[u16; 2]; 3],
        ctx: &RenderingContext,
    ) {
        ensure_vertex_order3(vs, uv, colors);
        vs[0].x += ctx.drawing_x_offset as i32;
        vs[0].y += ctx.drawing_y_offset as i32;
        vs[1].x += ctx.drawing_x_offset as i32;
        vs[1].y += ctx.drawing_y_offset as i32;
        vs[2].x += ctx.drawing_x_offset as i32;
        vs[2].y += ctx.drawing_y_offset as i32;

        // bounding box
        let mut min_x = cmp::min(vs[0].x, cmp::min(vs[1].x, vs[2].x));
        let mut max_x = cmp::max(vs[0].x, cmp::max(vs[1].x, vs[2].x));
        let mut min_y = cmp::min(vs[0].y, cmp::min(vs[1].y, vs[2].y));
        let mut max_y = cmp::max(vs[0].y, cmp::max(vs[1].y, vs[2].y));

        let Some((min_x, min_y, max_x, max_y)) = self.clip_rect(min_x, min_y, max_x, max_y, ctx) else {
            return;
        };

        // clip pixels outside of the drawing area
        // min_x = cmp::max(min_x, ctx.drawing_area_left as i32);
        // max_x = cmp::min(max_x, ctx.drawing_area_right as i32);
        // min_y = cmp::max(min_y, ctx.drawing_area_top as i32);
        // max_y = cmp::min(max_y, ctx.drawing_area_bottom as i32);

        let clut = Clut::new(clut);
        let texture = Texture::new(page, clut);

        for y in min_y..max_y {
            for x in min_x..max_x {
                let p = Vertex { x, y };
                if is_inside_triangle(p, vs[0], vs[1], vs[2]) {
                    let lambda = compute_barycentric_coordinates(p, vs[0], vs[1], vs[2]);
                    let [uv_x, uv_y] = compute_normal_coordinates(lambda, uv);

                    let mut pixel = texture.get_texel(self, uv_x, uv_y, ctx);

                    if pixel.is_black() {
                        continue;
                    }

                    if BLEND {
                        let color = interpolate_color(lambda, *colors);
                        pixel.blend(color);
                    }

                    let vram_addr = 2 * (y * 1024 + x) as usize;

                    if SEMI_TRANS && pixel.m == 1 {
                        let background_lsb = self.vram[vram_addr];
                        let background_msb = self.vram[vram_addr + 1];

                        let background = Colour::from_bytes(background_msb, background_lsb);

                        pixel.blend_with_background(background, texture.semi_transparency);
                    }

                    if texture.dithering {
                        pixel = apply_dithering(pixel, p);
                    }

                    self.vram_write_color(vram_addr, pixel, ctx);
                    // let [lsb, msb] = pixel.to_le_bytes();
                    //
                    // self.vram[vram_addr] = lsb;
                    // self.vram[vram_addr + 1] = msb;
                }
            }
        }

        // let [pixel_lsb, pixel_msb] = Self::gp0_color(self.gp0_command[0]);
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
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let vram_addr = 2 * (y * 1024 + x) as usize;
                let mut pixel = color;

                if SEMI_TRANS {
                    let background_lsb = self.vram[vram_addr];
                    let background_msb = self.vram[vram_addr + 1];

                    let background = Colour::from_bytes(background_msb, background_lsb);

                    pixel.blend_with_background(background, ctx.semi_transparency);
                }

                self.vram_write_color(vram_addr, pixel, ctx);
            }
        }
    }
    pub fn draw_rectangle_textured<const SEMI_TRANS: bool, const BLEND: bool>(
        &mut self,
        mut v: Vertex,
        side: Vertex,
        color: Colour,
        clut: u16,
        uv: [u16; 2],
        ctx: &RenderingContext,
    ) {
        v.x += ctx.drawing_x_offset as i32;
        v.y += ctx.drawing_y_offset as i32;

        let Some((min_x, min_y, max_x, max_y)) =
            self.clip_rect(v.x, v.y, v.x + side.x - 1, v.y + side.y - 1, ctx)
        else {
            return;
        };

        let clut = Clut::new(clut);
        let texture = Texture {
            base_x: (ctx.page_base_x as usize) * 64,
            base_y: (ctx.page_base_y as usize) * 256,
            semi_transparency: ctx.semi_transparency,
            depth: ctx.texture_depth,
            clut,
            dithering: ctx.dithering,
            draw_to_display: ctx.draw_to_display,
        };

        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let uv_x = (uv[0] as i32) + (x as i32) - v.x;
                let uv_y = (uv[1] as i32) + (y as i32) - v.y;

                let mut pixel = texture.get_texel(self, uv_x as usize, uv_y as usize, ctx);
                if pixel.is_black() {
                    continue;
                }

                if BLEND {
                    pixel.blend(color);
                }

                let vram_addr = 2 * (y * 1024 + x) as usize;

                if SEMI_TRANS && pixel.m == 1 {
                    let background_lsb = self.vram[vram_addr];
                    let background_msb = self.vram[vram_addr + 1];

                    let background = Colour::from_bytes(background_msb, background_lsb);

                    pixel.blend_with_background(background, ctx.semi_transparency);
                }

                self.vram_write_color(vram_addr, pixel, ctx);
            }
        }
    }

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

    pub fn render_fb(
        &self,
        framebuffer: &Mutex<Vec<Color>>,
        full_ram: bool,
        ctx: &RenderingContext,
    ) -> (usize, usize) {
        let mut mutex = framebuffer.lock().unwrap();
        let output_frame_buffer: &mut [Color] = mutex.as_mut();

        if ctx.display_disabled {
            for y in 0..16 {
                for x in 0..16 {
                    output_frame_buffer[1024 * y + x] = Color { r: 0, g: 0, b: 0, a: 255 };
                }
            }
            return (16, 16);
        }
        if full_ram {
            for y in 0..512 {
                for x in 0..1024 {
                    let vram_addr = 2 * (1024 * y + x);
                    let pixel =
                        u16::from_le_bytes([self.vram[vram_addr], self.vram[vram_addr + 1]]);

                    let r = convert_5bit_to_8bit(pixel & 0x1F);
                    let g = convert_5bit_to_8bit((pixel >> 5) & 0x1F);
                    let b = convert_5bit_to_8bit((pixel >> 10) & 0x1F);

                    output_frame_buffer[1024 * y + x] = Color { r, g, b, a: 255 };
                }
            }
            (1024, 512)
        } else {
            let (sx, sy, width, height, interlaced) = (
                ctx.display_vram_x_start as usize,
                ctx.display_vram_y_start as usize,
                ctx.hres.into_pixels(),
                ctx.vres.into_pixels(),
                ctx.interlaced,
            );
            match ctx.display_depth {
                DisplayDepth::D15Bits => {
                    for y in 0..height {
                        for x in 0..width {
                            let vram_addr = 2 * (1024 * (sy + y) + (sx + x));
                            let pixel = u16::from_le_bytes([
                                self.vram[vram_addr],
                                self.vram[vram_addr + 1],
                            ]);

                            let r = convert_5bit_to_8bit(pixel & 0x1F);
                            let g = convert_5bit_to_8bit((pixel >> 5) & 0x1F);
                            let b = convert_5bit_to_8bit((pixel >> 10) & 0x1F);

                            output_frame_buffer[1024 * y + x] = Color { r, g, b, a: 255 };
                        }
                    }
                }
                DisplayDepth::D24Bits => {
                    for y in 0..height {
                        for x in 0..width {
                            let vram_addr = 2 * (1024 * (y + sy)) + 3 * sx + 3 * x;
                            let r = self.vram[vram_addr];
                            let g = self.vram[vram_addr + 1];
                            let b = self.vram[vram_addr + 2];

                            output_frame_buffer[1024 * y + x] = Color { r, g, b, a: 255 };
                        }
                    }
                }
            };
            // black out lines outside of display field (wrong impl)
            let vrange = (ctx.display_line_end - ctx.display_line_start) as usize;
            if height >= vrange {
                let starting_row = vrange * 1024 * if interlaced { 2 } else { 1 };
                output_frame_buffer[starting_row..].fill(Color { r: 0, g: 0, b: 0, a: 255});
            }
            (width, height)
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
fn ensure_vertex_order(vs: &mut [Vertex; 3]) {
    let cross_product_z =
        (vs[1].x - vs[0].x) * (vs[2].y - vs[0].y) - (vs[1].y - vs[0].y) * (vs[2].x - vs[0].x);
    if cross_product_z < 0 {
        vs.swap(0, 1);
        // std::mem::swap(v0, v1);
    }
}

fn ensure_vertex_order2<T: Clone>(vs: &mut [Vertex; 3], attrs: &mut [T; 3]) {
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
fn ensure_vertex_order3<T: Clone, E: Clone>(
    vs: &mut [Vertex; 3],
    attrs: &mut [T; 3],
    attrs1: &mut [E; 3],
) {
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
fn compute_normal_coordinates(p: [f64; 3], vs: &[[u16; 2]; 3]) -> [usize; 2] {
    let x = p[0] * (vs[0][0] as f64) + p[1] * (vs[1][0] as f64) + p[2] * (vs[2][0] as f64);
    let y = p[0] * (vs[0][1] as f64) + p[1] * (vs[1][1] as f64) + p[2] * (vs[2][1] as f64);
    [x.round() as usize, y.round() as usize]
}

#[derive(Debug, Clone, Copy)]
pub struct Clut {
    base_x: usize,
    base_y: usize,
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

#[derive(Debug, Clone, Copy)]
pub struct Texture {
    base_x: usize,
    base_y: usize,
    semi_transparency: u8,
    dithering: bool,
    draw_to_display: bool,
    depth: TextureDepth,
    clut: Clut,
}

impl Texture {
    pub fn new(data: u16, clut: Clut) -> Self {
        let base_x = ((data & 0xf) as usize) * 64; // n * 64
        let base_y = (((data >> 4) & 1) as usize) * 256; // n * 256
        let semi_transparency = ((data >> 5) & 3) as u8;
        let dithering = (data >> 9) & 1 != 0;
        let draw_to_display = (data >> 10) & 1 != 0;
        // let depth = ((data >> 7) & 3) as u8;

        let depth = match (data >> 7) & 3 {
            0 => TextureDepth::T4Bit,
            1 => TextureDepth::T8Bit,
            2 => TextureDepth::T15Bit,
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

fn interpolate_color(lambda: [f64; 3], colors: [Colour; 3]) -> Colour {
    let colors_r: [f64; 3] = colors.map(|c| f64::from(c.r));
    let colors_g = colors.map(|c| f64::from(c.g));
    let colors_b = colors.map(|c| f64::from(c.b));

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
