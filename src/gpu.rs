use crossbeam::channel::{Receiver, Sender};
use std::{
    sync::{Arc, Mutex},
    thread::JoinHandle,
};

use crate::renderer::{RendererMsg, RendererResponse, RenderingContext};
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
    even: bool,
    in_hblank: bool,
    in_vblank: bool,
    read: u32,
    renderer_sender: Sender<RendererMsg>,
    renderer_receiver: Receiver<RendererResponse>,
    renderer_handle: JoinHandle<()>,
}

impl Gpu {
    pub fn new(
        renderer_sender: Sender<RendererMsg>,
        renderer_receiver: Receiver<RendererResponse>,
        renderer_handle: JoinHandle<()>,
    ) -> Gpu {
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
            even: true,
            in_vblank: false,
            in_hblank: false,
            read: 0,
            renderer_sender,
            renderer_receiver,
            renderer_handle,
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

                0x48 | 0x4C => {
                    self.gp0_mode = Gp0Mode::Polyline(false);
                    (2, Self::gp0_line_mono::<OPAQUE>)
                }
                0x4A | 0x4E => {
                    self.gp0_mode = Gp0Mode::Polyline(false);
                    (2, Self::gp0_line_mono::<SEMI_TRANS>)
                }
                0x58 | 0x5C => {
                    self.gp0_mode = Gp0Mode::Polyline(true);
                    (2, Self::gp0_line_shaded::<OPAQUE>)
                }
                0x5A | 0x5E => {
                    self.gp0_mode = Gp0Mode::Polyline(true);
                    (2, Self::gp0_line_shaded::<SEMI_TRANS>)
                }

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
        match &mut self.gp0_mode {
            Gp0Mode::Command => {
                self.gp0_command.push_word(val);
                if self.gp0_words_remaining == 0 {
                    (self.gp0_command_method)(self);
                }
            }
            Gp0Mode::ImageLoad {
                top_left,
                resolution,
                data
            } => {
                data.push(val);
                // self.process_cpu_to_vram_copy(val, top_left, resolution, current_row, current_col);
                if self.gp0_words_remaining == 0 {
                    self.renderer_sender.send(RendererMsg::CpuToVramCopy { top_left: *top_left, size: *resolution, data: data.clone() }).expect("ok");
                    self.gp0_mode = Gp0Mode::Command;
                }
            }
            Gp0Mode::ImageStore {
                current_word: _,
                data: _
            } => {
                // self.gp0_command.clear();
                // self.gp0_words_remaining = 0;
                // self.gp0_mode = Gp0Mode::Command;
                // self.gp0(val);
            }
            Gp0Mode::Polyline(color) => {
                if (val & 0xF000F000) == 0x50005000 {
                    self.gp0_mode = Gp0Mode::Command;
                    self.gp0_words_remaining = 0;
                    return;
                }
                self.gp0_command.push_word(val);
                if *color {
                    if self.gp0_command.len == 4 {
                        (self.gp0_command_method)(self);
                        self.gp0_command.buffer[0] = self.gp0_command[2];
                        self.gp0_command.buffer[1] = self.gp0_command[3];
                        self.gp0_command.len = self.gp0_command.len - 2;
                    }
                } else {
                    if self.gp0_command.len == 3 {
                        (self.gp0_command_method)(self);
                        self.gp0_command.buffer[1] = self.gp0_command[2];
                        self.gp0_command.len = self.gp0_command.len - 1;
                    }
                }
                self.gp0_words_remaining = 2;
            }
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
        let side = Self::gp0_vertex(self.gp0_command[2]);

        self.renderer_sender
            .send(RendererMsg::FillRect { v, side, c: color, ctx: self.rendering_ctx() })
            .expect("ok");
    }
    pub fn gp0_line_mono<const SEMI_TRANS: bool>(&mut self) {
        let color = Colour::from_gp0(self.gp0_command[0]);
        let v0 = Self::gp0_vertex(self.gp0_command[1]);
        let v1 = Self::gp0_vertex(self.gp0_command[2]);

        self.renderer_sender
            .send(RendererMsg::DrawLine {
                v0,
                v1,
                c0: color,
                semi_transparent: SEMI_TRANS,
                    ctx: self.rendering_ctx()
            })
            .expect("ok");
        // self.draw_line::<SEMI_TRANS>(v0, v1, color);
    }
    pub fn gp0_line_shaded<const SEMI_TRANS: bool>(&mut self) {
        let color0 = Colour::from_gp0(self.gp0_command[0]);
        let v0 = Self::gp0_vertex(self.gp0_command[1]);
        let color1 = Colour::from_gp0(self.gp0_command[2]);
        let v1 = Self::gp0_vertex(self.gp0_command[3]);

        self.renderer_sender
            .send(RendererMsg::DrawLineShaded {
                v0,
                v1,
                c0: color0,
                c1: color1,
                semi_transparent: SEMI_TRANS,
                    ctx: self.rendering_ctx()
            })
            .expect("ok");
        // self.draw_line_shaded::<SEMI_TRANS>(v0, v1, color0, color1);
    }

    pub fn gp0_rect_fixed<const SIDE: i32, const SEMI_TRANS: bool>(&mut self) {
        let color = Colour::from_gp0(self.gp0_command[0]);
        let v = Self::gp0_vertex(self.gp0_command[1]);

        self.renderer_sender
            .send(RendererMsg::DrawRectangle {
                v,
                side: Vertex { x: SIDE, y: SIDE },
                c: color,
                semi_transparent: SEMI_TRANS,
                    ctx: self.rendering_ctx()
            })
            .expect("ok");

        // self.draw_rectangle::<SEMI_TRANS>(v, Vertex { x: SIDE, y: SIDE }, color);
    }
    pub fn gp0_rect_variable<const SEMI_TRANS: bool>(&mut self) {
        let color = Colour::from_gp0(self.gp0_command[0]);
        let v = Self::gp0_vertex(self.gp0_command[1]);
        let side = Self::gp0_vertex(self.gp0_command[2]);

        self.renderer_sender
            .send(RendererMsg::DrawRectangle {
                v,
                side,
                c: color,
                semi_transparent: SEMI_TRANS,
                    ctx: self.rendering_ctx()
            })
            .expect("ok");
        // self.draw_rectangle::<SEMI_TRANS>(v, side, color);
    }
    pub fn gp0_rect_texture_fixed<const SIDE: i32, const SEMI_TRANS: bool, const BLEND: bool>(
        &mut self,
    ) {
        let color = Colour::from_gp0(self.gp0_command[0]);
        let v = Self::gp0_vertex(self.gp0_command[1]);
        let [_u, _v, clut] = Self::gp0_page_clut(self.gp0_command[2]);

        self.renderer_sender
            .send(RendererMsg::DrawRectangleTextured {
                v,
                side: Vertex { x: SIDE, y: SIDE },
                c: color,
                clut,
                _u,
                _v,
                semi_transparent: SEMI_TRANS,
                blend: BLEND,
                    ctx: self.rendering_ctx()
            })
            .expect("ok");
        // self.draw_rectangle_textured::<SEMI_TRANS, BLEND>(
        //     v,
        //     Vertex { x: SIDE, y: SIDE },
        //     color,
        //     clut,
        //     [_u, _v],
        // );
    }

    pub fn gp0_rect_texture_variable<const SEMI_TRANS: bool, const BLEND: bool>(&mut self) {
        let color = Colour::from_gp0(self.gp0_command[0]);
        let v = Self::gp0_vertex(self.gp0_command[1]);
        let [_u, _v, clut] = Self::gp0_page_clut(self.gp0_command[2]);
        let side = Self::gp0_vertex(self.gp0_command[3]);

        self.renderer_sender
            .send(RendererMsg::DrawRectangleTextured {
                v,
                side,
                c: color,
                clut,
                _u,
                _v,
                semi_transparent: SEMI_TRANS,
                blend: BLEND,
                    ctx: self.rendering_ctx()
            })
            .expect("ok");
        // self.draw_rectangle_textured::<SEMI_TRANS, BLEND>(v, side, color, clut, [_u, _v]);
    }

    pub fn gp0_poly_mono<const QUAD: bool, const SEMI_TRANS: bool>(&mut self) {
        let color = Colour::from_gp0(self.gp0_command[0]);
        let v0 = Self::gp0_vertex(self.gp0_command[1]);
        let v1 = Self::gp0_vertex(self.gp0_command[2]);
        let v2 = Self::gp0_vertex(self.gp0_command[3]);

        self.renderer_sender
            .send(RendererMsg::DrawTriangleMono {
                c: color,
                v0,
                v1,
                v2,
                semi_transparent: SEMI_TRANS,
                    ctx: self.rendering_ctx()
            })
            .expect("ok");
        // self.render_triangle_mono::<SEMI_TRANS>(color, &mut [v0, v1, v2]);
        if QUAD {
            let v3 = Self::gp0_vertex(self.gp0_command[4]);
            self.renderer_sender
                .send(RendererMsg::DrawTriangleMono {
                    c: color,
                    v0: v1,
                    v1: v2,
                    v2: v3,
                    semi_transparent: SEMI_TRANS,
                    ctx: self.rendering_ctx()
                })
                .expect("ok");
            // self.render_triangle_mono::<SEMI_TRANS>(color, &mut [v1, v2, v3]);
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

        let uv0 = [[_u0, _v0], [_u1, _v1], [_u2, _v2]];
        // let mut vs0 = [v0, v1, v2];
        // let mut cs0 = [c0, c1, c2];
        self.renderer_sender
            .send(RendererMsg::DrawTriangleTexturedShaded {
                vs: [v0, v1, v2],
                cs: [c0, c1, c2],
                uvs: uv0,
                page,
                clut,
                semi_transparent: SEMI_TRANS,
                blend: BLEND,
                    ctx: self.rendering_ctx()
            })
            .expect("ok");
        // self.draw_triangle_textured_shaded::<SEMI_TRANS, BLEND>(
        //     &mut cs0, clut, page, &mut vs0, &mut uv0,
        // );
        if QUAD {
            let c3 = Colour::from_gp0(self.gp0_command[9]);
            let v3 = Self::gp0_vertex(self.gp0_command[10]);
            let [_u3, _v3, _] = Self::gp0_page_clut(self.gp0_command[11]);
            let uv1 = [[_u1, _v1], [_u2, _v2], [_u3, _v3]];
            // let mut vs1 = [v1, v2, v3];
            // let mut cs1 = [c1, c2, c3];
            self.renderer_sender
                .send(RendererMsg::DrawTriangleTexturedShaded {
                    vs: [v1, v2, v3],
                    cs: [c1, c2, c3],
                    uvs: uv1,
                    page,
                    clut,
                    semi_transparent: SEMI_TRANS,
                    blend: BLEND,
                    ctx: self.rendering_ctx()
                })
                .expect("ok");
            // self.draw_triangle_textured_shaded::<SEMI_TRANS, BLEND>(
            //     &mut cs1, clut, page, &mut vs1, &mut uv1,
            // );
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

        let uv0 = [[_u0, _v0], [_u1, _v1], [_u2, _v2]];
        // self.draw_triangle_textured::<SEMI_TRANS, BLEND>(color, clut, page, &mut vs0, &mut uv0);
        self.renderer_sender
            .send(RendererMsg::DrawTriangleTextured {
                color,
                clut,
                page,
                vs: [v0, v1, v2],
                uvs: uv0,
                semi_transparent: SEMI_TRANS,
                blend: BLEND,
                    ctx: self.rendering_ctx()
            })
            .expect("ok");
        if QUAD {
            let v3 = Self::gp0_vertex(self.gp0_command[7]);
            let [_u3, _v3, _] = Self::gp0_page_clut(self.gp0_command[8]);
            let uv1 = [[_u1, _v1], [_u2, _v2], [_u3, _v3]];
            self.renderer_sender
                .send(RendererMsg::DrawTriangleTextured {
                    color,
                    clut,
                    page,
                    vs: [v1, v2, v3],
                    uvs: uv1,
                    semi_transparent: SEMI_TRANS,
                    blend: BLEND,
                    ctx: self.rendering_ctx()
                })
                .expect("ok");
            // self.draw_triangle_textured::<SEMI_TRANS, BLEND>(color, clut, page, &mut vs1, &mut uv1);
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
        self.renderer_sender
            .send(RendererMsg::DrawTriangleShaded {
                vs: [v0, v1, v2],
                cs: [c0, c1, c2],
                semi_transparent: SEMI_TRANS,
                    ctx: self.rendering_ctx()
            })
            .expect("ok");
        // self.draw_triangle_shaded::<SEMI_TRANS>(&mut [v0, v1, v2], &mut [c0, c1, c2]);
        if QUAD {
            let c3 = Self::gp0_colour(self.gp0_command[6]);
            let v3 = Self::gp0_vertex(self.gp0_command[7]);
            self.renderer_sender
                .send(RendererMsg::DrawTriangleShaded {
                    vs: [v1, v2, v3],
                    cs: [c1, c2, c3],
                    semi_transparent: SEMI_TRANS,
                    ctx: self.rendering_ctx()
                })
                .expect("ok");
            // self.draw_triangle_shaded::<SEMI_TRANS>(&mut [v1, v2, v3], &mut [c1, c2, c3]);
        }
    }
    pub fn gp0_vram_to_vram_blit(&mut self) {
        let src = Self::gp0_vertex(self.gp0_command[1]);
        let dst = Self::gp0_vertex(self.gp0_command[2]);
        let size = Self::gp0_vertex(self.gp0_command[3]);

        self.renderer_sender.send(
            RendererMsg::Vram2VramBlit { src, dst, size }).expect("ok");
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
            data: vec![]
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

        self.renderer_sender.send(RendererMsg::VramToCpuCopy { top_left: (x, y), size: (width as u16, height as u16) }).expect("ok");

        let msg = self.renderer_receiver.recv().expect("ok");
        let RendererResponse::VramToCpuData { data } = msg else {
            panic!("wrong response in sync code...");
        };



        self.gp0_mode = Gp0Mode::ImageStore {
            current_word: 0,
            data
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
            3 => TextureDepth::T15Bit,
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
            current_word,
            data,
        } = &mut self.gp0_mode
        {
            let word = data[*current_word];
            *current_word = *current_word + 1;
            self.gp0_words_remaining -= 1;
            if self.gp0_words_remaining == 0 {
                self.gp0_mode = Gp0Mode::Command;
            }
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



    pub fn render_fb(&self, framebuffer: Arc<Mutex<Vec<Color>>>, display_vram: bool) -> (usize, usize) {
        self.renderer_sender.send(RendererMsg::RenderFB {
            framebuffer: framebuffer,
            full_ram: display_vram,
            ctx: self.rendering_ctx() }).expect("ok");

        let msg = self.renderer_receiver.recv().expect("ok");
        match msg {
            RendererResponse::FBUpdated { width, height } => (width, height),
            _ => panic!("something went wrong.. sync call returned wrong response")
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

    pub fn rendering_ctx(&self) -> RenderingContext {
        RenderingContext {
            page_base_x: self.page_base_x,
            page_base_y: self.page_base_y,
            semi_transparency: self.semi_transparency,
            texture_depth: self.texture_depth,
            // dithering from 24 to 16 bits RGB
            dithering: self.dithering,
            draw_to_display: self.draw_to_display,
            // force "mask" bit of the pixel to 1 when writing to VRAM
            force_set_mask_bit: self.force_set_mask_bit,
            // don't draw to pixels which have the "mask" bit set
            preserve_masked_pixels: self.preserve_masked_pixels,
            texture_disable: self.texture_disable,
            hres: self.hres,
            vres: self.vres,
            // gpu itself always draws 15bit RGB, 24bit output must use external assets
            display_depth: self.display_depth,
            interlaced: self.interlaced,
            display_disabled: self.display_disabled,

            rectange_texture_x_flip: self.rectange_texture_x_flip,
            rectange_texture_y_flip: self.rectange_texture_y_flip,

            texture_window_x_mask: self.texture_window_x_mask,
            texture_window_y_mask: self.texture_window_y_mask,
            texture_window_x_offset: self.texture_window_x_offset,
            texture_window_y_offset: self.texture_window_y_offset,
            drawing_area_left: self.drawing_area_left,
            drawing_area_top: self.drawing_area_top,
            drawing_area_right: self.drawing_area_right,
            drawing_area_bottom: self.drawing_area_bottom,
            drawing_x_offset: self.drawing_x_offset,
            drawing_y_offset: self.drawing_y_offset,
            display_vram_x_start: self.display_vram_x_start,
            display_vram_y_start: self.display_vram_y_start,
            display_horiz_start: self.display_horiz_start,
            display_horiz_end: self.display_horiz_end,
            display_line_start: self.display_line_start,
            display_line_end: self.display_line_end,
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
pub enum TextureDepth {
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
pub struct HorizontalRes(u8, u8);

impl HorizontalRes {
    pub fn from_fields(hr1: u8, hr2: u8) -> HorizontalRes {
        let st = (hr2 & 1) | ((hr1 & 3) << 1);
        let hr = (hr2 & 1 << 4) | (hr1 & 3);
        HorizontalRes(hr,st)
    }
    fn into_status(self) -> u32 {
        let HorizontalRes(_, st) = self;

        (st as u32) << 16
    }

    pub fn into_pixels(self) -> usize {
        match self.0 {
            0 => 256,             // 256
            1 => 320,             // 320
            2 => 512,             // 512
            3 => 640,             // 640
            4 | 5 | 6 | 7 => 368, // 368
            _ => panic!("not implemented"),
        }
    }
}

#[derive(Clone, Copy)]
pub enum VerticalRes {
    Y240Lines = 0,
    // only for interlaced output
    Y480Lines = 1,
}

impl VerticalRes {
    pub fn into_pixels(self) -> usize {
        match self {
            Self::Y240Lines => 240,
            Self::Y480Lines => 480,
        }
    }
}

#[derive(Clone, Copy)]
enum VMode {
    // 480i60H
    Ntsc = 0,
    // 576i50Hz
    Pal = 1,
}

#[derive(Clone, Copy)]
pub enum DisplayDepth {
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
        data: Vec<u32>
    },
    ImageStore {
        current_word: usize,
        data: Vec<u32>,
    },
    Polyline(bool),
}

#[derive(Debug, Clone, Copy)]
pub struct Vertex {
    pub x: i32,
    pub y: i32,
}
#[derive(Clone, Copy)]
pub struct Colour {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub m: u8,
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
                self.r = (self.r as i32 / 2 + bg.r as i32 / 2).clamp(0,255) as u8;
                self.g = (self.g as i32 / 2 + bg.g as i32 / 2).min(255) as u8;
                self.b = (self.b as i32 / 2 + bg.b as i32 / 2).min(255) as u8;
            }
            1 => {
                self.r = (self.r as i32 + bg.r as i32).min(255) as u8;
                self.g = (self.g as i32 + bg.g as i32).min(255) as u8;
                self.b = (self.b as i32 + bg.b as i32).min(255) as u8;
            }
            2 => {
                self.r = (bg.r as i32 - self.r as i32).max(0) as u8;
                self.g = (bg.g as i32 - self.g as i32).max(0) as u8;
                self.b = (bg.b as i32 - self.b as i32).max(0) as u8;
            }
            3 => {
                self.r = (self.r as i32 / 4 + bg.r as i32).min(255) as u8;
                self.g = (self.g as i32 / 4 + bg.g as i32).min(255) as u8;
                self.b = (self.b as i32 / 4 + bg.b as i32).min(255) as u8;
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

        let r = convert_5bit_to_8bit(pixel & 0x1F);
        let g = convert_5bit_to_8bit((pixel >> 5) & 0x1F);
        let b = convert_5bit_to_8bit((pixel >> 10) & 0x1F);
        let m = (pixel >> 15) as u8;
        Colour { r, g, b, m }
    }

    pub fn is_black(&self) -> bool {
        self.r == 0 && self.b == 0 && self.g == 0 && self.m == 0
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

pub const OPAQUE: bool = false;
pub const SEMI_TRANS: bool = true;
pub const BLEND: bool = true;
pub const RAW: bool = false;
pub const QUAD: bool = true;
pub const TRI: bool = false;

const DITHER_TABLE: &[[i8; 4]; 4] = &[
    [-4, 0, -3, 1],
    [2, -2, 3, 1],
    [-3, 1, -4, 0],
    [3, -1, 2, -2],
];

    pub fn convert_5bit_to_8bit(color: u16) -> u8 {
        // Note it is probably a lot faster to use a 32-entry lookup table than doing this calculation live
        // (f64::from(color) * 255.0 / 31.0).round() as u8
        FIVE_BIT_TO_8BIT[color as usize]
    }
