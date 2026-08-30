use std::cmp;
use wgpu::CurrentSurfaceTexture;

use crate::{
    Color,
    memory_bus::{AccessWidth, Addressable},
};

pub struct Gpu {
    // Texture page base X coordinate (4 bits, 64 byte increment)
    page_base_x: u8,
    // Texture page base Y coordinate (1 bit, 256 line increment)
    page_base_y: u8,
    semi_transparency: u8, // Semi Transparency     (0=B/2+F/2, 1=B+F, 2=B-F, 3=B+F/4)
    texture_depth: TextureDepth,
    // dithering from 24 to 16 bits RGB
    dithering: bool,
    draw_to_display: bool,
    // force "mask" bit of the pixel to 1 when writing to VRAM
    force_set_mask_bit: bool,
    // don't draw to pixels which have the "mask" bit set
    preserve_masked_pixels: bool,
    // currently displayed field. For progressive output this is always Top
    field: Field,
    texture_disable: bool,
    hres: HorizontalRes,
    vres: VerticalRes,
    vmode: VMode,
    // gpu itself always draws 15bit RGB, 24bit output must use external assets
    display_depth: DisplayDepth,
    interlaced: bool,
    display_disabled: bool,
    pub interrupt: bool,
    dma_direction: DmaDirection,

    rectange_texture_x_flip: bool,
    rectange_texture_y_flip: bool,

    texture_window_x_mask: u8,
    texture_window_y_mask: u8,
    texture_window_x_offset: u8,
    texture_window_y_offset: u8,
    drawing_area_left: u16,
    drawing_area_top: u16,
    drawing_area_right: u16,
    drawing_area_bottom: u16,
    drawing_x_offset: i16,
    drawing_y_offset: i16,
    display_vram_x_start: u16,
    display_vram_y_start: u16,
    display_horiz_start: u16,
    display_horiz_end: u16,
    display_line_start: u16,
    display_line_end: u16,

    gp0_command: CommandBuffer,
    gp0_words_remaining: u32,
    gp0_command_method: fn(&mut Gpu),
    gp0_mode: Gp0Mode,
    vram: Box<[u8]>,
    even: bool,
    in_hblank: bool,
    in_vblank: bool,
    read: u32,
}

impl Gpu {
    pub fn new() -> Gpu {
        Gpu {
            page_base_x: 0,
            page_base_y: 0,
            semi_transparency: 0,
            texture_depth: TextureDepth::T4Bit,
            dithering: false,
            draw_to_display: false,
            force_set_mask_bit: false,
            preserve_masked_pixels: false,
            field: Field::Top,
            texture_disable: false,
            hres: HorizontalRes::from_fields(0, 0),
            vres: VerticalRes::Y240Lines,
            vmode: VMode::Ntsc,
            display_depth: DisplayDepth::D15Bits,
            interlaced: false,
            display_disabled: true,
            interrupt: false,
            dma_direction: DmaDirection::Off,
            rectange_texture_x_flip: false,
            rectange_texture_y_flip: false,
            texture_window_x_mask: 0,
            texture_window_y_mask: 0,
            texture_window_x_offset: 0,
            texture_window_y_offset: 0,
            drawing_area_left: 0,
            drawing_area_top: 0,
            drawing_area_right: 0,
            drawing_area_bottom: 0,
            drawing_x_offset: 0,
            drawing_y_offset: 0,
            display_vram_x_start: 0,
            display_vram_y_start: 0,
            display_horiz_start: 0,
            display_horiz_end: 0,
            display_line_start: 0,
            display_line_end: 0,

            gp0_command: CommandBuffer::new(),
            gp0_words_remaining: 0,
            gp0_command_method: Gpu::gp0_nop,
            gp0_mode: Gp0Mode::Command,
            vram: vec![0; 2 * 1024 * 512].into_boxed_slice(),
            even: true,
            in_vblank: false,
            in_hblank: false,
            read: 0,
        }
    }

    pub fn get_command(val: u32) -> u32 {
        val >> 29
    }

    pub fn get_shading(val: u32) -> bool {
        val & (0x1 << 28) != 0
    }
    pub fn get_vertices(val: u32) -> bool {
        val & (0x1 << 27) != 0
    }
    pub fn get_textured(val: u32) -> bool {
        val & (0x1 << 26) != 0
    }

    pub fn get_blend_mode(val: u32) -> bool {
        val & (0x1 << 25) != 0
    }

    pub fn get_modulation(val: u32) -> bool {
        val & (0x1 << 24) != 0
    }

    pub fn gp0(&mut self, val: u32) {
        if self.gp0_words_remaining == 0 {
            let opcode = (val >> 24) & 0xff;
            let (len, method): (u32, fn(&mut Gpu)) = match opcode {
                0x00 => (1, Gpu::gp0_nop),
                0x01 => (1, Gpu::gp0_clear_cache),
                0x02 => (3, Gpu::gp0_fill_rect),
                0x03 => (1, Gpu::gp0_nop),
                0x04..=0x1E => (1, Gpu::gp0_nop),

                0x20 => (4, Self::gp0_poly_mono::<TRI, OPAQUE>),
                0x21 => (4, Self::gp0_poly_mono::<TRI, OPAQUE>),
                0x22 => (4, Self::gp0_poly_mono::<TRI, SEMI_TRANS>),
                0x23 => (4, Self::gp0_poly_mono::<TRI, SEMI_TRANS>),
                0x24 => (7, Self::gp0_poly_texture::<TRI, OPAQUE, BLEND>),
                0x25 => (7, Self::gp0_poly_texture::<TRI, OPAQUE, RAW>),
                0x26 => (7, Self::gp0_poly_texture::<TRI, SEMI_TRANS, BLEND>),
                0x27 => (7, Self::gp0_poly_texture::<TRI, SEMI_TRANS, RAW>),

                0x28 => (5, Gpu::gp0_poly_mono::<QUAD, OPAQUE>),
                0x29 => (5, Gpu::gp0_poly_mono::<QUAD, OPAQUE>),
                0x2A => (5, Gpu::gp0_poly_mono::<QUAD, SEMI_TRANS>),
                0x2B => (5, Gpu::gp0_poly_mono::<QUAD, SEMI_TRANS>),
                0x2C => (9, Gpu::gp0_poly_texture::<QUAD, OPAQUE, BLEND>),
                0x2D => (9, Gpu::gp0_poly_texture::<QUAD, OPAQUE, RAW>),
                0x2E => (9, Gpu::gp0_poly_texture::<QUAD, SEMI_TRANS, BLEND>),
                0x2F => (9, Gpu::gp0_poly_texture::<QUAD, SEMI_TRANS, RAW>),

                0x30 => (6, Gpu::gp0_poly_shaded::<TRI, OPAQUE>),
                0x31 => (6, Gpu::gp0_poly_shaded::<TRI, OPAQUE>),
                0x32 => (6, Gpu::gp0_poly_shaded::<TRI, SEMI_TRANS>),
                0x33 => (6, Gpu::gp0_poly_shaded::<TRI, SEMI_TRANS>),

                0x34 => (9, Self::gp0_poly_texture_shaded::<TRI, OPAQUE, BLEND>),
                0x35 => (9, Self::gp0_poly_texture_shaded::<TRI, OPAQUE, RAW>),
                0x36 => (9, Self::gp0_poly_texture_shaded::<TRI, SEMI_TRANS, BLEND>),
                0x37 => (9, Self::gp0_poly_texture_shaded::<TRI, SEMI_TRANS, RAW>),

                0x38 => (8, Gpu::gp0_poly_shaded::<QUAD, OPAQUE>),
                0x39 => (8, Gpu::gp0_poly_shaded::<QUAD, OPAQUE>),

                0x3A => (8, Gpu::gp0_poly_shaded::<QUAD, SEMI_TRANS>),
                0x3B => (8, Gpu::gp0_poly_shaded::<QUAD, SEMI_TRANS>),

                0x3C => (12, Gpu::gp0_poly_texture_shaded::<QUAD, OPAQUE, BLEND>),
                0x3D => (12, Gpu::gp0_poly_texture_shaded::<QUAD, OPAQUE, RAW>),
                0x3E => (12, Gpu::gp0_poly_texture_shaded::<QUAD, SEMI_TRANS, BLEND>),
                0x3F => (12, Gpu::gp0_poly_texture_shaded::<QUAD, SEMI_TRANS, RAW>),

                0x40 | 0x41 | 0x44 | 0x45 => (3, Self::gp0_line_mono::<OPAQUE>),
                0x42 | 0x43 | 0x46 | 0x47 => (3, Self::gp0_line_mono::<SEMI_TRANS>),
                0x50 | 0x51 => (4, Self::gp0_line_shaded::<OPAQUE>),
                0x52 | 0x53 => (4, Self::gp0_line_shaded::<SEMI_TRANS>),
                0x60 => (3, Self::gp0_rect_variable::<OPAQUE>),
                0x61 => (3, Self::gp0_rect_variable::<OPAQUE>),
                0x62 => (3, Self::gp0_rect_variable::<SEMI_TRANS>),
                0x63 => (3, Self::gp0_rect_variable::<SEMI_TRANS>),

                0x64 => (4, Self::gp0_rect_texture_variable::<OPAQUE, BLEND>),
                0x65 => (4, Self::gp0_rect_texture_variable::<OPAQUE, RAW>),
                0x66 => (4, Self::gp0_rect_texture_variable::<SEMI_TRANS, BLEND>),
                0x67 => (4, Self::gp0_rect_texture_variable::<SEMI_TRANS, RAW>),

                0x68 => (2, Gpu::gp0_rect_fixed::<1, OPAQUE>),
                0x69 => (2, Gpu::gp0_rect_fixed::<1, OPAQUE>),
                0x6A => (2, Gpu::gp0_rect_fixed::<1, SEMI_TRANS>),
                0x6B => (2, Gpu::gp0_rect_fixed::<1, SEMI_TRANS>),
                0x6C => (3, Self::gp0_rect_texture_fixed::<1, OPAQUE, BLEND>),
                0x6D => (3, Self::gp0_rect_texture_fixed::<1, OPAQUE, RAW>),
                0x6E => (3, Self::gp0_rect_texture_fixed::<1, SEMI_TRANS, BLEND>),
                0x6F => (3, Self::gp0_rect_texture_fixed::<1, SEMI_TRANS, RAW>),
                0x70 => (2, Gpu::gp0_rect_fixed::<8, OPAQUE>),
                0x71 => (2, Gpu::gp0_rect_fixed::<8, OPAQUE>),
                0x72 => (2, Gpu::gp0_rect_fixed::<8, SEMI_TRANS>),
                0x73 => (2, Gpu::gp0_rect_fixed::<8, SEMI_TRANS>),
                0x74 => (3, Self::gp0_rect_texture_fixed::<8, OPAQUE, BLEND>),
                0x75 => (3, Self::gp0_rect_texture_fixed::<8, OPAQUE, RAW>),
                0x76 => (3, Self::gp0_rect_texture_fixed::<8, SEMI_TRANS, BLEND>),
                0x77 => (3, Self::gp0_rect_texture_fixed::<8, SEMI_TRANS, RAW>),
                0x78 => (2, Gpu::gp0_rect_fixed::<16, OPAQUE>),
                0x79 => (2, Gpu::gp0_rect_fixed::<16, OPAQUE>),
                0x7A => (2, Gpu::gp0_rect_fixed::<16, SEMI_TRANS>),
                0x7B => (2, Gpu::gp0_rect_fixed::<16, SEMI_TRANS>),
                0x7C => (3, Self::gp0_rect_texture_fixed::<16, OPAQUE, BLEND>),
                0x7D => (3, Self::gp0_rect_texture_fixed::<16, OPAQUE, RAW>),
                0x7E => (3, Self::gp0_rect_texture_fixed::<16, SEMI_TRANS, BLEND>),
                0x7F => (3, Self::gp0_rect_texture_fixed::<16, SEMI_TRANS, RAW>),
                0x80 => (4, Self::gp0_vram_to_vram_blit),
                0xA0 => (3, Gpu::gp0_image_load),
                0xC0 => (3, Gpu::gp0_image_store),
                0xCA => {
                    println!("woot?");
                    (3, Gpu::gp0_image_store)
                }
                0xE0 => (1, Gpu::gp0_nop),
                0xE1 => (1, Gpu::gp0_draw_mode),
                0xE2 => (1, Gpu::gp0_texture_window),
                0xE3 => (1, Gpu::gp0_drawing_area_top_left),
                0xE4 => (1, Gpu::gp0_drawing_area_bottom_right),
                0xE5 => (1, Gpu::gp0_drawing_offset),
                0xE6 => (1, Gpu::gp0_mask_bit_setting),
                0xE7..=0xEF => (1, Self::gp0_nop),
                _ => {
                    let cmd = Self::get_command(val);
                    let shading = Self::get_shading(val);
                    let vertices = Self::get_vertices(val);
                    let textured = Self::get_textured(val);
                    let blend_mode = Self::get_blend_mode(val);
                    let modulation = Self::get_modulation(val);

                    println!(
                        "command: {:03b}, shading: {shading}, vertices: {vertices}, textured: {textured}, blend: {blend_mode}, modulation: {modulation}",
                        cmd
                    );

                    // std::thread::sleep(std::time::Duration::new(5, 0 ));
                    panic!(
                        "Unhandled GP0 command 0x{:08X} opcode: 0x{:02X}",
                        val, opcode
                    );
                }
            };
            self.gp0_words_remaining = len;
            self.gp0_command_method = method;
            self.gp0_command.clear();
        }
        self.gp0_words_remaining -= 1;
        match self.gp0_mode {
            Gp0Mode::Command => {
                self.gp0_command.push_word(val);
                if self.gp0_words_remaining == 0 {
                    (self.gp0_command_method)(self);
                }
            }
            Gp0Mode::ImageLoad {
                top_left,
                resolution,
                current_row,
                current_col,
            } => {
                self.process_cpu_to_vram_copy(val, top_left, resolution, current_row, current_col);
                if self.gp0_words_remaining == 0 {
                    self.gp0_mode = Gp0Mode::Command;
                }
            }
            Gp0Mode::ImageStore {
                top_left,
                resolution,
                current_row,
                current_col,
            } => {
                // self.gp0_command.clear();
                // self.gp0_words_remaining = 0;
                // self.gp0_mode = Gp0Mode::Command;
                // self.gp0(val);
            }
        }
    }

    pub fn process_cpu_to_vram_copy(
        &mut self,
        word: u32,
        top_left: (u16, u16),
        resolution: (u16, u16),
        curr_row: u16,
        curr_col: u16,
    ) {
        let mut current_row = curr_row;
        let mut current_col = curr_col;
        // 2 halfwords per GP0 write
        for i in 0..2 {
            let halfword = (word >> (16 * i)) as u16;

            let vram_row = ((top_left.1 + current_row) & 0x1FF) as usize;
            let vram_col = ((top_left.0 + current_col) & 0x3FF) as usize;

            let [lsb, msb] = halfword.to_le_bytes();
            let vram_addr = 2 * (1024 * vram_row + vram_col);

            self.vram[vram_addr] = lsb;
            self.vram[vram_addr + 1] = msb;

            current_col += 1;
            if current_col == resolution.0 {
                current_col = 0;
                current_row += 1;
                // TODO: maybe return from here?
            }
        }
        self.gp0_mode = Gp0Mode::ImageLoad {
            top_left,
            resolution,
            current_row,
            current_col,
        }
    }

    pub fn gp0_colour(color: u32) -> Colour {
        let r = (color & 0xFF) as u8;
        let g = ((color >> 8) & 0xFF) as u8;
        let b = ((color >> 16) & 0xFF) as u8;
        Colour { r, g, b, m: 0 }
    }
    // pub fn gp0_color(color: u32) -> [u8; 2] {
    //     let r = (color & 0xFF) >> 3;
    //     let g = ((color >> 8) & 0xFF) >> 3;
    //     let b = ((color >> 16) & 0xFf) >> 3;
    //
    //     let pixel = (r | (g << 5) | (b << 10)) as u16;
    //     pixel.to_le_bytes()
    // }
    pub fn gp0_vertex(v: u32) -> Vertex {
        let x = ((v & 0xFFFF) as i16) << 5 >> 5;
        let y = (((v >> 16) & 0xFFFF) as i16) << 5 >> 5;
        // let x = (v & 0x3FF) as u16 as i16 as i32;
        // let y = ((v >> 16) & 0x1FF)  as u16 as i16 as i32;
        Vertex {
            x: x as i32,
            y: y as i32,
        }
    }
    pub fn gp0_page_clut(value: u32) -> [u16; 3] {
        let u = value & 0xFF;
        let v = (value >> 8) & 0xFF;
        let pc = value >> 16;
        [u as u16, v as u16, pc as u16]
    }
    pub fn gp0_nop(&mut self) {}
    pub fn gp0_clear_cache(&mut self) {}
    pub fn gp0_fill_rect(&mut self) {
        let color = Colour::from_gp0(self.gp0_command[0]);
        let v = Self::gp0_vertex(self.gp0_command[1]);
        let Vertex {
            x: width,
            y: height,
        } = Self::gp0_vertex(self.gp0_command[2]);

        let min_x = v.x.max(0) as usize;
        let min_y = v.y.max(0) as usize;
        let max_x = (v.x + width).min(0x400) as usize;
        let max_y = (v.y + height).min(0x200) as usize;

        for x in min_x..max_x {
            for y in min_y..max_y {
                let vram_addr = 2 * (y * 1024 + x) as usize;

                let [pixel_lsb, pixel_msb] = color.to_le_bytes();
                self.vram[vram_addr] = pixel_lsb;
                self.vram[vram_addr + 1] = pixel_msb;
            }
        }
    }
    pub fn render_triangle_mono<const SEMI_TRANS: bool>(
        &mut self,
        mono: Colour,
        vs: &mut [Vertex; 3],
    ) {
        ensure_vertex_order(vs);
        // let [pixel_lsb, pixel_msb] = color;
        let [v0, v1, v2] = vs;

        // bounding box
        let mut min_x = cmp::min(v0.x, cmp::min(v1.x, v2.x));
        let mut max_x = cmp::max(v0.x, cmp::max(v1.x, v2.x));
        let mut min_y = cmp::min(v0.y, cmp::min(v1.y, v2.y));
        let mut max_y = cmp::max(v0.y, cmp::max(v1.y, v2.y));

        // clip pixels outside of the drawing area
        min_x = cmp::max(min_x, self.drawing_area_left as i32);
        max_x = cmp::min(max_x, self.drawing_area_right as i32);
        min_y = cmp::max(min_y, self.drawing_area_top as i32);
        max_y = cmp::min(max_y, self.drawing_area_bottom as i32);

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

                        color.blend_with_background(bg, self.semi_transparency);
                    }
                    self.vram_write_color(vram_addr, color);
                    // let [pixel_lsb, pixel_msb] = color.to_le_bytes();
                    // self.vram[vram_addr] = pixel_lsb;
                    // self.vram[vram_addr + 1] = pixel_msb;
                }
            }
        }

        // let [pixel_lsb, pixel_msb] = Self::gp0_color(self.gp0_command[0]);
    }

    pub fn draw_triangle_shaded<const SEMI_TRANS: bool>(
        &mut self,
        vs: &mut [Vertex; 3],
        colors: &mut [Colour; 3],
    ) {
        ensure_vertex_order2(vs, colors);
        vs[0].x += self.drawing_x_offset as i32;
        vs[0].y += self.drawing_y_offset as i32;
        vs[1].x += self.drawing_x_offset as i32;
        vs[1].y += self.drawing_y_offset as i32;
        vs[2].x += self.drawing_x_offset as i32;
        vs[2].y += self.drawing_y_offset as i32;
        let [v0, v1, v2] = vs;
        let [c0, c1, c2] = colors;

        // bounding box
        let mut min_x = cmp::min(v0.x, cmp::min(v1.x, v2.x));
        let mut max_x = cmp::max(v0.x, cmp::max(v1.x, v2.x));
        let mut min_y = cmp::min(v0.y, cmp::min(v1.y, v2.y));
        let mut max_y = cmp::max(v0.y, cmp::max(v1.y, v2.y));

        // clip pixels outside of the drawing area
        min_x = cmp::max(min_x, self.drawing_area_left as i32);
        max_x = cmp::min(max_x, self.drawing_area_right as i32);
        min_y = cmp::max(min_y, self.drawing_area_top as i32);
        max_y = cmp::min(max_y, self.drawing_area_bottom as i32);

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

                        color.blend_with_background(bg, self.semi_transparency);
                    }

                    self.vram_write_color(vram_addr, color);
                    // let [pixel_lsb, pixel_msb] = color.to_le_bytes();
                    //
                    // self.vram[vram_addr] = pixel_lsb;
                    // self.vram[vram_addr + 1] = pixel_msb;
                }
            }
        }

        // let [pixel_lsb, pixel_msb] = Self::gp0_color(self.gp0_command[0]);
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
    // pub fn render_triangle_texture_opaque(
    //     &mut self,
    //     clut: u16,
    //     page: u16,
    //     vs: &mut [Vertex; 3],
    //     uv: &mut [[u16; 2]; 3],
    // ) {
    //
    // }
    pub fn compute_texel_offset(&self, x: usize, y: usize) -> [usize; 2] {
        let x_mask = self.texture_window_x_mask as usize;
        let y_mask = self.texture_window_y_mask as usize;
        let x_offset = self.texture_window_x_offset as usize;
        let y_offset = self.texture_window_y_offset as usize;

        [
            (x & (!(x_mask * 8))) | ((x_offset & x_mask) * 8),
            (y & (!(y_mask * 8))) | ((y_offset & y_mask) * 8),
        ]
    }

    // TODO: dithering? (should't be the case?)
    pub fn draw_triangle_textured<const SEMI_TRANS: bool, const BLEND: bool>(
        &mut self,
        mono: Colour,
        clut: u16,
        page: u16,
        vs: &mut [Vertex; 3],
        uv: &mut [[u16; 2]; 3],
    ) {
        ensure_vertex_order2(vs, uv);
        vs[0].x += self.drawing_x_offset as i32;
        vs[0].y += self.drawing_y_offset as i32;
        vs[1].x += self.drawing_x_offset as i32;
        vs[1].y += self.drawing_y_offset as i32;
        vs[2].x += self.drawing_x_offset as i32;
        vs[2].y += self.drawing_y_offset as i32;

        // bounding box
        let mut min_x = cmp::min(vs[0].x, cmp::min(vs[1].x, vs[2].x));
        let mut max_x = cmp::max(vs[0].x, cmp::max(vs[1].x, vs[2].x));
        let mut min_y = cmp::min(vs[0].y, cmp::min(vs[1].y, vs[2].y));
        let mut max_y = cmp::max(vs[0].y, cmp::max(vs[1].y, vs[2].y));

        // clip pixels outside of the drawing area
        min_x = cmp::max(min_x, self.drawing_area_left as i32);
        max_x = cmp::min(max_x, self.drawing_area_right as i32);
        min_y = cmp::max(min_y, self.drawing_area_top as i32);
        max_y = cmp::min(max_y, self.drawing_area_bottom as i32);

        // textpage stuff
        let page_base_x = ((page & 0xf) as usize) * 64; // n * 64
        let page_base_y = (((page >> 4) & 1) as usize) * 256; // n * 256
        let semi_transparency = ((page >> 5) & 3) as u8;
        let dithering = (page >> 9) & 1 != 0;
        let draw_to_display = (page >> 10) & 1 != 0;
        let depth = ((page >> 7) & 3) as u8;

        let texture_depth = match (page >> 7) & 3 {
            0 => TextureDepth::T4Bit,
            1 => TextureDepth::T8Bit,
            2 => TextureDepth::T15Bit,
            n => panic!("Unhandled texture depth: {n}"),
        };
        // let texture_page_y_base2 = ((page >> 11) & 1) != 0;

        // clut stuff
        // 0-5    X coordinate X/16  (ie. in 16-halfword steps)
        // 6-14   Y coordinate 0-511 (ie. in 1-line steps)  ;\on v0 GPU (max 1 MB VRAM)
        // 15     Unused (should be 0)                      ;/
        // 6-15   Y coordinate 0-1023 (ie. in 1-line steps) ;on v2 GPU (max 2 MB VRAM)
        let clut_x = ((clut & 0x3f) as usize) * 16;
        let clut_y = ((clut >> 6) & 0x1FF) as usize; // y coord 0-511 (on v0 GPU)

        // println!("uv: {:?}", texture_depth);

        for y in min_y..max_y {
            for x in min_x..max_x {
                let p = Vertex { x, y };
                if is_inside_triangle(p, vs[0], vs[1], vs[2]) {
                    let lambda = compute_barycentric_coordinates(p, vs[0], vs[1], vs[2]);
                    let [uv_x, uv_y] = compute_normal_coordinates(lambda, uv);
                    let [uv_x, uv_y] = self.compute_texel_offset(uv_x, uv_y);

                    let (pixel_msb, pixel_lsb) = match texture_depth {
                        TextureDepth::T4Bit => {
                            let pixel = self.vram
                                [page_base_y * 2048 + uv_y * 2048 + 2 * page_base_x + uv_x / 2];
                            let pixel = (pixel >> 4 * (uv_x & 1)) & 0xF;
                            if pixel == 0 {
                                continue;
                            }

                            let pixel_lsb =
                                self.vram[2 * (clut_y * 1024 + clut_x + pixel as usize)];
                            let pixel_msb =
                                self.vram[2 * (clut_y * 1024 + clut_x + pixel as usize) + 1];
                            (pixel_msb, pixel_lsb)
                        }
                        TextureDepth::T8Bit => {
                            // Width 2048...
                            let pixel = self.vram
                                [page_base_y * 2048 + uv_y * 2048 + 2 * page_base_x + uv_x];
                            if pixel == 0 {
                                continue;
                            }

                            let pixel_lsb =
                                self.vram[2 * (clut_y * 1024 + clut_x + pixel as usize)];
                            let pixel_msb =
                                self.vram[2 * (clut_y * 1024 + clut_x + pixel as usize) + 1];
                            (pixel_msb, pixel_lsb)
                        }
                        TextureDepth::T15Bit => {
                            let texture_x = page_base_x + uv_x;
                            let texture_y = page_base_y + uv_y;
                            let pixel_lsb = self.vram[2 * (texture_y * 1024 + texture_x)];
                            let pixel_msb = self.vram[2 * (texture_y * 1024 + texture_x) + 1];
                            (pixel_msb, pixel_lsb)
                            // let vram_addr = 2 * (y * 1024 + x) as usize;

                            // self.vram[vram_addr] = pixel_lsb;
                            // self.vram[vram_addr + 1] = pixel_msb;
                        }
                    };

                    if pixel_lsb == 0 && pixel_msb == 0 {
                        continue;
                    }

                    let mut pixel = Colour::from_bytes(pixel_msb, pixel_lsb);

                    if BLEND {
                        pixel.blend(mono);
                    }

                    let vram_addr = 2 * (y * 1024 + x) as usize;

                    if SEMI_TRANS && pixel_msb >> 7 == 1 {
                        let background_lsb = self.vram[vram_addr];
                        let background_msb = self.vram[vram_addr + 1];

                        let background = Colour::from_bytes(background_msb, background_lsb);

                        pixel.blend_with_background(background, semi_transparency);
                    }

                    if dithering {
                        pixel.apply_dithering(x, y);
                    }

                    self.vram_write_color(vram_addr, pixel);
                    // let [lsb, msb] = pixel.to_le_bytes();

                    // self.vram[vram_addr] = lsb;
                    // self.vram[vram_addr + 1] = msb;
                }
            }
        }

        // let [pixel_lsb, pixel_msb] = Self::gp0_color(self.gp0_command[0]);
    }

    // TODO: dithering, masked bit...
    pub fn draw_triangle_textured_shaded<const SEMI_TRANS: bool, const BLEND: bool>(
        &mut self,
        colors: &mut [Colour; 3],
        clut: u16,
        page: u16,
        vs: &mut [Vertex; 3],
        uv: &mut [[u16; 2]; 3],
    ) {
        ensure_vertex_order3(vs, uv, colors);
        vs[0].x += self.drawing_x_offset as i32;
        vs[0].y += self.drawing_y_offset as i32;
        vs[1].x += self.drawing_x_offset as i32;
        vs[1].y += self.drawing_y_offset as i32;
        vs[2].x += self.drawing_x_offset as i32;
        vs[2].y += self.drawing_y_offset as i32;

        // bounding box
        let mut min_x = cmp::min(vs[0].x, cmp::min(vs[1].x, vs[2].x));
        let mut max_x = cmp::max(vs[0].x, cmp::max(vs[1].x, vs[2].x));
        let mut min_y = cmp::min(vs[0].y, cmp::min(vs[1].y, vs[2].y));
        let mut max_y = cmp::max(vs[0].y, cmp::max(vs[1].y, vs[2].y));

        // clip pixels outside of the drawing area
        min_x = cmp::max(min_x, self.drawing_area_left as i32);
        max_x = cmp::min(max_x, self.drawing_area_right as i32);
        min_y = cmp::max(min_y, self.drawing_area_top as i32);
        max_y = cmp::min(max_y, self.drawing_area_bottom as i32);

        // textpage stuff
        let page_base_x = ((page & 0xf) as usize) * 64; // n * 64
        let page_base_y = (((page >> 4) & 1) as usize) * 256; // n * 256
        let semi_transparency = ((page >> 5) & 3) as u8;

        let texture_depth = match (page >> 7) & 3 {
            0 => TextureDepth::T4Bit,
            1 => TextureDepth::T8Bit,
            2 => TextureDepth::T15Bit,
            n => panic!("Unhandled texture depth: {n}"),
        };
        // let texture_page_y_base2 = ((page >> 11) & 1) != 0;

        // clut stuff
        // 0-5    X coordinate X/16  (ie. in 16-halfword steps)
        // 6-14   Y coordinate 0-511 (ie. in 1-line steps)  ;\on v0 GPU (max 1 MB VRAM)
        // 15     Unused (should be 0)                      ;/
        // 6-15   Y coordinate 0-1023 (ie. in 1-line steps) ;on v2 GPU (max 2 MB VRAM)
        let clut_x = ((clut & 0x3f) as usize) * 16;
        let clut_y = ((clut >> 6) & 0x1FF) as usize; // y coord 0-511 (on v0 GPU)

        // println!("uv: {:?}", texture_depth);

        for y in min_y..max_y {
            for x in min_x..max_x {
                let p = Vertex { x, y };
                if is_inside_triangle(p, vs[0], vs[1], vs[2]) {
                    let lambda = compute_barycentric_coordinates(p, vs[0], vs[1], vs[2]);
                    let [uv_x, uv_y] = compute_normal_coordinates(lambda, uv);
                    let [uv_x, uv_y] = self.compute_texel_offset(uv_x, uv_y);

                    let (pixel_msb, pixel_lsb) = match texture_depth {
                        TextureDepth::T4Bit => {
                            let pixel = self.vram
                                [page_base_y * 2048 + uv_y * 2048 + 2 * page_base_x + uv_x / 2];
                            let pixel = (pixel >> 4 * (uv_x & 1)) & 0xF;
                            if pixel == 0 {
                                continue;
                            }

                            let pixel_lsb =
                                self.vram[2 * (clut_y * 1024 + clut_x + pixel as usize)];
                            let pixel_msb =
                                self.vram[2 * (clut_y * 1024 + clut_x + pixel as usize) + 1];
                            (pixel_msb, pixel_lsb)
                        }
                        TextureDepth::T8Bit => {
                            // Width 2048...
                            let pixel = self.vram
                                [page_base_y * 2048 + uv_y * 2048 + 2 * page_base_x + uv_x];
                            if pixel == 0 {
                                continue;
                            }

                            let pixel_lsb =
                                self.vram[2 * (clut_y * 1024 + clut_x + pixel as usize)];
                            let pixel_msb =
                                self.vram[2 * (clut_y * 1024 + clut_x + pixel as usize) + 1];
                            (pixel_msb, pixel_lsb)
                        }
                        TextureDepth::T15Bit => {
                            let texture_x = page_base_x + uv_x;
                            let texture_y = page_base_y + uv_y;
                            let pixel_lsb = self.vram[2 * (texture_y * 1024 + texture_x)];
                            let pixel_msb = self.vram[2 * (texture_y * 1024 + texture_x) + 1];
                            (pixel_msb, pixel_lsb)
                            // let vram_addr = 2 * (y * 1024 + x) as usize;

                            // self.vram[vram_addr] = pixel_lsb;
                            // self.vram[vram_addr + 1] = pixel_msb;
                        }
                    };

                    if pixel_lsb == 0 && pixel_msb == 0 {
                        continue;
                    }

                    let mut pixel = Colour::from_bytes(pixel_msb, pixel_lsb);

                    if BLEND {
                        let color = interpolate_color(lambda, *colors);
                        let color = apply_dithering(color, p);
                        pixel.blend(color);
                    }

                    let vram_addr = 2 * (y * 1024 + x) as usize;

                    if SEMI_TRANS && pixel_msb >> 7 == 1 {
                        let background_lsb = self.vram[vram_addr];
                        let background_msb = self.vram[vram_addr + 1];

                        let background = Colour::from_bytes(background_msb, background_lsb);

                        pixel.blend_with_background(background, semi_transparency);
                    }

                    self.vram_write_color(vram_addr, pixel);
                    // let [lsb, msb] = pixel.to_le_bytes();
                    //
                    // self.vram[vram_addr] = lsb;
                    // self.vram[vram_addr + 1] = msb;
                }
            }
        }

        // let [pixel_lsb, pixel_msb] = Self::gp0_color(self.gp0_command[0]);
    }

    pub fn clip_rect(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> Option<(i32, i32, i32, i32)> {
        if x1 - x0 > 1023 || y1 - y0 > 511 {
            return None;
        }
        let l = self.drawing_area_left as i32;
        let t = self.drawing_area_top as i32;
        let b = self.drawing_area_bottom as i32;
        let r = self.drawing_area_right as i32;

        if x1 < l || x0 > r || y1 < t || y0 > b {
            return None;
        }
        Some((x0.max(l), y0.max(t), x1.min(r), y1.min(b)))
    }

    pub fn draw_rectangle<const SEMI_TRANS: bool>(
        &mut self,
        mut v: Vertex,
        side: Vertex,
        color: Colour,
    ) {
        v.x += self.drawing_x_offset as i32;
        v.y += self.drawing_y_offset as i32;

        let Some((min_x, min_y, max_x, max_y)) =
            self.clip_rect(v.x, v.y, v.x + side.x - 1, v.y + side.y - 1)
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

                    pixel.blend_with_background(background, self.semi_transparency);
                }

                // let [lsb, msb] = pixel.to_le_bytes();
                //
                // self.vram[vram_addr] = lsb;
                // self.vram[vram_addr + 1] = msb;
                self.vram_write_color(vram_addr, pixel);
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
    ) {
        v.x += self.drawing_x_offset as i32;
        v.y += self.drawing_y_offset as i32;

        let Some((min_x, min_y, max_x, max_y)) =
            self.clip_rect(v.x, v.y, v.x + side.x - 1, v.y + side.y - 1)
        else {
            return;
        };

        // textpage stuff
        let page_base_x = (self.page_base_x as usize) * 64; // n * 64
        let page_base_y = (self.page_base_y as usize) * 256; // n * 256
        let semi_transparency = self.semi_transparency;

        let texture_depth = self.texture_depth;

        // clut stuff
        // 0-5    X coordinate X/16  (ie. in 16-halfword steps)
        // 6-14   Y coordinate 0-511 (ie. in 1-line steps)  ;\on v0 GPU (max 1 MB VRAM)
        // 15     Unused (should be 0)                      ;/
        // 6-15   Y coordinate 0-1023 (ie. in 1-line steps) ;on v2 GPU (max 2 MB VRAM)
        let clut_x = ((clut & 0x3f) as usize) * 16;
        let clut_y = ((clut >> 6) & 0x1FF) as usize; // y coord 0-511 (on v0 GPU)
        //
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let uv_x = (uv[0] as i32) + (x as i32) - v.x;
                let uv_y = (uv[1] as i32) + (y as i32) - v.y;
                let [uv_x, uv_y] = self.compute_texel_offset(uv_x as usize, uv_y as usize);
                let (pixel_msb, pixel_lsb) = match texture_depth {
                    TextureDepth::T4Bit => {
                        let pixel = self.vram[page_base_y * 2048
                            + (uv_y as usize) * 2048
                            + 2 * page_base_x
                            + (uv_x as usize) / 2];
                        let pixel = (pixel >> 4 * (uv_x & 1)) & 0xF;
                        if pixel == 0 {
                            continue;
                        }

                        let pixel_lsb = self.vram[2 * (clut_y * 1024 + clut_x + pixel as usize)];
                        let pixel_msb =
                            self.vram[2 * (clut_y * 1024 + clut_x + pixel as usize) + 1];
                        (pixel_msb, pixel_lsb)
                    }
                    TextureDepth::T8Bit => {
                        // Width 2048...
                        let pixel = self.vram[page_base_y * 2048
                            + (uv_y as usize) * 2048
                            + 2 * page_base_x
                            + (uv_x as usize)];
                        if pixel == 0 {
                            continue;
                        }

                        let pixel_lsb = self.vram[2 * (clut_y * 1024 + clut_x + pixel as usize)];
                        let pixel_msb =
                            self.vram[2 * (clut_y * 1024 + clut_x + pixel as usize) + 1];
                        (pixel_msb, pixel_lsb)
                    }
                    TextureDepth::T15Bit => {
                        let texture_x = page_base_x + (uv_x as usize);
                        let texture_y = page_base_y + (uv_y as usize);
                        let pixel_lsb = self.vram[2 * (texture_y * 1024 + texture_x)];
                        let pixel_msb = self.vram[2 * (texture_y * 1024 + texture_x) + 1];
                        (pixel_msb, pixel_lsb)
                        // let vram_addr = 2 * (y * 1024 + x) as usize;

                        // self.vram[vram_addr] = pixel_lsb;
                        // self.vram[vram_addr + 1] = pixel_msb;
                    }
                };

                if pixel_lsb == 0 && pixel_msb == 0 {
                    continue;
                }

                let mut pixel = Colour::from_bytes(pixel_msb, pixel_lsb);

                if BLEND {
                    pixel.blend(color);
                }

                let vram_addr = 2 * (y * 1024 + x) as usize;

                if SEMI_TRANS && pixel_msb >> 7 == 1 {
                    let background_lsb = self.vram[vram_addr];
                    let background_msb = self.vram[vram_addr + 1];

                    let background = Colour::from_bytes(background_msb, background_lsb);

                    pixel.blend_with_background(background, self.semi_transparency);
                }

                self.vram_write_color(vram_addr, pixel);
                // let [lsb, msb] = pixel.to_le_bytes();

                // self.vram[vram_addr] = lsb;
                // self.vram[vram_addr + 1] = msb;
            }
        }
    }

    pub fn draw_line<const SEMI_TRANS: bool>(
        &mut self,
        mut v0: Vertex,
        mut v1: Vertex,
        mono: Colour,
    ) {
        v0.x += self.drawing_x_offset as i32;
        v0.y += self.drawing_y_offset as i32;
        v1.x += self.drawing_x_offset as i32;
        v1.y += self.drawing_y_offset as i32;

        let Some((x0, y0, x1, y1)) = self.clip_rect(v0.x, v0.y, v1.x, v1.y) else {
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

                pixel.blend_with_background(background, self.semi_transparency);
            }
            pixel.apply_dithering(x, y);

            self.vram_write_color(vram_addr, pixel);
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

    // TODO: dither!
    pub fn draw_line_shaded<const SEMI_TRANS: bool>(
        &mut self,
        mut v0: Vertex,
        mut v1: Vertex,
        color0: Colour,
        color1: Colour,
    ) {
        v0.x += self.drawing_x_offset as i32;
        v0.y += self.drawing_y_offset as i32;
        v1.x += self.drawing_x_offset as i32;
        v1.y += self.drawing_y_offset as i32;

        let Some((x0, y0, x1, y1)) = self.clip_rect(v0.x, v0.y, v1.x, v1.y) else {
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

                pixel.blend_with_background(background, self.semi_transparency);
            }

            pixel.apply_dithering(x, y);

            self.vram_write_color(vram_addr, pixel);

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

    pub fn gp0_line_mono<const SEMI_TRANS: bool>(&mut self) {
        let color = Colour::from_gp0(self.gp0_command[0]);
        let v0 = Self::gp0_vertex(self.gp0_command[1]);
        let v1 = Self::gp0_vertex(self.gp0_command[2]);

        self.draw_line::<SEMI_TRANS>(v0, v1, color);
    }
    pub fn gp0_line_shaded<const SEMI_TRANS: bool>(&mut self) {
        let color0 = Colour::from_gp0(self.gp0_command[0]);
        let v0 = Self::gp0_vertex(self.gp0_command[1]);
        let color1 = Colour::from_gp0(self.gp0_command[2]);
        let v1 = Self::gp0_vertex(self.gp0_command[3]);

        self.draw_line_shaded::<SEMI_TRANS>(v0, v1, color0, color1);
    }

    pub fn gp0_rect_fixed<const SIDE: i32, const SEMI_TRANS: bool>(&mut self) {
        let color = Colour::from_gp0(self.gp0_command[0]);
        let v = Self::gp0_vertex(self.gp0_command[1]);

        self.draw_rectangle::<SEMI_TRANS>(v, Vertex { x: SIDE, y: SIDE }, color);
        // let [pixel_lsb, pixel_msb] = Self::gp0_color(self.gp0_command[0]);
        // let [x, y] = Self::gp0_position(self.gp0_command[1]);
        // // println!("monochrome rect 1x1");
        // let vram_addr = 2 * (y * 1024 + x) as usize;
        // self.vram[vram_addr] = pixel_lsb;
        // self.vram[vram_addr + 1] = pixel_msb;
    }
    pub fn gp0_rect_variable<const SEMI_TRANS: bool>(&mut self) {
        let color = Colour::from_gp0(self.gp0_command[0]);
        let v = Self::gp0_vertex(self.gp0_command[1]);
        let side = Self::gp0_vertex(self.gp0_command[2]);

        self.draw_rectangle::<SEMI_TRANS>(v, side, color);
    }
    pub fn gp0_rect_texture_fixed<const SIDE: i32, const SEMI_TRANS: bool, const BLEND: bool>(
        &mut self,
    ) {
        let color = Colour::from_gp0(self.gp0_command[0]);
        let v = Self::gp0_vertex(self.gp0_command[1]);
        let [_u, _v, clut] = Self::gp0_page_clut(self.gp0_command[2]);

        self.draw_rectangle_textured::<SEMI_TRANS, BLEND>(
            v,
            Vertex { x: SIDE, y: SIDE },
            color,
            clut,
            [_u, _v],
        );
    }

    pub fn gp0_rect_texture_variable<const SEMI_TRANS: bool, const BLEND: bool>(&mut self) {
        let color = Colour::from_gp0(self.gp0_command[0]);
        let v = Self::gp0_vertex(self.gp0_command[1]);
        let [_u, _v, clut] = Self::gp0_page_clut(self.gp0_command[2]);
        let side = Self::gp0_vertex(self.gp0_command[3]);

        self.draw_rectangle_textured::<SEMI_TRANS, BLEND>(v, side, color, clut, [_u, _v]);
    }

    pub fn gp0_poly_mono<const QUAD: bool, const SEMI_TRANS: bool>(&mut self) {
        let color = Colour::from_gp0(self.gp0_command[0]);
        let v0 = Self::gp0_vertex(self.gp0_command[1]);
        let v1 = Self::gp0_vertex(self.gp0_command[2]);
        let v2 = Self::gp0_vertex(self.gp0_command[3]);

        self.render_triangle_mono::<SEMI_TRANS>(color, &mut [v0, v1, v2]);
        if QUAD {
            let v3 = Self::gp0_vertex(self.gp0_command[4]);
            self.render_triangle_mono::<SEMI_TRANS>(color, &mut [v1, v2, v3]);
        }
    }

    pub fn gp0_poly_texture_shaded<const QUAD: bool, const SEMI_TRANS: bool, const BLEND: bool>(
        &mut self,
    ) {
        let c0 = Colour::from_gp0(self.gp0_command[0]);
        let c1 = Colour::from_gp0(self.gp0_command[3]);
        let c2 = Colour::from_gp0(self.gp0_command[6]);
        let v0 = Self::gp0_vertex(self.gp0_command[1]);
        let [_u0, _v0, clut] = Self::gp0_page_clut(self.gp0_command[2]);
        let v1 = Self::gp0_vertex(self.gp0_command[4]);
        let [_u1, _v1, page] = Self::gp0_page_clut(self.gp0_command[5]);
        let v2 = Self::gp0_vertex(self.gp0_command[7]);
        let [_u2, _v2, _] = Self::gp0_page_clut(self.gp0_command[8]);

        let mut uv0 = [[_u0, _v0], [_u1, _v1], [_u2, _v2]];
        let mut vs0 = [v0, v1, v2];
        let mut cs0 = [c0, c1, c2];
        self.draw_triangle_textured_shaded::<SEMI_TRANS, BLEND>(
            &mut cs0, clut, page, &mut vs0, &mut uv0,
        );
        if QUAD {
            let c3 = Colour::from_gp0(self.gp0_command[9]);
            let v3 = Self::gp0_vertex(self.gp0_command[10]);
            let [_u3, _v3, _] = Self::gp0_page_clut(self.gp0_command[11]);
            let mut uv1 = [[_u1, _v1], [_u2, _v2], [_u3, _v3]];
            let mut vs1 = [v1, v2, v3];
            let mut cs1 = [c1, c2, c3];
            self.draw_triangle_textured_shaded::<SEMI_TRANS, BLEND>(
                &mut cs1, clut, page, &mut vs1, &mut uv1,
            );
        }
    }

    pub fn gp0_poly_texture<const QUAD: bool, const SEMI_TRANS: bool, const BLEND: bool>(
        &mut self,
    ) {
        let color = Colour::from_gp0(self.gp0_command[0]);
        let v0 = Self::gp0_vertex(self.gp0_command[1]);
        let [_u0, _v0, clut] = Self::gp0_page_clut(self.gp0_command[2]);
        let v1 = Self::gp0_vertex(self.gp0_command[3]);
        let [_u1, _v1, page] = Self::gp0_page_clut(self.gp0_command[4]);
        let v2 = Self::gp0_vertex(self.gp0_command[5]);
        let [_u2, _v2, _] = Self::gp0_page_clut(self.gp0_command[6]);

        let mut uv0 = [[_u0, _v0], [_u1, _v1], [_u2, _v2]];
        let mut vs0 = [v0, v1, v2];
        self.draw_triangle_textured::<SEMI_TRANS, BLEND>(color, clut, page, &mut vs0, &mut uv0);
        if QUAD {
            let v3 = Self::gp0_vertex(self.gp0_command[7]);
            let [_u3, _v3, _] = Self::gp0_page_clut(self.gp0_command[8]);
            let mut uv1 = [[_u1, _v1], [_u2, _v2], [_u3, _v3]];
            let mut vs1 = [v1, v2, v3];
            self.draw_triangle_textured::<SEMI_TRANS, BLEND>(color, clut, page, &mut vs1, &mut uv1);
        }
    }
    pub fn gp0_poly_shaded<const QUAD: bool, const SEMI_TRANS: bool>(&mut self) {
        // 6
        let c0 = Self::gp0_colour(self.gp0_command[0]);
        let v0 = Self::gp0_vertex(self.gp0_command[1]);
        let c1 = Self::gp0_colour(self.gp0_command[2]);
        let v1 = Self::gp0_vertex(self.gp0_command[3]);
        let c2 = Self::gp0_colour(self.gp0_command[4]);
        let v2 = Self::gp0_vertex(self.gp0_command[5]);
        self.draw_triangle_shaded::<SEMI_TRANS>(&mut [v0, v1, v2], &mut [c0, c1, c2]);
        if QUAD {
            let c3 = Self::gp0_colour(self.gp0_command[6]);
            let v3 = Self::gp0_vertex(self.gp0_command[7]);
            self.draw_triangle_shaded::<SEMI_TRANS>(&mut [v1, v2, v3], &mut [c1, c2, c3]);
        }
    }
    pub fn gp0_vram_to_vram_blit(&mut self) {
        let src = Self::gp0_vertex(self.gp0_command[1]);
        let dst = Self::gp0_vertex(self.gp0_command[2]);
        let size = Self::gp0_vertex(self.gp0_command[3]);
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
    pub fn gp0_image_load(&mut self) {
        let pos = self.gp0_command[1];

        let x = pos as u16;
        let y = (pos >> 16) as u16;

        let res = self.gp0_command[2];
        let mut width = res & 0xffff;
        if width == 0 {
            width = 1024;
        }

        let mut height = res >> 16;
        if height == 0 {
            height = 512;
        }

        let imgsize = width * height;
        // rounding so we have 16 bit of padding in last word
        let imgsize = (imgsize + 1) & !1;
        self.gp0_words_remaining = imgsize / 2;
        self.gp0_mode = Gp0Mode::ImageLoad {
            top_left: (x, y),
            resolution: (width as u16, height as u16),
            current_row: 0,
            current_col: 0,
        };
    }

    pub fn gp0_image_store(&mut self) {
        let pos = self.gp0_command[1];
        let res = self.gp0_command[2];
        // let width = res & 0xffff;
        // let height = res >> 16;
        let mut width = res & 0xffff;
        if width == 0 {
            width = 1024;
        }

        let mut height = res >> 16;
        if height == 0 {
            height = 512;
        }

        let x = pos as u16;
        let y = (pos >> 16) as u16;

        let imgsize = width * height;
        // rounding so we have 16 bit of padding in last word
        let imgsize = (imgsize + 1) & !1;
        self.gp0_words_remaining = imgsize / 2;
        self.gp0_mode = Gp0Mode::ImageStore {
            top_left: (x, y),
            resolution: (width as u16, height as u16),
            current_row: 0,
            current_col: 0,
        };
        // println!("Unhandled image store: {}x{}", width, height);
    }

    pub fn gp0_draw_mode(&mut self) {
        let val: u32 = self.gp0_command[0];
        self.page_base_x = (val & 0xf) as u8;
        self.page_base_y = ((val >> 4) & 1) as u8;
        self.semi_transparency = ((val >> 5) & 3) as u8;

        self.texture_depth = match (val >> 7) & 3 {
            0 => TextureDepth::T4Bit,
            1 => TextureDepth::T8Bit,
            2 => TextureDepth::T15Bit,
            n => panic!("Unhandled texture depth: {n}"),
        };
        self.dithering = ((val >> 9) & 1) != 0;
        self.draw_to_display = ((val >> 10) & 1) != 0;
        self.texture_disable = ((val >> 11) & 1) != 0;
        self.rectange_texture_x_flip = ((val >> 12) & 1) != 0;
        self.rectange_texture_y_flip = ((val >> 13) & 1) != 0;
    }
    pub fn gp0_texture_window(&mut self) {
        let val: u32 = self.gp0_command[0];
        self.texture_window_x_mask = (val & 0x1f) as u8;
        self.texture_window_y_mask = ((val >> 5) & 0x1f) as u8;
        self.texture_window_x_offset = ((val >> 10) & 0x1f) as u8;
        self.texture_window_y_offset = ((val >> 15) & 0x1f) as u8;
    }
    pub fn gp0_drawing_area_top_left(&mut self) {
        let val: u32 = self.gp0_command[0];
        self.drawing_area_top = ((val >> 10) & 0x3ff) as u16;
        self.drawing_area_left = (val & 0x3ff) as u16;
    }
    pub fn gp0_drawing_area_bottom_right(&mut self) {
        let val: u32 = self.gp0_command[0];
        self.drawing_area_bottom = ((val >> 10) & 0x3ff) as u16;
        self.drawing_area_right = (val & 0x3ff) as u16;
    }
    pub fn gp0_drawing_offset(&mut self) {
        let val: u32 = self.gp0_command[0];
        let x = (val & 0x7ff) as u16;
        let y = ((val >> 11) & 0x7ff) as u16;

        self.drawing_x_offset = ((x << 5) as i16) >> 5;
        self.drawing_y_offset = ((y << 5) as i16) >> 5;
    }
    pub fn gp0_mask_bit_setting(&mut self) {
        let val: u32 = self.gp0_command[0];
        self.force_set_mask_bit = (val & 1) != 0;
        self.preserve_masked_pixels = (val & 2) != 0;
    }

    pub fn vram_write_color(&mut self, address: usize, color: Colour) {
        if self.preserve_masked_pixels && (self.vram[address + 1] & 0x80) != 0 {
            return;
        }
        let [lsb, msb] = color.to_le_bytes();
        let msb = msb | (self.force_set_mask_bit as u8) << 7;
        self.vram[address] = lsb;
        self.vram[address + 1] = msb;
    }

    pub fn gp1(&mut self, val: u32) {
        let opcode = (val >> 24) & 0xff;
        match opcode {
            0x00 => self.gp1_reset(val),
            0x01 => self.gp1_reset_command_buffer(val),
            0x02 => self.gp1_acknowledge_irq(val),
            0x03 => self.gp1_display_enable(val),
            0x04 => self.gp1_dma_direction(val),
            0x05 => self.gp1_display_vram_start(val),
            0x06 => self.gp1_display_horizontal_range(val),
            0x07 => self.gp1_display_vertical_range(val),
            0x08 => self.gp1_display_mode(val),
            0x10 => self.gp1_get_gpu_info(val),
            _ => panic!("Unhandled GP1 command {:08X} opcode: {:02X}", val, opcode),
        }
    }
    pub fn gp1_reset(&mut self, _val: u32) {
        // TODO: also clear the command FIFO when we implement it
        // and texture cache
        self.page_base_x = 0;
        self.page_base_y = 0;
        self.semi_transparency = 0;
        self.texture_depth = TextureDepth::T4Bit;
        self.texture_window_x_mask = 0;
        self.texture_window_y_mask = 0;
        self.texture_window_x_offset = 0;
        self.texture_window_y_offset = 0;
        self.dithering = false;
        self.draw_to_display = false;
        self.texture_disable = false;
        self.rectange_texture_x_flip = false;
        self.rectange_texture_y_flip = false;
        self.drawing_area_left = 0;
        self.drawing_area_top = 0;
        self.drawing_area_right = 0;
        self.drawing_area_bottom = 0;
        self.drawing_x_offset = 0;
        self.drawing_y_offset = 0;
        self.force_set_mask_bit = false;
        self.preserve_masked_pixels = false;

        self.dma_direction = DmaDirection::Off;

        self.display_disabled = true;
        self.display_vram_x_start = 0;
        self.display_vram_y_start = 0;
        self.hres = HorizontalRes::from_fields(0, 0);
        self.vres = VerticalRes::Y240Lines;

        self.vmode = VMode::Ntsc;
        self.interlaced = true;
        self.display_horiz_start = 0x200;
        self.display_horiz_end = 0xc00;
        self.display_line_start = 0x10;
        self.display_line_end = 0x100;
        self.display_depth = DisplayDepth::D15Bits;
        self.interrupt = false;
        self.gp1_reset_command_buffer(0);
    }

    pub fn gp1_reset_command_buffer(&mut self, _: u32) {
        self.gp0_command.clear();
        self.gp0_words_remaining = 0;
        self.gp0_mode = Gp0Mode::Command;
    }
    pub fn gp1_acknowledge_irq(&mut self, _: u32) {
        self.interrupt = false;
    }
    pub fn gp1_display_enable(&mut self, val: u32) {
        self.display_disabled = val & 1 != 0;
    }
    pub fn gp1_display_mode(&mut self, val: u32) {
        let hr1 = (val & 3) as u8;
        let hr2 = ((val >> 6) & 1) as u8;

        self.hres = HorizontalRes::from_fields(hr1, hr2);

        self.vres = match val & 0x4 != 0 {
            false => VerticalRes::Y240Lines,
            true => VerticalRes::Y480Lines,
        };

        self.vmode = match val & 0x8 != 0 {
            false => VMode::Ntsc,
            true => VMode::Pal,
        };

        self.display_depth = match val & 0x10 != 0 {
            false => DisplayDepth::D15Bits,
            true => DisplayDepth::D24Bits,
        };

        self.interlaced = val & 0x20 != 0;

        if val & 0x80 != 0 {
            panic!("Unsupported display mode {:08x}", val);
        }
    }

    pub fn gp1_get_gpu_info(&mut self, val: u32) {
        let v = val % 8;
        self.read = match v {
            0 | 1 => self.read, // nop
            2 => self.texture_window_setting(),
            3 => self.draw_area_top_left(),
            4 => self.draw_area_bottom_right(),
            5 => self.draw_offset(),
            6 | 7 => self.read, // nop
            _ => unreachable!("v could be 0...7"),
        }
    }
    pub fn texture_window_setting(&self) -> u32 {
        let mask_x = self.texture_window_x_mask as u32;
        let mask_y = self.texture_window_y_mask as u32;

        let offs_x = self.texture_window_x_offset as u32;
        let offs_y = self.texture_window_y_offset as u32;

        (mask_x & 31) | ((mask_y & 31) << 5) | ((offs_x & 31) << 10) | ((offs_y & 31) << 15)
    }
    pub fn draw_offset(&self) -> u32 {
        let x = self.drawing_x_offset as u32;
        let y = self.drawing_y_offset as u32;

        ((y & 0x3FF) << 11) | (x & 0x7FF)
    }

    pub fn draw_area_top_left(&self) -> u32 {
        let x = self.drawing_area_left as u32;
        let y = self.drawing_area_top as u32;

        ((y & 0x1FF) << 10) | (x & 0x3FF)
    }

    pub fn draw_area_bottom_right(&self) -> u32 {
        let x = self.drawing_area_right as u32;
        let y = self.drawing_area_bottom as u32;

        ((y & 0x1FF) << 10) | (x & 0x3FF)
    }

    pub fn gp1_display_vram_start(&mut self, val: u32) {
        self.display_vram_x_start = (val & 0x3fe) as u16;
        self.display_vram_y_start = ((val >> 10) & 0x1ff) as u16;
    }

    pub fn gp1_display_horizontal_range(&mut self, val: u32) {
        self.display_horiz_start = (val & 0xfff) as u16;
        self.display_horiz_end = ((val >> 12) & 0xfff) as u16;
    }

    pub fn gp1_display_vertical_range(&mut self, val: u32) {
        self.display_line_start = (val & 0x3ff) as u16;
        self.display_line_end = ((val >> 10) & 0x3ff) as u16;
    }

    pub fn gp1_dma_direction(&mut self, val: u32) {
        self.dma_direction = match val & 3 {
            0 => DmaDirection::Off,
            1 => DmaDirection::Fifo,
            2 => DmaDirection::CpuToGp0,
            3 => DmaDirection::VRamToCpu,
            _ => unreachable!(),
        };
    }

    pub fn read(&mut self) -> u32 {
        if let Gp0Mode::ImageStore {
            top_left,
            resolution,
            current_row,
            current_col,
        } = self.gp0_mode
        {
            let mut current_row = current_row;
            let mut current_col = current_col;
            // 2 halfwords per GP0 read..
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
                if current_col == resolution.0 {
                    current_col = 0;
                    current_row += 1;
                }
            }
            self.gp0_words_remaining -= 1;
            if self.gp0_words_remaining == 0 {
                self.gp0_mode = Gp0Mode::Command;
                // println!("done transfering");
            } else {
                self.gp0_mode = Gp0Mode::ImageStore {
                    top_left,
                    resolution,
                    current_row,
                    current_col,
                };
            }
            // println!("transfering 0x{:X}", word);
            // 0xff
            word
        } else {
            self.read
        }
    }

    pub fn status(&self) -> u32 {
        let mut r = 0u32;

        r |= (self.page_base_x as u32) << 0;
        r |= (self.page_base_y as u32) << 4;
        r |= (self.semi_transparency as u32) << 5;
        r |= (self.texture_depth as u32) << 7;
        r |= (self.dithering as u32) << 9;
        r |= (self.draw_to_display as u32) << 10;
        r |= (self.force_set_mask_bit as u32) << 11;
        r |= (self.preserve_masked_pixels as u32) << 12;
        r |= (self.field as u32) << 13;
        // bit 14 not supported (reverse flag)
        r |= (self.texture_disable as u32) << 15;
        r |= self.hres.into_status();
        r |= (self.vres as u32) << 19;
        r |= (self.vmode as u32) << 20;
        r |= (self.display_depth as u32) << 21;
        r |= (self.interlaced as u32) << 22;
        r |= (self.display_disabled as u32) << 23;
        r |= (self.interrupt as u32) << 24;

        let dma_request = self.dma_direction == DmaDirection::CpuToGp0
            || self.dma_direction == DmaDirection::VRamToCpu;

        r |= (dma_request as u32) << 25;

        // Ready to receive command:
        // let is_idle = self.gp0_words_remaining == 0 && self.gp0_mode == Gp0Mode::Command;
        // r |= (is_idle as u32) << 26;
        r |= (1 as u32) << 26;

        // Ready to send VRAM to CPU
        // if let Gp0Mode::ImageStore {
        //     top_left,
        //     resolution,
        //     current_row,
        //     current_col,
        // } = self.gp0_mode
        {
            r |= 1 << 27;
        }
        // r |= 0 << 27;
        // ready to receive DMA block
        r |= 1 << 28;

        let dma_request = match self.dma_direction {
            DmaDirection::Off => 0,
            DmaDirection::Fifo => 1,
            DmaDirection::CpuToGp0 => (r >> 28) & 1,
            DmaDirection::VRamToCpu => (r >> 27) & 1,
        };

        r |= dma_request << 25;

        // should change depending even/odd/vblank line
        // (0=Even or Vblank, 1=Odd)
        // In 480-lines mode, bit31 changes per frame. And in 240-lines mode, the bit changes per scanline.
        r |= ((!self.even & !self.in_vblank) as u32) << 31;

        r
    }
    pub fn load<T: Addressable>(&mut self, offset: u32) -> T {
        if T::width() != AccessWidth::Word {
            panic!("Unhandled {:?} GPU load", T::width());
        }
        let r = match offset {
            4 => self.status(), // 0x1c000000,
            0 => self.read(),
            _ => panic!("Unhandled GPU read {offset}"),
        };
        T::from_u32(r)
    }
    pub fn store<T: Addressable>(&mut self, offset: u32, value: T) {
        if T::width() != AccessWidth::Word {
            panic!("Unhandled {:?} GPU load", T::width());
        }
        let val = value.as_u32();
        match offset {
            0 => self.gp0(val),
            4 => self.gp1(val),
            _ => panic!("GPU write {}: {:08X}", offset, val),
        };
    }

    pub fn convert_5bit_to_8bit(color: u16) -> u8 {
        // Note it is probably a lot faster to use a 32-entry lookup table than doing this calculation live
        // (f64::from(color) * 255.0 / 31.0).round() as u8
        FIVE_BIT_TO_8BIT[color as usize]
    }

    pub fn render_vram(&self, output_frame_buffer: &mut [Color]) {
        for y in 0..512 {
            for x in 0..1024 {
                let vram_addr = 2 * (1024 * y + x);
                let pixel = u16::from_le_bytes([self.vram[vram_addr], self.vram[vram_addr + 1]]);

                let r = Gpu::convert_5bit_to_8bit(pixel & 0x1F);
                let g = Gpu::convert_5bit_to_8bit((pixel >> 5) & 0x1F);
                let b = Gpu::convert_5bit_to_8bit((pixel >> 10) & 0x1F);

                output_frame_buffer[1024 * y + x] = Color { r, g, b, a: 255 };
            }
        }
    }

    pub fn enter_vsync(&mut self) {
        self.interrupt = true;
        self.in_vblank = true;
        self.even = !self.even;
    }
    pub fn exit_vsync(&mut self) {
        self.in_vblank = false;
    }
    pub fn enter_hsync(&mut self) {
        self.in_hblank = true;
        if !self.interlaced {
            self.even = !self.even;
        }
    }
    pub fn exit_hsync(&mut self) {
        self.in_hblank = false;
    }
    pub fn get_clock_divider(&self) -> u16 {
        match self.hres.0 {
            0 => 10,            // 256
            1 => 8,             // 320
            2 => 7,             // 512
            3 => 5,             // 640
            4 | 5 | 6 | 7 => 4, // 368
            _ => panic!("not implemented"),
        }
    }
}

const FIVE_BIT_TO_8BIT: [u8; 32] = {
    let mut table = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        table[i] = (i as f64 * 255.0 / 31.0).round() as u8;
        i += 1;
    }
    table
};

#[derive(Clone, Copy, Debug)]
enum TextureDepth {
    T4Bit = 0,
    T8Bit = 1,
    T15Bit = 2,
}

#[derive(Clone, Copy)]
enum Field {
    // Top field (odd lines)
    Top = 1,
    // Bottom field (even lines)
    Bottom = 2,
}

#[derive(Clone, Copy)]
struct HorizontalRes(u8);

impl HorizontalRes {
    fn from_fields(hr1: u8, hr2: u8) -> HorizontalRes {
        let hr = (hr2 & 1) | ((hr1 & 3) << 1);
        HorizontalRes(hr)
    }
    fn into_status(self) -> u32 {
        let HorizontalRes(hr) = self;

        (hr as u32) << 16
    }
}

#[derive(Clone, Copy)]
enum VerticalRes {
    Y240Lines = 0,
    // only for interlaced output
    Y480Lines = 1,
}

#[derive(Clone, Copy)]
enum VMode {
    // 480i60H
    Ntsc = 0,
    // 576i50Hz
    Pal = 1,
}

#[derive(Clone, Copy)]
enum DisplayDepth {
    D15Bits = 0,
    D24Bits = 1,
}

#[derive(Clone, Copy, PartialEq)]
enum DmaDirection {
    Off = 0,
    Fifo = 1,
    CpuToGp0 = 2,
    VRamToCpu = 3,
}

struct CommandBuffer {
    buffer: [u32; 12],
    len: u8,
}

impl CommandBuffer {
    fn new() -> CommandBuffer {
        CommandBuffer {
            buffer: [0; 12],
            len: 0,
        }
    }
    fn clear(&mut self) {
        self.len = 0;
    }

    fn push_word(&mut self, word: u32) {
        self.buffer[self.len as usize] = word;
        self.len += 1;
    }
}

impl ::std::ops::Index<usize> for CommandBuffer {
    type Output = u32;

    fn index<'a>(&'a self, index: usize) -> &'a Self::Output {
        if index >= self.len as usize {
            panic!(
                "Command buffer index out of range: {} ({})",
                index, self.len
            );
        }
        &self.buffer[index]
    }
}

#[derive(PartialEq)]
enum Gp0Mode {
    Command,
    ImageLoad {
        top_left: (u16, u16),
        resolution: (u16, u16),
        current_row: u16,
        current_col: u16,
    },
    ImageStore {
        top_left: (u16, u16),
        resolution: (u16, u16),
        current_row: u16,
        current_col: u16,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct Vertex {
    x: i32,
    y: i32,
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

#[derive(Clone, Copy)]
pub struct Colour {
    r: u8,
    g: u8,
    b: u8,
    m: u8,
}

impl Colour {
    pub fn blend(&mut self, other: Self) {
        self.r = (((self.r as u16) * (other.r as u16)) >> 7).min(255) as u8;
        self.g = (((self.g as u16) * (other.g as u16)) >> 7).min(255) as u8;
        self.b = (((self.b as u16) * (other.b as u16)) >> 7).min(255) as u8;
    }

    pub fn blend_with_background(&mut self, bg: Self, semi_transparency: u8) {
        match semi_transparency {
            0 => {
                self.r = (self.r / 2 + bg.r / 2).min(255);
                self.g = (self.g / 2 + bg.g / 2).min(255);
                self.b = (self.b / 2 + bg.b / 2).min(255);
            }
            1 => {
                self.r = (self.r + bg.r).min(255);
                self.g = (self.g + bg.g).min(255);
                self.b = (self.b + bg.b).min(255);
            }
            2 => {
                self.r = (bg.r - self.r).max(0);
                self.g = (bg.g - self.g).max(0);
                self.b = (bg.b - self.b).max(0);
            }
            3 => {
                self.r = (self.r / 4 + bg.r).min(255);
                self.g = (self.g / 4 + bg.g).min(255);
                self.b = (self.b / 4 + bg.b).min(255);
            }
            _ => unreachable!("wrong semi-transparency coeff"),
        }
    }

    pub fn from_5bit(color: u16) -> Self {
        let [lsb, msb] = color.to_le_bytes();
        Self::from_bytes(msb, lsb)
    }

    pub fn from_gp0(color: u32) -> Self {
        let r = (color & 0xFF) as u8;
        let g = ((color >> 8) & 0xFF) as u8;
        let b = ((color >> 16) & 0xFF) as u8;
        Colour { r, g, b, m: 0 }
    }

    pub fn from_bytes(msb: u8, lsb: u8) -> Self {
        let pixel = u16::from_le_bytes([lsb, msb]);

        let r = Gpu::convert_5bit_to_8bit(pixel & 0x1F);
        let g = Gpu::convert_5bit_to_8bit((pixel >> 5) & 0x1F);
        let b = Gpu::convert_5bit_to_8bit((pixel >> 10) & 0x1F);
        let m = (pixel >> 15) as u8;
        Colour { r, g, b, m }
    }

    pub fn to_le_bytes(&self) -> [u8; 2] {
        let r = ((self.r & 0xFF) >> 3) as u16;
        let g = ((self.g & 0xFF) >> 3) as u16;
        let b = ((self.b & 0xFF) >> 3) as u16;
        let m = self.m as u16;

        let pixel = (r | (g << 5) | (b << 10) | (m << 15)) as u16;
        // let pixel = (self.r as u16) | ((self.g as u16) << 5) | ((self.b as u16) << 10);
        pixel.to_le_bytes()
    }

    pub fn apply_dithering(&mut self, x: i32, y: i32) {
        let offset = DITHER_TABLE[(y & 3) as usize][(x & 3) as usize];
        self.r = self.r.saturating_add_signed(offset);
        self.g = self.g.saturating_add_signed(offset);
        self.b = self.b.saturating_add_signed(offset);
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
const OPAQUE: bool = false;
const SEMI_TRANS: bool = true;
const BLEND: bool = true;
const RAW: bool = false;
const QUAD: bool = true;
const TRI: bool = false;

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

    pub fn get_color(&self, gpu: &Gpu, index: u8) -> (u8, u8) {
        let pixel_lsb = gpu.vram[2 * (self.base_y * 1024 + self.base_x + index as usize)];
        let pixel_msb = gpu.vram[2 * (self.base_y * 1024 + self.base_x + index as usize) + 1];
        (pixel_lsb, pixel_msb)
    }
}
