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
    gp0_mode: Gp0Mode
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
            gp0_mode: Gp0Mode::Command
        }
    }

    pub fn gp0(&mut self, val: u32) {
        if self.gp0_words_remaining == 0 {
            let opcode = (val >> 24) & 0xff;
            let (len, method):(u32, fn (&mut Gpu)) = match opcode {
                0x00 => (1, Gpu::gp0_nop),
                0x01 => (1, Gpu::gp0_clear_cache),
                0x28 => (5, Gpu::gp0_quad_mono_opaque),
                0x2C => (9, Gpu::gp0_quad_texture_blend_opaque),
                0x30 => (6, Gpu::gp0_triangle_shaded_opaque),
                0x38 => (8, Gpu::gp0_quad_shaded_opaque),
                0xA0 => (3, Gpu::gp0_image_load),
                0xC0 => (3, Gpu::gp0_image_store),
                0xE1 => (1, Gpu::gp0_draw_mode),
                0xE2 => (1, Gpu::gp0_texture_window),
                0xE3 => (1, Gpu::gp0_drawing_area_top_left),
                0xE4 => (1, Gpu::gp0_drawing_area_bottom_right),
                0xE5 => (1, Gpu::gp0_drawing_offset),
                0xE6 => (1, Gpu::gp0_mask_bit_setting),
                _ => panic!("Unhandled GP0 command {:08X} opcode: {:02X}", val, opcode),
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
            },
            Gp0Mode::ImageLoad => {
                // FIXME: should copy pixels to VRAM
                if self.gp0_words_remaining == 0 {
                    self.gp0_mode = Gp0Mode::Command;
                }
            }
        }
    }
    pub fn gp0_nop(&mut self) {}
    pub fn gp0_clear_cache(&mut self) {}
    pub fn gp0_quad_mono_opaque(&mut self) {
        println!("draw quad!");
    }
    pub fn gp0_quad_texture_blend_opaque(&mut self) {
        println!("draw texture quad blend opaque!");
    }
    pub fn gp0_triangle_shaded_opaque(&mut self) {
        println!("draw triangle!");
    }
    pub fn gp0_quad_shaded_opaque(&mut self) {
        println!("draw shaded quad!");
    }
    pub fn gp0_image_load(&mut self) {
        let res = self.gp0_command[2];
        let width = res & 0xffff;
        let height = res >> 16;

        let imgsize = width * height;
        // rounding so we have 16 bit of padding in last word
        let imgsize = (imgsize + 1) & !1;
        self.gp0_words_remaining = imgsize / 2;
        self.gp0_mode = Gp0Mode::ImageLoad;

    }
    pub fn gp0_image_store(&mut self) {
        let res = self.gp0_command[2];
        let width = res & 0xffff;
        let height = res >> 16;
        println!("Unhandled image store: {}x{}", width, height);
    }
    pub fn gp0_draw_mode(&mut self) {
        let val:u32 = self.gp0_command[0];
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
        let val:u32 = self.gp0_command[0];
        self.texture_window_x_mask = (val & 0x1f) as u8;
        self.texture_window_y_mask = ((val >> 5) & 0x1f) as u8;
        self.texture_window_x_offset = ((val >> 10) & 0x1f) as u8;
        self.texture_window_y_offset = ((val >> 15) & 0x1f) as u8;
    }
    pub fn gp0_drawing_area_top_left(&mut self) {
        let val:u32 = self.gp0_command[0];
        self.drawing_area_top = ((val >> 10) & 0x3ff) as u16;
        self.drawing_area_left = (val & 0x3ff) as u16;
    }
    pub fn gp0_drawing_area_bottom_right(&mut self) {
        let val:u32 = self.gp0_command[0];
        self.drawing_area_bottom = ((val >> 10) & 0x3ff) as u16;
        self.drawing_area_right = (val & 0x3ff) as u16;
    }
    pub fn gp0_drawing_offset(&mut self) {
        let val:u32 = self.gp0_command[0];
        let x = (val & 0x7ff) as u16;
        let y = ((val >> 11) & 0x7ff) as u16;

        self.drawing_x_offset = ((x << 5) as i16) >> 5;
        self.drawing_y_offset = ((y << 5) as i16) >> 5;
    }
    pub fn gp0_mask_bit_setting(&mut self) {
        let val:u32 = self.gp0_command[0];
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
        // FIXME: temporary hack, setting vres to 1 locks the bios
        // r |= (self.vres as u32) << 19;
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
        r |= 1 << 31;

        let dma_request = match self.dma_direction {
            DmaDirection::Off => 0,
            DmaDirection::Fifo => 1,
            DmaDirection::CpuToGp0 => (r >> 28) & 1,
            DmaDirection::VRamToCpu => (r >> 27) & 1,
        };

        r |= dma_request << 25;

        r
    }
}

#[derive(Clone, Copy)]
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
    ImageLoad
}
