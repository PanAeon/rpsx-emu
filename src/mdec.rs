use std::collections::VecDeque;

use num_enum::FromPrimitive;
use serde::{Deserialize, Serialize};

use crate::system::Addressable;

#[derive(Serialize, Deserialize, PartialEq)]
pub enum CmdState {
    Idle,
    SetQuant,
    SetScale,
    Decode,
}

#[derive(Serialize, Deserialize)]
pub struct Mdec {
    status: Status,
    data_in_enable: bool,
    data_out_enable: bool,
    // control: Control,
    cmd_state: CmdState,
    param_word_idx: u16,
    quant_color: bool,

    #[serde(with = "serde_bytes")]
    iq_y: [u8; 64],
    #[serde(with = "serde_bytes")]
    iq_uv: [u8; 64],
    #[serde(with = "serde_arrays")]
    scale_table: [i16; 64],

    in_buf: Vec<u16>,
    src_pos: usize,

    // #[serde(with = "serde_arrays")]
    // crblk: [i16; 64],
    // #[serde(with = "serde_arrays")]
    // cbblk: [i16; 64],
    // #[serde(with = "serde_arrays")]
    // yblk: [i16; 4 * 64],
    output_fifo: Vec<u32>,
    out_pos: usize,
    read_part: u8, // which packed word within a 4-pixel RGB24 group (0, 1, or 2)
}

impl Mdec {
    pub fn new() -> Self {
        Mdec {
            status: Status::default(),
            // control: Control::default(),
            data_in_enable: false,
            data_out_enable: false,

            cmd_state: CmdState::Idle,
            param_word_idx: 0,
            quant_color: false,

            iq_y: [0; 64],
            iq_uv: [0; 64],
            scale_table: [0; 64],

            in_buf: vec![],
            src_pos: 0,

            // crblk: [0; 64],
            // cbblk: [0; 64],
            // yblk: [0; 4 * 64],
            output_fifo: vec![],
            out_pos: 0,
            read_part: 0,
        }
    }
    pub fn load<T: Addressable>(&mut self, offset: u32) -> T {
        if T::width() as u8 != 4 {
            panic!("Unhandled MDEC load ({})", T::width() as u8);
        }
        match offset {
            0 => T::from_u32(self.dma_read_word()),
            4 => T::from_u32(self.read_status()),
            _ => panic!("unhandled mdec load offset: 0x{:X}", offset),
        }
    }
    pub fn store<T: Addressable>(&mut self, offset: u32, value: T) {
        if T::width() as u8 != 4 {
            panic!("Unhandled MDEC store ({})", T::width() as u8);
        }
        match offset {
            0 => self.write_param(value.as_u32()),
            4 => self.write_control(value.as_u32()),
            _ => panic!("unhandled mdec store offset: 0x{:X}", offset),
        }
    }

    // pub fn dma_can_read(&self) -> bool {
    //     self.control.enable_data_out() && !self.output_fifo.is_empty()
    // }
    // pub fn dma_can_write(&self) -> bool {
    // self.control.enable_data_in() && self.output_fifo.is_empty()
    pub fn dma_can_write(&self) -> bool {
        self.status.dma_in_request() && self.output_fifo.is_empty()
    }

    pub fn dma_can_read(&self) -> bool {
        self.status.dma_out_request() && !self.output_fifo.is_empty()
    }

    pub fn read_status(&mut self) -> u32 {
        let mut status = self.status.clone();
        status.set_data_out_fifo_empty(self.output_fifo.is_empty());
        status.set_data_in_fifo_full(false);
        status.0
    }

    pub fn write_control(&mut self, val: u32) {
        let value = Control(val);
        if value.reset() {
            self.status.0 = 0x8004_0000;
            self.data_in_enable = false;
            self.data_out_enable = false;
            self.cmd_state = CmdState::Idle;
            self.in_buf.clear();
            self.src_pos = 0;
            self.output_fifo.clear();
            self.out_pos = 0;
            self.read_part = 0;
            return;
        }
        self.data_in_enable = value.enable_data_in();
        self.data_out_enable = value.enable_data_out();
        self.update_dma_requests();
    }

    pub fn update_dma_requests(&mut self) {
        self.status
            .set_dma_in_request(self.data_in_enable && self.output_fifo.is_empty());
        self.status
            .set_dma_out_request(self.data_out_enable && !self.output_fifo.is_empty());
    }

    pub fn dma_read_word(&mut self) -> u32 {
        if self.out_pos >= self.output_fifo.len() {
            return 0;
        }

        let data = match self.status.output_depth() {
            Depth::Bit15 => {
                if self.out_pos + 1 >= self.output_fifo.len() {
                    0
                } else {
                    let hw0 =
                        rgb24_to_bgr15(self.output_fifo[self.out_pos], self.status.output_bit15());
                    let hw1 = rgb24_to_bgr15(
                        self.output_fifo[self.out_pos + 1],
                        self.status.output_bit15(),
                    );
                    self.out_pos += 2;
                    hw0 | (hw1 << 16)
                }
            }
            Depth::Bit24 => self.pack_rgb24(),
            Depth::Bit4 | Depth::Bit8 => {
                let word = self.output_fifo[self.out_pos];
                self.out_pos += 1;
                word
            }
        };
        if self.out_pos >= self.output_fifo.len() {
            self.output_fifo.clear();
            self.out_pos = 0;
            self.status.set_command_busy(false);
            self.update_dma_requests();
        }
        data
        // let data = self.output_fifo.pop_front().unwrap_or(0xFE00_FE00);
        // if self.output_fifo.is_empty() {
        //     self.status.set_data_out_fifo_empty(true);
        // }
        // data
    }

    pub fn pack_rgb24(&mut self) -> u32 {
        let base = self.out_pos;
        if base + 3 >= self.output_fifo.len() {
            self.out_pos += 1;
            return 0;
        }

        let px = [
            self.output_fifo[base],
            self.output_fifo[base + 1],
            self.output_fifo[base + 2],
            self.output_fifo[base + 3],
        ];

        let mut rgb = [0_u8; 12];
        for i in 0..4 {
            rgb[i * 3 + 0] = px[i] as u8;
            rgb[i * 3 + 1] = (px[i] >> 8) as u8;
            rgb[i * 3 + 2] = (px[i] >> 16) as u8;
        }
        let part = self.read_part;
        let off = part as usize * 4;
        let data = (rgb[off] as u32)
            | (rgb[off + 1] as u32) << 8
            | (rgb[off + 2] as u32) << 16
            | (rgb[off + 3] as u32) << 24;
        if part == 2 {
            self.read_part = 0;
            self.out_pos += 4;
        } else {
            self.read_part += 1;
        }
        data
    }

    pub fn write_param(&mut self, v: u32) {
        if self.cmd_state == CmdState::Idle {
            self.write_command(v);
            return;
        }
        let was_decode = self.cmd_state == CmdState::Decode;
        let is_last = self.status.params_remaining() == 0;

        match self.cmd_state {
            CmdState::SetQuant => self.receive_quant_param(v),
            CmdState::SetScale => self.receive_scale_param(v),
            CmdState::Decode => {
                self.receive_decode_param(v);
                if is_last {
                    self.decode_macroblocks();
                }
            }
            CmdState::Idle => unreachable!(),
        }

        self.status
            .set_params_remaining(self.status.params_remaining().wrapping_sub(1));
        self.param_word_idx = self.param_word_idx.wrapping_add(1);

        if is_last {
            self.cmd_state = CmdState::Idle;
            self.param_word_idx = 0;
            self.status
                .set_command_busy(was_decode && self.output_fifo.len() > 0);
        }
    }

    pub fn write_command(&mut self, data: u32) {
        let cmd = CommandWord(data);
        self.status.set_output_bit15(cmd.output_bit15());
        self.status.set_output_signed(cmd.output_signed());
        self.status.set_output_depth(cmd.output_depth());

        match cmd.command() {
            1 => self.decode_macroblock(cmd),
            2 => self.set_quant_table(cmd),
            3 => self.set_scale_table(),
            _ => self.status.set_params_remaining(cmd.params_len()),
        }
        self.update_dma_requests();
    }

    pub fn decode_macroblock(&mut self, cmd: CommandWord) {
        self.status
            .set_params_remaining(cmd.params_len().wrapping_sub(1));
        self.status.set_command_busy(true);
        self.status.set_current_block(4); // Cr
        self.cmd_state = CmdState::Decode;
        self.param_word_idx = 0;
        self.in_buf.clear();
        self.src_pos = 0;
        self.output_fifo.clear();
        self.out_pos = 0;
        self.read_part = 0;
    }

    pub fn set_quant_table(&mut self, cmd: CommandWord) {
        self.quant_color = cmd.params_len() & 1 != 0;
        self.status
            .set_params_remaining(if self.quant_color { 31 } else { 15 });
        self.status.set_command_busy(true);
        self.cmd_state = CmdState::SetQuant;
        self.param_word_idx = 0;
    }

    pub fn set_scale_table(&mut self) {
        self.status.set_params_remaining(31);
        self.status.set_command_busy(true);
        self.cmd_state = CmdState::SetScale;
        self.param_word_idx = 0;
    }

    pub fn receive_quant_param(&mut self, v: u32) {
        let byte_idx = self.param_word_idx as usize * 4;
        let bytes = v.to_le_bytes();
        if byte_idx < 64 {
            for (i, b) in bytes.iter().enumerate() {
                if byte_idx + i < 64 {
                    self.iq_y[byte_idx + i] = *b;
                }
            }
        } else {
            let uv_idx = byte_idx - 64;
            for (i, b) in bytes.iter().enumerate() {
                if uv_idx + i < 64 {
                    self.iq_uv[uv_idx + i] = *b;
                }
            }
        }
    }

    pub fn receive_decode_param(&mut self, v: u32) {
        self.in_buf.push(v as u16);
        self.in_buf.push((v >> 16) as u16);
    }

    pub fn receive_scale_param(&mut self, v: u32) {
        let hw_idx = self.param_word_idx as usize * 2;
        if hw_idx + 1 < 64 {
            self.scale_table[hw_idx] = v as i16;
            self.scale_table[hw_idx + 1] = (v >> 16) as i16;
        }
    }

    fn decode_macroblocks(&mut self) {
        self.src_pos = 0;
        self.output_fifo.clear();

        let is_color = self.status.output_depth() == Depth::Bit15
            || self.status.output_depth() == Depth::Bit24;

        while self.src_pos < self.in_buf.len() {
            if is_color {

                let mut pixels = [0u32; 256];
                let Some(cr) = self.rl_decode_block::<IQ_UV>() else {
                    break;
                };
                let Some(cb) =  self.rl_decode_block::<IQ_UV>() else {
                    break;
                };
                let Some(y1) =  self.rl_decode_block::< IQ_Y>() else {
                    break;
                };
                let Some(y2) =  self.rl_decode_block::< IQ_Y>() else {
                    break;
                };
                let Some(y3) = self.rl_decode_block::< IQ_Y>() else {
                    break;
                };
                let Some(y4) =  self.rl_decode_block::< IQ_Y>() else {
                    break;
                }; 
                yuv_to_rgb15(&cr, &cb, &y1, (0, 0), self.status.output_signed(), self.status.output_bit15(), &mut pixels);
                yuv_to_rgb15(&cr, &cb, &y2, (8, 0), self.status.output_signed(), self.status.output_bit15(), &mut pixels);
                yuv_to_rgb15(&cr, &cb, &y3, (0, 8), self.status.output_signed(), self.status.output_bit15(), &mut pixels);
                yuv_to_rgb15(&cr, &cb, &y4, (8, 8), self.status.output_signed(), self.status.output_bit15(), &mut pixels);
                // self.yuv_to_rgb(&mut pixels, 0, 0);
                // self.yuv_to_rgb(&mut pixels, 8, 0);
                // self.yuv_to_rgb(&mut pixels, 0, 8);
                // self.yuv_to_rgb(&mut pixels, 8, 8);
                self.output_fifo.extend(pixels);


                        // let y1 = self.decode_block(&mut source, &self.luminance_table);
                        // yuv_to_rgb15_block(&cr, &cb, &y1, (0, 0), is_signed, b15, &mut dst);
                        //
                        // let y2 = self.decode_block(&mut source, &self.luminance_table);
                        // yuv_to_rgb15_block(&cr, &cb, &y2, (8, 0), is_signed, b15, &mut dst);
                        //
                        // let y3 = self.decode_block(&mut source, &self.luminance_table);
                        // yuv_to_rgb15_block(&cr, &cb, &y3, (0, 8), is_signed, b15, &mut dst);
                        //
                        // let y4 = self.decode_block(&mut source, &self.luminance_table);
                        // yuv_to_rgb15_block(&cr, &cb, &y4, (8, 8), is_signed, b15, &mut dst);
                        //
                        // let words: &[u32] = bytemuck::cast_slice(&dst);
                        // self.output_fifo.extend(words);
            } else {
                let Some(block) = self.rl_decode_block::<IQ_Y>() else {
                    break;
                };
                let mut mono = [0_u8; 64];
                self.y_to_mono(&block, &mut mono);
                let words: &[u32] = bytemuck::cast_slice(&mono);
                self.output_fifo.extend(words);
            }
        }

        self.out_pos = 0;
        self.read_part = 0;
        self.update_dma_requests();
    }

    pub fn y_to_mono(&self, block: &[i16;64], out: &mut [u8; 64]) {
        for i in 0..64 {
            let raw = block[i];
            // Clip to signed 9-bit range, then saturate to signed 8-bit
            let clipped = ((raw << (16 - 9)) >> (16 - 9)) as i32;
            let mut v = clipped.clamp(-128, 127);
            if !self.status.output_signed() {
                v ^= 0x80;
            }
            out[i] = (v & 0xff) as u8;
        }
    }

    // Decode one 8x8 block using run-length encoding (spec: rl_decode_block).
    pub fn rl_decode_block<const QT: usize>(&mut self) -> Option<[i16;64]> {
        // Skip fe00h padding
        let mut hw = self.next_halfword();
        while hw == 0xfe00 {
            if self.src_pos >= self.in_buf.len() {
                return None;
            }
            hw = self.next_halfword();
        }

        // let blk: &mut [i16] = match WHICH {
        //     0 => &mut self.yblk[0..64],
        //     1 => &mut self.yblk[64..128],
        //     2 => &mut self.yblk[128..192],
        //     3 => &mut self.yblk[192..],
        //     4 => &mut self.crblk,
        //     5 => &mut self.cbblk,
        //     _ => unreachable!(),
        // };
        // let blk = 


        let mut blk = [0;64];

        let qt: &[u8] = match QT {
            IQ_Y => &self.iq_y,
            IQ_UV => &self.iq_uv,
            _ => unreachable!(),
        };

        let q_scale = ((hw >> 10) & 0x3f) as u32;
        let mut current = signed10(hw);
        let mut val = current * qt[0] as i32;

        let mut k: u32 = 0;
        while k < 64 {
            if q_scale == 0 {
                val = current * 2;
            }
            val = val.clamp(-0x400, 0x3ff);

            if q_scale > 0 {
                blk[ZAG_ZIG[k as usize]] = val as i16;
            } else {
                blk[k as usize] = val as i16;
            }

            let rle = self.next_halfword();
            current = signed10(rle);
            k += (((rle >> 10) & 0x3f) + 1) as u32;
            if k >= 64 {
                break;
            }
            let qt: &[u8] = match QT {
                IQ_Y => &self.iq_y,
                IQ_UV => &self.iq_uv,
                _ => unreachable!(),
            };
            val = (current * qt[k as usize] as i32 * q_scale as i32 + 4) / 8;
        }


        idct(&self.scale_table, &mut blk);
        return Some(blk)
    }

    pub fn next_halfword(&mut self) -> u16 {
        if self.src_pos >= self.in_buf.len() {
            return 0xfe00;
        };
        let hw = self.in_buf[self.src_pos];
        self.src_pos += 1;
        hw
    }

    /* fn decode_macoblock(
            &mut self,
            depth: Depth,
            is_signed: bool,
            b15: bool,
            parameters: &Vec<u32>,
        ) {
            let raw: &[u16] = bytemuck::cast_slice(parameters.as_slice());
            let mut source: VecDeque<u16> = raw.iter().copied().collect();

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
    */
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
    #[derive(Serialize, Deserialize, Clone)]
    pub struct Status(u32);
    _, set_data_out_fifo_empty: 31;
    _, set_data_in_fifo_full: 30;
    _, set_command_busy: 29;
    dma_in_request, set_dma_in_request: 28;
    dma_out_request, set_dma_out_request: 27;
    from into Depth, output_depth, set_output_depth: 26,25;
    output_signed, set_output_signed: 24;
    output_bit15, set_output_bit15: 23;
    _, set_cmd_data_out: 26,23;
    current_block, set_current_block: 18,16;
    u16, params_remaining, set_params_remaining: 15,0; // words remaining minus 1
}

impl Default for Status {
    fn default() -> Self {
        Self(0x80040000)
    }
}

bitfield::bitfield! {
    #[derive(Default, Serialize, Deserialize)]
    pub struct Control(u32);
    reset, _: 31;
    enable_data_in, _: 30;
    enable_data_out, _: 29;
}

bitfield::bitfield! {
    #[derive(Serialize, Deserialize)]
    pub struct CommandWord(u32);
    command, _: 31, 29;
    // data_out, _: 28, 25;
    into Depth, output_depth, _: 28, 27;
    output_signed, _: 26;
    output_bit15, _: 25;
    u16, params_len, _: 15, 0;

    into Color, color, _: 0, 0;

}

#[derive(Serialize, Deserialize)]
struct Command {
    command_type: CommandType,
    parameters: Vec<u32>,
}

#[derive(Debug, Clone, Copy, FromPrimitive, PartialEq, Serialize, Deserialize)]
#[repr(u32)]
enum Depth {
    #[default]
    Bit4 = 0,
    Bit8 = 1,
    Bit24 = 2,
    Bit15 = 3,
}

impl From<Depth> for u32 {
    fn from(value: Depth) -> Self {
        match value {
            Depth::Bit4 => 0,
            Depth::Bit8 => 1,
            Depth::Bit24 => 2,
            Depth::Bit15 => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, FromPrimitive, PartialEq, Serialize, Deserialize)]
#[repr(u32)]
enum Color {
    #[default]
    Luminance = 0,
    LuminanceAndColor = 1,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum CommandType {
    SetQuantTable(Color),
    SetScaleTable,
    DecodeMacroblock {
        depth: Depth,
        is_signed: bool,
        b15: bool,
    },
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

pub fn yuv_to_rgb15(
    cr: &[i16; 64],
    cb: &[i16; 64],
    yb: &[i16; 64],
    pos: (usize, usize),
    is_signed: bool,
    b15: bool,
    dst: &mut [u32; 256],
) {
    let bx = pos.0;
    let by = pos.1;
    for y in 0..8 {
        for x in 0..8 {
            let luma = yb[y * 8 + x] as i32;
            let cb = cb[((y + by) / 2) * 8 + ((x + bx) / 2)] as i32;
                let cr: i32 = cr[((y + by) / 2) * 8 + ((x + bx) / 2)] as i32;

                let cr_f: f64 = cr as f64;
                let cb_f: f64 = cb as f64;

                // Spec: R=(1.402*Cr), G=(-0.3437*Cb)+(-0.7143*Cr), B=(1.772*Cb)
                let mut r: i32 = (1.402 * cr_f) as i32;
                let mut g: i32 = (-0.3437 * cb_f - 0.7143 * cr_f) as i32;
                let mut b: i32 = (1.772 * cb_f) as i32;

                // Clamp to signed 8-bit after adding luma
                r = (luma + r).clamp( -128, 127);
                g = (luma + g).clamp( -128, 127);
                b = (luma + b).clamp( -128, 127);

                // If unsigned output, add 128 (xor 0x80)
                if !is_signed {
                    r ^= 0x80;
                    g ^= 0x80;
                    b ^= 0x80;
                }

                let ru: u32 = (r & 0xff) as u32;
                let gu: u32 = (g & 0xff) as u32;
                let bu: u32 = (b & 0xff) as u32;
                dst[(by + y) * 16 + (bx + x)] = (bu << 16) | (gu << 8) | ru;
        }
    }
}

pub fn yuv_to_rgb15_block(
    cr: &[i16; 64],
    cb: &[i16; 64],
    y: &[i16; 64],
    pos: (usize, usize),
    is_signed: bool,
    b15: bool,
    dst: &mut [u32; 256],
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

            let r5 = u32::from(r as u8 & 0xff);
            let g5 = u32::from(g as u8 & 0xff);
            let b5 = u32::from(b as u8 & 0xff);
            let pixel = r5 | (g5 << 8) | (b5 << 16) | u32::from(b15) << 15;

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

pub fn rgb24_to_bgr15(rgb: u32, bit15: bool) -> u32 {
    let r = (rgb >> 3) & 0x1f;
    let g = (rgb >> 11) & 0x1f;
    let b = (rgb >> 19) & 0x1f;
    ((bit15 as u32) << 15) | (b << 10) | (g << 5) | r
}

const IQ_Y: usize = 0;
const IQ_UV: usize = 1;

pub fn signed10(v: u16) -> i32 {
    (((v as i16)  << 6 ) >> 6) as i32
}

pub fn idct(scale_table: &[i16], blk: &mut [i16]) {
    let mut tmp = [0_i64;64];

    for x in 0..8 {
        for y in 0..8 {
            let mut sum: i64 = 0;
            for i in 0..8 {
                sum += scale_table[i * 8 + y] as i64 * blk[x + i * 8] as i64;
            }
            tmp[x + y * 8] = sum;
        }
    }

    for x in 0..8 {
        for y in 0..8 {
            let mut sum: i64 = 0;
            for i in 0..8 {
                sum += tmp[i + y * 8] * scale_table[x + i * 8] as i64;
            }
            let round = (sum >> 31) & 1;
            blk[x + y * 8] = ((sum >> 32) + round) as i16;
        }
    }
}
