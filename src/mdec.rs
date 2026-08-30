use std::collections::VecDeque;

use num_enum::FromPrimitive;

use crate::memory_bus::Addressable;



pub struct Mdec {
    status: Status,
    control: Control,

    params_remaining: u16,
    command: Option<Command>,
    output_fifo: VecDeque<u32>,

    scale_table: [i16; 64],
    luminance_table: [u8; 64],
    chrominance_table: [u8; 64]


}

impl Mdec {
    pub fn new() -> Self {
        Mdec {
            status: Status::default(),
            control: Control::default(),
            params_remaining: 0,
            command: None,
            output_fifo: VecDeque::default(),
            scale_table: [0; 64],
            luminance_table: [0; 64],
            chrominance_table: [0; 64]
            
        }
    }
    pub fn load<T:Addressable>(&mut self, offset: u32) -> T {
         if T::width() as u8 != 4 {
            panic!("Unhandled MDEC load ({})", T::width() as u8);
        }
        match offset {
            0 => T::from_u32(self.get_response_data()),
            4 => T::from_u32(self.get_status()),
            _ => panic!("unhandled mdec load offset: 0x{:X}", offset)
        }
    }
    pub fn store<T:Addressable>(&mut self, offset: u32, value: T) {
         if T::width() as u8 != 4 {
            panic!("Unhandled MDEC store ({})", T::width() as u8);
        }
        match offset {
            0 => self.command_or_param(value.as_u32()),
            4 => self.set_control(value.as_u32()),
            _ => panic!("unhandled mdec store offset: 0x{:X}", offset)
        }
    }

    pub fn get_status(&mut self) -> u32 {
        // self.status.set_parameter_words_remaining(self.command_remaining.wrapping_sub(1) as u32);
        // self.status.set_data_in_fifo_full(false);
        // self.status.set_data_out_fifo_empty(false);
        // self.status.set_data_in_request(true);
        // self.status.set_data_out_request(true);
        // let busy = self.command_handler != (Mdec::handle_command); 
        // self.status.set_command_busy(busy);
        (self.status.0 & !0xFFFF) | self.params_remaining.wrapping_sub(1) as u32
    }

    pub fn set_control(&mut self, val: u32) {
        self.control.0 = val;
        if self.control.reset() {
            self.status.0 = 0x8004_0000;
            self.params_remaining = 0;
            self.command = None;
        }
        self.status.set_data_in_request(self.control.enable_data_in());
        self.status.set_data_out_request(self.control.enable_data_out());
    }

    pub fn get_response_data(&mut self) -> u32 {
        let data = self.output_fifo.pop_front().unwrap_or(0xFE00_FE00);
        if self.output_fifo.is_empty() {
            self.status.set_data_out_fifo_empty(true);
        }
        data
    }

    pub fn command_or_param(&mut self, val: u32) {
        let cmd = self.command.take();
        // println!("mdec command or param");

        match cmd {
            None => self.decode_command(val),
            Some(mut collected) => {
                self.params_remaining -= 1;
                collected.parameters.push(val);

                if self.params_remaining == 0 {
                    self.status.set_command_busy(false);
                    self.handle_command(&collected);
                } else {
                    self.command = Some(collected);
                }
            }
        }
    }

    pub fn decode_command(&mut self, data: u32) {
        let cmd = CommandWord(data);

        self.status.set_cmd_data_out(cmd.data_out());

        match cmd.command() {
            0 | 4..8 => {
                self.params_remaining = cmd.params_len() + 1;
            },
            1 => {
                self.status.set_command_busy(true);
                self.params_remaining = cmd.params_len();
                self.command = Some(Command { 
                    command_type: CommandType::DecodeMacroblock {
                        depth: cmd.output_depth(),
                        is_signed: cmd.output_signed(),
                        b15: cmd.output_bit5() },
                    parameters: vec![]
                });
            },
            2 => {
                self.status.set_command_busy(true);
                self.params_remaining = match cmd.color() {
                    Color::Luminance => 16,
                    Color::LuminanceAndColor => 32,
                }; 
                self.command = Some(Command { command_type: CommandType::SetQuantTable(cmd.color()), parameters: vec![] });
            },
            3 => {
                self.status.set_command_busy(true);
                self.params_remaining = 32;
                self.command = Some(Command { command_type: CommandType::SetScaleTable, parameters: vec![] });

            },
            _ => unreachable!()
        }
    }


    fn handle_command(&mut self, cmd: &Command) {
        match cmd.command_type {
            CommandType::DecodeMacroblock { depth, is_signed, b15 } => self.decode_macoblock(depth, is_signed, b15, &cmd.parameters),
            CommandType::SetQuantTable(color) => {
                let raw_bytes: &[u8] = bytemuck::cast_slice(cmd.parameters.as_slice());
                self.luminance_table.copy_from_slice(&raw_bytes[0..64]);
                if color == Color::LuminanceAndColor {
                    self.chrominance_table.copy_from_slice(&raw_bytes[64..128]);
                }
            },
            CommandType::SetScaleTable => {
                let scale_table: [u32; 32] = cmd
                    .parameters
                    .clone()
                    .try_into()
                    .expect("scale table is 32 words");
                self.scale_table = bytemuck::cast(scale_table);
            },
        }

    }

    fn decode_macoblock(&mut self, depth: Depth, is_signed: bool, b15: bool, parameters: &Vec<u32>) {
        let raw: &[u16] = bytemuck::cast_slice(parameters.as_slice());
        let mut source: VecDeque<u16> = raw.iter().copied().collect();
        println!("decode macroblock");

         match depth {
                    Depth::Bit4 => {
                        let block = self.decode_block(&mut source, &self.luminance_table);
                        let pixels = level_shift_4bpp(block);

                        let words: [u32; 8] = bytemuck::cast(pixels);
                        self.output_fifo.extend(words);
                    }

                    Depth::Bit8 => {
                        let block = self.decode_block(&mut source, &self.luminance_table);
                        let pixels = level_shift_8bpp(block);

                        let words: [u32; 16] = bytemuck::cast(pixels);
                        self.output_fifo.extend(words);
                    }

                    Depth::Bit15 => {
                        while !source.is_empty() {
                            // Skip any trailing padding before checking if there's real data left
                            while source.front() == Some(&0xFE00) {
                                source.pop_front();
                            }

                            if source.is_empty() {
                                break;
                            }

                            let cr = self.decode_block(&mut source, &self.chrominance_table);
                            let cb = self.decode_block(&mut source, &self.chrominance_table);

                            let mut dst = [0u16; 256];

                            let y1 = self.decode_block(&mut source, &self.luminance_table);
                            yuv_to_rgb15_block(&cr, &cb, &y1, (0, 0), is_signed, b15, &mut dst);

                            let y2 = self.decode_block(&mut source, &self.luminance_table);
                            yuv_to_rgb15_block(&cr, &cb, &y2, (8, 0), is_signed, b15, &mut dst);

                            let y3 = self.decode_block(&mut source, &self.luminance_table);
                            yuv_to_rgb15_block(&cr, &cb, &y3, (0, 8), is_signed, b15, &mut dst);

                            let y4 = self.decode_block(&mut source, &self.luminance_table);
                            yuv_to_rgb15_block(&cr, &cb, &y4, (8, 8), is_signed, b15, &mut dst);

                            let words: &[u32] = bytemuck::cast_slice(&dst);
                            self.output_fifo.extend(words);
                        }
                    }

                    Depth::Bit24 => {
                        while !source.is_empty() {
                            // Skip any trailing padding before checking if there's real data left
                            while source.front() == Some(&0xFE00) {
                                source.pop_front();
                            }

                            if source.is_empty() {
                                break;
                            }

                            let cr = self.decode_block(&mut source, &self.chrominance_table);
                            let cb = self.decode_block(&mut source, &self.chrominance_table);

                            let mut dst = [0u8; 768];

                            let y1 = self.decode_block(&mut source, &self.luminance_table);
                            yuv_to_rgb24_block(&cr, &cb, &y1, (0, 0), is_signed, &mut dst);

                            let y2 = self.decode_block(&mut source, &self.luminance_table);
                            yuv_to_rgb24_block(&cr, &cb, &y2, (8, 0), is_signed, &mut dst);

                            let y3 = self.decode_block(&mut source, &self.luminance_table);
                            yuv_to_rgb24_block(&cr, &cb, &y3, (0, 8), is_signed, &mut dst);

                            let y4 = self.decode_block(&mut source, &self.luminance_table);
                            yuv_to_rgb24_block(&cr, &cb, &y4, (8, 8), is_signed, &mut dst);

                            let words: &[u32] = bytemuck::cast_slice(&dst);
                            self.output_fifo.extend(words);
                        }
                    }
                }

                self.status.set_data_out_fifo_empty(false);

    }

    fn inverse_discrete_cosine(&self, src: &mut [i16; 64]) {
        let dst = &mut [0i16; 64];

        for _ in 0..2 {
            for x in 0..8 {
                for y in 0..8 {
                    let mut sum: i32 = 0;
                    for z in 0..8 {
                        sum += i32::from(src[y + z * 8])
                            * (i32::from(self.scale_table[x + z * 8]) / 8);
                    }
                    dst[x + y * 8] = ((sum + 0xFFF) / 0x2000) as i16;
                }
            }
            std::mem::swap(src, dst);
        }
    }

    pub fn decode_block(&self, source: &mut VecDeque<u16>, qt: &[u8]) -> [i16; 64] {
        let mut block = [0; 64];
        let mut k: usize = 0;

        while source.front() == Some(&0xFE00) {
            source.pop_front();
        }
        if source.is_empty() {
            return [0; 64];
        }

        let first_word = source.pop_front().expect("source is not empty");
        let q_fact = i32::from((first_word >> 10) & 0x3F);
        let dc_coeff = signed10bit(first_word & 0x3FF);
        let dc_val = if q_fact == 0 {
            (dc_coeff * 2).clamp(-0x400, 0x3FF)
        } else {
            (dc_coeff * i16::from(qt[0])).clamp(-0x400, 0x3FF)
        };
        block[ZAG_ZIG[0]] = dc_val;

        while let Some(&n) = source.front() {
            if n == 0xFE00 {
                break;
            }
            source.pop_front();

            k += 1 + ((n >> 10) & 0x3F) as usize;
            if k > 63 {
                break;
            }
            let ac_level = i32::from(signed10bit(n & 0x3FF));
            let val = if q_fact == 0 {
                (ac_level * 2).clamp(-0x400, 0x3FF)
            } else {
                let qt_val = i32::from(qt[k]);
                ((ac_level * qt_val * q_fact + 4) / 8).clamp(-0x400, 0x3FF)
            };
            let target_idx = if q_fact == 0 { k } else { ZAG_ZIG[k] };
            block[target_idx] = val as i16;

            if k == 63 {
                break; // block is full, don't consume the next word
            }
        }

        self.inverse_discrete_cosine(&mut block);
        block
    }


}

bitfield::bitfield! {
    // #[derive(Default)]
    pub struct Status(u32);
    _, set_data_out_fifo_empty: 31;
    _, set_data_in_fifo_full: 30;
    _, set_command_busy: 29;
    _, set_data_in_request: 28;
    _, set_data_out_request: 27;
    data_output_depth, set_data_output_depth: 26,25;
    data_output_signed, set_data_output_signed: 24;
    _, set_data_output_bit15: 23;
    _, set_cmd_data_out: 26,23;
    current_block, set_current_block: 18,16;
    parameter_words_remaining, set_parameter_words_remaining: 15,0;
}

impl Default for Status {
    fn default() -> Self {
        Self(0x80040000)
    }
}

bitfield::bitfield! {
    #[derive(Default)]
    pub struct Control(u32);
    reset, _: 31;
    enable_data_in, _: 30;
    enable_data_out, _: 29;
}


bitfield::bitfield! {
    pub struct CommandWord(u32);
    command, _: 31, 29;
    data_out, _: 28, 25;
    into Depth, output_depth, _: 28, 27;
    output_signed, _: 26;
    output_bit5, _: 25;
    u16, params_len, _: 15, 0;

    into Color, color, _: 0, 0;

}

struct Command {
    command_type: CommandType,
    parameters: Vec<u32>
}

#[derive(Debug, Clone, Copy, FromPrimitive, PartialEq)]
#[repr(u32)]
enum Depth {
    #[default]
    Bit4 = 0,
    Bit8 = 1,
    Bit24 = 2,
    Bit15 = 3,
}

#[derive(Debug, Clone, Copy, FromPrimitive, PartialEq)]
#[repr(u32)]
enum Color {
    #[default]
    Luminance = 0,
    LuminanceAndColor = 1,
}

#[derive(Debug, Clone, Copy)]
enum CommandType {
    SetQuantTable(Color),
    SetScaleTable,
    DecodeMacroblock {
        depth: Depth,
        is_signed: bool,
        b15: bool
    }
}

pub const fn signed10bit(v: u16) -> i16 {
    (v as i16) << 6 >> 6
}

pub const ZAG_ZIG: [usize; 64] = {
    let mut buf = [0; 64];
    let mut i = 0;
    while i < 64 {
        buf[ZIG_ZAG[i]] = i;
        i += 1;
    }
    buf
};

const ZIG_ZAG: [usize; 64] = [
    0, 1, 5, 6, 14, 15, 27, 28, 2, 4, 7, 13, 16, 26, 29, 42, 3, 8, 12, 17, 25, 30, 41, 43, 9, 11,
    18, 24, 31, 40, 44, 53, 10, 19, 23, 32, 39, 45, 52, 54, 20, 22, 33, 38, 46, 51, 55, 60, 21, 34,
    37, 47, 50, 56, 59, 61, 35, 36, 48, 49, 57, 58, 62, 63,
];

pub fn level_shift_8bpp(block: [i16; 64]) -> [u8; 64] {
    let mut out = [0; 64];
    for (i, &y) in block.iter().enumerate() {
        let masked = y & 0x1FF;
        let signed_9bit = if masked > 0xFF {
            masked | !0x1FF
        } else {
            masked
        };

        let clamped = signed_9bit.clamp(-128, 127) as i8;
        out[i] = (clamped as u8) ^ 0x80;
    }
    out
}

pub fn level_shift_4bpp(block: [i16; 64]) -> [u8; 32] {
    let pixels = level_shift_8bpp(block);
    let mut out = [0u8; 32];
    for (i, y) in pixels.chunks_exact(2).enumerate() {
        out[i] = (y[0] >> 4) | (y[1] & 0xF0);
    }
    out
}

pub fn yuv_to_rgb15_block(
    cr: &[i16; 64],
    cb: &[i16; 64],
    y: &[i16; 64],
    pos: (usize, usize),
    is_signed: bool,
    b15: bool,
    dst: &mut [u16; 256],
) {
    let (xx, yy) = pos;
    for py in 0..8 {
        for px in 0..8 {
            let cr_val = i32::from(cr[usize::midpoint(px, xx) + usize::midpoint(py, yy) * 8]);
            let cb_val = i32::from(cb[usize::midpoint(px, xx) + usize::midpoint(py, yy) * 8]);

            let r_off = (1.402 * f64::from(cr_val)) as i32;
            let b_off = (1.772 * f64::from(cb_val)) as i32;
            let g_off = (-0.3437f64).mul_add(f64::from(cb_val), -0.7143 * f64::from(cr_val)) as i32;

            let luma = i32::from(y[px + py * 8]);

            let mut r = (luma + r_off).clamp(-128, 127);
            let mut g = (luma + g_off).clamp(-128, 127);
            let mut b = (luma + b_off).clamp(-128, 127);

            if !is_signed {
                r ^= 0x80;
                g ^= 0x80;
                b ^= 0x80;
            }

            let r5 = u16::from(r as u8 >> 3);
            let g5 = u16::from(g as u8 >> 3);
            let b5 = u16::from(b as u8 >> 3);
            let pixel = r5 | (g5 << 5) | (b5 << 10) | u16::from(b15) << 15;

            dst[(px + xx) + (py + yy) * 16] = pixel;
        }
    }
}

pub fn yuv_to_rgb24_block(
    cr: &[i16; 64],
    cb: &[i16; 64],
    y: &[i16; 64],
    pos: (usize, usize),
    is_signed: bool,
    dst: &mut [u8; 768], // 16 * 16 * 3
) {
    let (xx, yy) = pos;
    for py in 0..8 {
        for px in 0..8 {
            let cr_val = i32::from(cr[usize::midpoint(px, xx) + usize::midpoint(py, yy) * 8]);
            let cb_val = i32::from(cb[usize::midpoint(px, xx) + usize::midpoint(py, yy) * 8]);

            let r_off = (1.402 * f64::from(cr_val)) as i32;
            let b_off = (1.772 * f64::from(cb_val)) as i32;
            let g_off = (-0.3437f64).mul_add(f64::from(cb_val), -0.7143 * f64::from(cr_val)) as i32;

            let luma = i32::from(y[px + py * 8]);

            let mut r = (luma + r_off).clamp(-128, 127);
            let mut g = (luma + g_off).clamp(-128, 127);
            let mut b = (luma + b_off).clamp(-128, 127);

            if !is_signed {
                r ^= 0x80;
                g ^= 0x80;
                b ^= 0x80;
            }

            let base = ((px + xx) + (py + yy) * 16) * 3;
            dst[base] = r as u8;
            dst[base + 1] = g as u8;
            dst[base + 2] = b as u8;
        }
    }
}

