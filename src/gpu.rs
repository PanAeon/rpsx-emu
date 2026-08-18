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
    interrupt: bool,
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
        }
    }

    pub fn gp0(&mut self, val: u32) {
        if self.gp0_words_remaining == 0 {
            let opcode = (val >> 24) & 0xff;
            let (len, method): (u32, fn(&mut Gpu)) = match opcode {
                0x00 => (1, Gpu::gp0_nop),
                0x01 => (1, Gpu::gp0_clear_cache),
                0x02 => (3, Gpu::gp0_fill_rect),
                // 0x28 => (5, Gpu::gp0_nop),
                0x28 => (5, Gpu::gp0_quad_mono_opaque),
                // 0x2C => (9, Gpu::gp0_nop),
                0x2C => (9, Gpu::gp0_quad_texture_blend_opaque),
                // 0x30 => (6, Gpu::gp0_nop),
                0x30 => (6, Gpu::gp0_triangle_shaded_opaque),
                // 0x38 => (8, Gpu::gp0_nop),
                0x38 => (8, Gpu::gp0_quad_shaded_opaque),
                0x68 => (2, Gpu::gp0_monochrome_rect_1x1),
                0xA0 => (3, Gpu::gp0_image_load),
                0xC0 => (3, Gpu::gp0_image_store),
                0xE1 => (1, Gpu::gp0_draw_mode),
                0xE2 => (1, Gpu::gp0_texture_window),
                0xE3 => (1, Gpu::gp0_drawing_area_top_left),
                0xE4 => (1, Gpu::gp0_drawing_area_bottom_right),
                0xE5 => (1, Gpu::gp0_drawing_offset),
                0xE6 => (1, Gpu::gp0_mask_bit_setting),
                _ => panic!(
                    "Unhandled GP0 command 0x{:08X} opcode: 0x{:02X}",
                    val, opcode
                ),
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

    pub fn gp0_position(pos: u32) -> [u32; 2] {
        // Parameter word contains the pixel coordinates.
        // Vertex coordinates are technically signed 11-bit integers, and the drawing offset needs to be
        // applied, but let's ignore that for now
        let x = pos & 0x3FF;
        let y = (pos >> 16) & 0x1FF;

        [x, y]
    }
    pub fn gp0_colour(color: u32) -> Colour {
        let r = (color & 0xFF) as u8;
        let g = ((color >> 8) & 0xFF) as u8;
        let b = ((color >> 16) & 0xFF) as u8;
        Colour { r, g, b }
    }
    pub fn gp0_color(color: u32) -> [u8; 2] {
        let r = (color & 0xFF) >> 3;
        let g = ((color >> 8) & 0xFF) >> 3;
        let b = ((color >> 16) & 0xFf) >> 3;

        let pixel = (r | (g << 5) | (b << 10)) as u16;
        pixel.to_le_bytes()
    }
    pub fn gp0_vertex(v: u32) -> Vertex {
        let x = (v & 0x3FF) as i32;
        let y = ((v >> 16) & 0x1FF) as i32;
        Vertex { x, y }
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
        println!("gp0 fill rect");
    }
    pub fn render_triangle_mono_opaque(
        &mut self,
        color: [u8; 2],
        v0: &mut Vertex,
        v1: &mut Vertex,
        v2: Vertex,
    ) {
        ensure_vertex_order(v0, v1, v2);
        let [pixel_lsb, pixel_msb] = color;

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
                if is_inside_triangle(p, *v0, *v1, v2) {
                    let vram_addr = 2 * (y * 1024 + x) as usize;
                    self.vram[vram_addr] = pixel_lsb;
                    self.vram[vram_addr + 1] = pixel_msb;
                }
            }
        }

        // let [pixel_lsb, pixel_msb] = Self::gp0_color(self.gp0_command[0]);
    }

    pub fn render_triangle_shaded_opaque(
        &mut self,
        v0: &mut Vertex,
        v1: &mut Vertex,
        v2: Vertex,
        c0: &mut Colour,
        c1: &mut Colour,
        c2: Colour,
    ) {
        ensure_vertex_and_color_order(v0, v1, v2, c0, c1, c2);

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
                if is_inside_triangle(p, *v0, *v1, v2) {
                    let lambda = compute_barycentric_coordinates(p, *v0, *v1, v2);
                    let color = interpolate_color(lambda, [*c0, *c1, c2]);
                    let color = apply_dithering(color, p);

                    let r = ((color.r & 0xFF) >> 3) as u16;
                    let g = ((color.g & 0xFF) >> 3) as u16;
                    let b = ((color.b & 0xFF) >> 3) as u16;

                    let pixel = (r | (g << 5) | (b << 10)) as u16;
                    let [pixel_lsb, pixel_msb] = pixel.to_le_bytes();

                    let vram_addr = 2 * (y * 1024 + x) as usize;

                    self.vram[vram_addr] = pixel_lsb;
                    self.vram[vram_addr + 1] = pixel_msb;
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
    pub fn render_triangle_texture_blend(
        &mut self,
        clut: u16,
        page: u16,
        vs: &mut [Vertex; 3],
        uv: &mut [[u16; 2]; 3],
    ) {
        ensure_vertex_order2(vs, uv);

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
        let clut_x = ((clut & 0x1f) as usize) * 16;
        let clut_y = ((clut >> 6) & 0x1FF) as usize; // y coord 0-511 (on v0 GPU)

        // println!("uv: {:?}", texture_depth);

        for y in min_y..max_y {
            for x in min_x..max_x {
                let p = Vertex { x, y };
                if is_inside_triangle(p, vs[0], vs[1], vs[2]) {
                    let lambda = compute_barycentric_coordinates(p, vs[0], vs[1], vs[2]);
                    let [uv_x, uv_y] = compute_normal_coordinates(lambda, uv);


                    match texture_depth {
                        TextureDepth::T4Bit => {
                            // Width 4096...
                            let pixel = self.vram[page_base_y * 2048 + uv_y * 4096 + 2*page_base_x + uv_x/2];
                            let pixel = (pixel >> 4 * (uv_x & 1)) & 0xF;
                            if pixel == 0 {
                                continue;
                            }

                            let mut pixel_lsb = self.vram[2 * (clut_y * 1024 + clut_x + pixel as usize)];
                            let mut pixel_msb = self.vram[2 * (clut_y * 1024 + clut_x + pixel as usize) + 1];

                            let vram_addr = 2 * (y * 1024 + x) as usize;

                            if pixel_msb >> 7 == 1 {
                                let background_lsb = self.vram[vram_addr];
                                let background_msb = self.vram[vram_addr + 1];
                                let pixel = u16::from_le_bytes([pixel_lsb, pixel_msb]);

                                let mut r = Gpu::convert_5bit_to_8bit(pixel & 0x1F) as u16 as i16;
                                let mut g = Gpu::convert_5bit_to_8bit((pixel >> 5) & 0x1F) as u16 as i16;
                                let mut b = Gpu::convert_5bit_to_8bit((pixel >> 10) & 0x1F) as u16 as i16;

                                let background = u16::from_le_bytes([background_lsb, background_msb]);

                                let br = Gpu::convert_5bit_to_8bit(background & 0x1F) as u16 as i16;
                                let bg = Gpu::convert_5bit_to_8bit((background >> 5) & 0x1F) as u16 as i16;
                                let bb = Gpu::convert_5bit_to_8bit((background >> 10) & 0x1F) as u16 as i16;

                                //     (0=B/2+F/2, 1=B+F, 2=B-F, 3=B+F/4)
                                match (self.semi_transparency) {
                                    0 => {
                                        r = (r / 2 + br / 2).min(255);
                                        g = (g / 2 + bg / 2).min(255);
                                        b = (b / 2 + bb / 2).min(255);
                                    },
                                    1 => {
                                        r = (r  + br ).min(255);
                                        g = (g  + bg ).min(255);
                                        b = (b  + bb ).min(255);
                                    },
                                    2 => {
                                        r = ( br - r ).max(0);
                                        g = ( bg - b ).max(0);
                                        b = ( bb - b ).max(0);
                                    },
                                    3 => {
                                        r = (r / 4 + br ).min(255);
                                        g = (g / 4 + bg ).min(255);
                                        b = (b / 4 + bb ).min(255);
                                    }
                                    _ => unreachable!("semi_transparency should be 0..=3")
                                }
                                let pixel = (r as u16 | ((g as u16) << 5) | ((b as u16) << 10)) as u16;
                                let [lsb, msb] = pixel.to_le_bytes();
                                pixel_lsb = lsb;
                                pixel_msb = msb;

                            }

                            if pixel_lsb == 0 && pixel_msb == 0 {
                                continue;
                            }


                            // self.vram[vram_addr] = pixel;
                            // self.vram[vram_addr + 1] = 0;
                            self.vram[vram_addr] = pixel_lsb;
                            self.vram[vram_addr + 1] = pixel_msb;
                            
                        },
                        TextureDepth::T8Bit => {
                            // Width 4096...
                            let pixel = self.vram[page_base_y * 2048 + uv_y * 2048 + 2*page_base_x + uv_x];
                            if pixel == 0 {
                                continue;
                            }

                            let pixel_lsb = self.vram[2 * (clut_y * 1024 + clut_x + pixel as usize)];
                            let pixel_msb = self.vram[2 * (clut_y * 1024 + clut_x + pixel as usize) + 1];

                            let vram_addr = 2 * (y * 1024 + x) as usize;

                            // self.vram[vram_addr] = pixel;
                            // self.vram[vram_addr + 1] = 0;
                            self.vram[vram_addr] = pixel_lsb;
                            self.vram[vram_addr + 1] = pixel_msb;
                        },
                        TextureDepth::T15Bit => {
                            let texture_x = page_base_x + uv_x;
                            let texture_y = page_base_y + uv_y;
                            let pixel_lsb = self.vram[2 * (texture_y * 1024 + texture_x)];
                            let pixel_msb = self.vram[2 * (texture_y * 1024 + texture_x) + 1];
                            if pixel_lsb == 0 && pixel_msb == 0 {
                                continue;
                            }
                            let vram_addr = 2 * (y * 1024 + x) as usize;

                            self.vram[vram_addr] = pixel_lsb;
                            self.vram[vram_addr + 1] = pixel_msb;
                        }
                    };
                    // let pixel_lsb = self.vram[2 * (texture_y * 1024 + texture_x)];
                    // let pixel_msb = self.vram[2 * (texture_y * 1024 + texture_x) + 1];
                    //
                    // Colour { r, g, b }
                    // let color = interpolate_color(lambda, [*c0, *c1, c2]);
                    // let color = apply_dithering(color, p);

                    // let r = ((color.r & 0xFF) >> 3) as u16;
                    // let g = ((color.g & 0xFF) >> 3) as u16;
                    // let b = ((color.b & 0xFF) >> 3) as  u16;

                    // let pixel = (r | (g << 5) | (b << 10)) as u16;
                    // let [pixel_lsb, pixel_msb] = pixel.to_le_bytes();

                    // let vram_addr = 2 * (y * 1024 + x) as usize;
                    //
                    // self.vram[vram_addr] = pixel_lsb;
                    // self.vram[vram_addr + 1] = pixel_msb;
                }
            }
        }

        // let [pixel_lsb, pixel_msb] = Self::gp0_color(self.gp0_command[0]);
    }

    pub fn gp0_quad_mono_opaque(&mut self) {
        let color = Self::gp0_color(self.gp0_command[0]);
        let mut v0 = Self::gp0_vertex(self.gp0_command[1]);
        let mut v1 = Self::gp0_vertex(self.gp0_command[2]);
        let mut v2 = Self::gp0_vertex(self.gp0_command[3]);
        let v3 = Self::gp0_vertex(self.gp0_command[4]);

        self.render_triangle_mono_opaque(color, &mut v0, &mut v1, v2);
        self.render_triangle_mono_opaque(color, &mut v1, &mut v2, v3);
    }

    pub fn gp0_quad_texture_blend_opaque(&mut self) {
        // let color = Self::gp0_color(self.gp0_command[0]);
        let v0 = Self::gp0_vertex(self.gp0_command[1]);
        let [_u0, _v0, clut] = Self::gp0_page_clut(self.gp0_command[2]);
        let v1 = Self::gp0_vertex(self.gp0_command[3]);
        let [_u1, _v1, page] = Self::gp0_page_clut(self.gp0_command[4]);
        let v2 = Self::gp0_vertex(self.gp0_command[5]);
        let [_u2, _v2, _] = Self::gp0_page_clut(self.gp0_command[6]);
        let v3 = Self::gp0_vertex(self.gp0_command[7]);
        let [_u3, _v3, _] = Self::gp0_page_clut(self.gp0_command[8]);
        // 9
        // self.render_triangle_mono_opaque(color, &mut v0, &mut v1, v2);
        // self.render_triangle_mono_opaque(color, &mut v1, &mut v2, v3);
        let mut uv0 = [[_u0, _v0], [_u1, _v1], [_u2, _v2]];
        let mut vs0 = [v0, v1, v2];
        let mut uv1 = [[_u1, _v1], [_u2, _v2], [_u3, _v3]];
        let mut vs1 = [v1, v2, v3];
        self.render_triangle_texture_blend(clut, page, &mut vs0, &mut uv0);
        self.render_triangle_texture_blend(clut, page, &mut vs1, &mut uv1);
    }
    pub fn gp0_triangle_shaded_opaque(&mut self) {
        // 6
        let mut c0 = Self::gp0_colour(self.gp0_command[0]);
        let mut v0 = Self::gp0_vertex(self.gp0_command[1]);
        let mut c1 = Self::gp0_colour(self.gp0_command[2]);
        let mut v1 = Self::gp0_vertex(self.gp0_command[3]);
        let c2 = Self::gp0_colour(self.gp0_command[4]);
        let mut v2 = Self::gp0_vertex(self.gp0_command[5]);
        self.render_triangle_shaded_opaque(&mut v0, &mut v1, v2, &mut c0, &mut c1, c2);
        // self.render_triangle_mono_opaque(c0, &mut v0, &mut v1, v2);
    }
    pub fn gp0_quad_shaded_opaque(&mut self) {
        // 8
        // println!("draw shaded quad!");
        let mut c0 = Self::gp0_colour(self.gp0_command[0]);
        let mut v0 = Self::gp0_vertex(self.gp0_command[1]);
        let mut c1 = Self::gp0_colour(self.gp0_command[2]);
        let mut v1 = Self::gp0_vertex(self.gp0_command[3]);
        let mut c2 = Self::gp0_colour(self.gp0_command[4]);
        let mut v2 = Self::gp0_vertex(self.gp0_command[5]);
        let c3 = Self::gp0_colour(self.gp0_command[6]);
        let mut v3 = Self::gp0_vertex(self.gp0_command[7]);

        self.render_triangle_shaded_opaque(&mut v0, &mut v1, v2, &mut c0, &mut c1, c2);
        self.render_triangle_shaded_opaque(&mut v1, &mut v2, v3, &mut c1, &mut c2, c3);
        // self.render_triangle_mono_opaque(c0, &mut v0, &mut v1, v2);
        // self.render_triangle_mono_opaque(c0, &mut v1, &mut v2, v3);
    }
    pub fn gp0_monochrome_rect_1x1(&mut self) {
        let [pixel_lsb, pixel_msb] = Self::gp0_color(self.gp0_command[0]);
        let [x, y] = Self::gp0_position(self.gp0_command[1]);
        // println!("monochrome rect 1x1");
        let vram_addr = 2 * (y * 1024 + x) as usize;
        self.vram[vram_addr] = pixel_lsb;
        self.vram[vram_addr + 1] = pixel_msb;
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
        let res = self.gp0_command[2];
        let width = res & 0xffff;
        let height = res >> 16;
        println!("Unhandled image store: {}x{}", width, height);
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
    pub fn gp1_reset(&mut self, val: u32) {
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
        self.gp1_reset_command_buffer(0);
    }

    pub fn gp1_reset_command_buffer(&mut self, _: u32) {
        self.gp0_command.clear();
        self.gp0_words_remaining = 0;
        self.gp0_mode = Gp0Mode::Command;
        // TODO: should also clear the command FIFO
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
        match v {
            0 | 1 => {} // nop
            //           02h     = Read Texture Window setting  ;GP0(E2h) ;20bit/MSBs=Nothing
            //           03h     = Read Draw area top left      ;GP0(E3h) ;19bit/MSBs=Nothing
            //           04h     = Read Draw area bottom right  ;GP0(E4h) ;19bit/MSBs=Nothing
            //           05h     = Read Draw offset             ;GP0(E5h) ;22bit
            6 | 7 => {} // nop
            _ => unreachable!("v could be 0...7"),
        }
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

    pub fn read(&self) -> u32 {
        // NOT implemented for now
        0
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

        // Ready to receive command:
        r |= 1 << 26;
        // Ready to send VRAM to CPU
        r |= 1 << 27;
        // ready to receive DMA block
        r |= 1 << 28;

        // should change depending even/odd/vblank line
        // (0=Even or Vblank, 1=Odd)
        // In 480-lines mode, bit31 changes per frame. And in 240-lines mode, the bit changes per scanline. In 480-lines mode, bit31 changes per frame. And in 240-lines mode, the bit changes per scanline.
        r |= ((!self.even & !self.in_vblank) as u32) << 31;

        let dma_request = match self.dma_direction {
            DmaDirection::Off => 0,
            DmaDirection::Fifo => 1,
            DmaDirection::CpuToGp0 => (r >> 28) & 1,
            DmaDirection::VRamToCpu => (r >> 27) & 1,
        };

        r |= dma_request << 25;

        r
    }
    pub fn load<T: Addressable>(&self, offset: u32) -> T {
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
    }
    pub fn exit_hsync(&mut self) {
        self.in_hblank = false;
    }
    // pub fn get_clock_divider(&self) -> u16 {
    //     match self.hres.0 {
    //         256 => 10,
    //         320 => 8,
    //         368 => 7,
    //         512 => 5,
    //         640 => 4,
    //         _ => panic!("not implemented")
    //     }
    // }
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

#[derive(Clone, Copy)]
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

enum Gp0Mode {
    Command,
    ImageLoad {
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
fn ensure_vertex_order(v0: &mut Vertex, v1: &mut Vertex, v2: Vertex) {
    let cross_product_z = (v1.x - v0.x) * (v2.y - v0.y) - (v1.y - v0.y) * (v2.x - v0.x);
    if cross_product_z < 0 {
        std::mem::swap(v0, v1);
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

fn ensure_vertex_and_color_order(
    v0: &mut Vertex,
    v1: &mut Vertex,
    v2: Vertex,
    c0: &mut Colour,
    c1: &mut Colour,
    _: Colour,
) {
    let cross_product_z = (v1.x - v0.x) * (v2.y - v0.y) - (v1.y - v0.y) * (v2.x - v0.x);
    if cross_product_z < 0 {
        std::mem::swap(v0, v1);
        std::mem::swap(c0, c1);
    }
}

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
struct Colour {
    r: u8,
    g: u8,
    b: u8,
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

    Colour { r, g, b }
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
    }
}
