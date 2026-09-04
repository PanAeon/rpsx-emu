use std::{cell::Cell, collections::VecDeque, ops::{Index, IndexMut, Range}};

use num_enum::FromPrimitive;

use crate::{cdrom::CDRom, memory_bus::{Addressable, MemoryBus}};

bitfield::bitfield! {
    #[derive(Default)]
    struct SpuControl(u16);
    enabled, _: 15; // doesn't affect CD Audio
    unmuted, _: 14; // doesnt' affect CD Audio
    u8, noise_frequency_shift, _: 13,10;
    u8, noise_frequency_step, _:9,8;
    reverb_master_enabled, _: 7;
    irq9_enabled, _: 6;
    sound_ram_transfer_mode, _: 5,4;
    external_audio_reverb, _: 3;
    cd_audio_reverb, _: 2;
    external_audio_enabled, _: 1;
    cd_audio_enabled, _: 0;
}

bitfield::bitfield! {
    #[derive(Default)]
    struct SpuStatus(u16);
    unused, _: 15,12;
    write_to_second_half_of_capture_buffers, set_write_to_second_half: 11;
    data_transfer_busy_flag, set_data_transfer_busy_flag: 10;
    dma_read_request, set_dma_read_request: 9;
    dma_write_request, set_dma_write_request: 8;
    read_write_request, set_read_write_request: 7; // same as SPUCNT.Bit5
    irq9_flag, set_irq9_flag: 6;
    u8, current_spu_mode, set_current_spu_mode: 5,0;   // same as SPUCNT Bit5-0

}

#[derive(Default)]
pub struct NoiseGenerator {
    lfsr: u16,
    step: u8,
    shift: u8,
    timer: i32,
}

impl NoiseGenerator {

    pub fn clock(&mut self) {
        // Configured 2-bit step value of N represents a decrement of (N + 4)
        self.timer -= i32::from(self.step + 4);
        if self.timer >= 0 {
            return;
        }

        self.clock_lfsr();

        // Reset timer
        while self.timer <= 0 {
            self.timer += 0x20000 >> self.shift;
        }
    }

    // Called on SPUCNT writes
    pub fn update_frequency(&mut self, step: u8, shift: u8) {
        if shift != self.shift {
            self.timer = 0x20000 >> shift;
        }
        self.step = step;
        self.shift = shift;
    }

    pub fn clock_lfsr(&mut self) {
        // XOR bits 10, 11, 12, and 15, and then XOR with 1 to invert the result
        let parity = ((self.lfsr >> 15) & 1)
            ^ ((self.lfsr >> 12) & 1)
            ^ ((self.lfsr >> 11) & 1)
            ^ ((self.lfsr >> 10) & 1)
            ^ 1;
        self.lfsr = (self.lfsr << 1) | parity;
    }
}

// R/W
// All volume registers are signed 16bit (range -8000h..+7FFFh).
// All src/dst/disp/base registers are addresses in SPU memory (divided by 8),
// src/dst are relative to the current buffer address,
// the disp registers are relative to src registers,
// the base register defines the start address of the reverb buffer
// (the end address is fixed, at 7FFFEh).
// Writing a value to mBASE does additionally set the current buffer address to that value.
#[derive(Default)]
pub struct Reverb {
    // Reverb Work Area Start Address in Sound RAM
    output_volume_left: i16,
    output_volume_right: i16,
    m_base: usize,
    d_apf1: usize,
    d_apf2: usize,
    v_iir: i32,
    v_comb1: i32,
    v_comb2: i32,
    v_comb3: i32,
    v_comb4: i32,
    v_wall: i32,
    v_apf1: i32,
    v_apf2: i32,
    ml_same: usize,
    mr_same: usize,
    ml_comb1: usize,
    mr_comb1: usize,
    ml_comb2: usize,
    mr_comb2: usize,
    dl_same: usize,
    dr_same: usize,
    ml_diff: usize,
    mr_diff: usize,
    ml_comb3: usize,
    mr_comb3: usize,
    ml_comb4: usize,
    mr_comb4: usize,
    dl_diff: usize,
    dr_diff: usize,
    ml_apf1: usize,
    mr_apf1: usize,
    ml_apf2: usize,
    mr_apf2: usize,
    input_volume_left: i16,
    input_volume_right: i16,
    half_tick: bool,
    // left_queue: VecDeque<i32>,
    // right_queue: VecDeque<i32>,
    current_buffer_address: usize,
    pub l_out: i32,
    pub r_out: i32,
}

impl Reverb {
    pub fn set_base_address(&mut self, address: u16) {
        self.m_base = (address as usize) * 8;
        self.current_buffer_address = self.m_base;
    }
    // pub fn push_input_sample(deque: &mut VecDeque<i32>, sample: i32) {
    //     if deque.len() == FIR_FILTER.len() {
    //         deque.pop_front();
    //     }
    //     deque.push_back(sample);
    // }
    // pub fn apply_fir_filter(deque: &VecDeque<i32>) -> i32 {
    //     FIR_FILTER.iter().zip(deque)
    //         .map(|(&a, &b)| (a * b) >> 15)
    //         .sum()
    // }

    pub fn read_sample(&self, ram: &SoundRam, pos: usize) -> i32 {
        let base = self.m_base;
        let offs = (pos + self.current_buffer_address - base) % (0x80000 - base);
        let addr = (base + offs) & 0x7FFFE;

        let bytes = [ram[addr], ram[addr + 1]];
        i32::from(i16::from_le_bytes(bytes))
    }
    
    pub fn write_sample(&self, ram: &mut SoundRam, pos: usize, val: i32) {
        let base = self.m_base;
        let offs = (pos + self.current_buffer_address - base) % (0x80000 - base);
        let addr = (base + offs) & 0x7FFFE;

        let bytes = clamped_i16(val).to_le_bytes();
        ram[addr] = bytes[0];
        ram[addr + 1] = bytes[1];
    }

    pub fn tick(&mut self, mixed: [i32;2], ram: &mut SoundRam, write_to_ram: bool) {
        self.half_tick = !self.half_tick;
        if self.half_tick {
            return;
        }
          // ___Input from Mixer (Input volume multiplied with incoming data)_____________
          // Lin = vLIN * LeftInput    ;from any channels that have Reverb enabled
          // Rin = vRIN * RightInput   ;from any channels that have Reverb enabled
        let l_in = mixed[0].saturating_mul(i32::from(self.input_volume_left)) >> 15;
        let r_in = mixed[1].saturating_mul(i32::from(self.input_volume_right)) >> 15;
          // ____Same Side Reflection (left-to-left and right-to-right)___________________
          // [mLSAME] = (Lin + [dLSAME]*vWALL - [mLSAME-2])*vIIR + [mLSAME-2]  ;L-to-L
          // [mRSAME] = (Rin + [dRSAME]*vWALL - [mRSAME-2])*vIIR + [mRSAME-2]  ;R-to-R
        let l2l = 
            mul_16(l_in + mul_16(self.read_sample(ram, self.dl_same), self.v_wall)
             - self.read_sample(ram, self.ml_same.saturating_sub(2)),
            self.v_iir) + self.read_sample(ram, self.ml_same.saturating_sub(2));

        let r2r = 
            mul_16(r_in + mul_16(self.read_sample(ram, self.dr_same), self.v_wall)
             - self.read_sample(ram, self.mr_same.saturating_sub(2)),
            self.v_iir) + self.read_sample(ram, self.mr_same.saturating_sub(2));

        if write_to_ram {
            self.write_sample(ram, self.ml_same, l2l);
            self.write_sample(ram, self.mr_same, r2r);
        }
          // ___Different Side Reflection (left-to-right and right-to-left)_______________
          // [mLDIFF] = (Lin + [dRDIFF]*vWALL - [mLDIFF-2])*vIIR + [mLDIFF-2]  ;R-to-L
          // [mRDIFF] = (Rin + [dLDIFF]*vWALL - [mRDIFF-2])*vIIR + [mRDIFF-2]  ;L-to-R

        let r2l = mul_16(
            l_in + mul_16(self.read_sample(ram, self.dr_diff), self.v_wall)
                - self.read_sample(ram, self.ml_diff.saturating_sub(2)),
            self.v_iir
        ) + self.read_sample(ram, self.ml_diff.saturating_sub(2));

        let l2r = mul_16(
            l_in + mul_16(self.read_sample(ram, self.dl_diff), self.v_wall)
                - self.read_sample(ram, self.mr_diff.saturating_sub(2)),
            self.v_iir
        ) + self.read_sample(ram, self.mr_diff.saturating_sub(2));

        if write_to_ram {
            self.write_sample(ram, self.ml_diff, r2l);
            self.write_sample(ram, self.mr_diff, l2r);
        }
          // ___Early Echo (Comb Filter, with input from buffer)__________________________
          // Lout=vCOMB1*[mLCOMB1]+vCOMB2*[mLCOMB2]+vCOMB3*[mLCOMB3]+vCOMB4*[mLCOMB4]
          // Rout=vCOMB1*[mRCOMB1]+vCOMB2*[mRCOMB2]+vCOMB3*[mRCOMB3]+vCOMB4*[mRCOMB4]
        let mut l_out = mul_16(self.v_comb1, self.read_sample(ram, self.ml_comb1))
            + mul_16(self.v_comb2, self.read_sample(ram, self.ml_comb2))
            + mul_16(self.v_comb3, self.read_sample(ram, self.ml_comb3))
            + mul_16(self.v_comb4, self.read_sample(ram, self.ml_comb4));

        let mut r_out = mul_16(self.v_comb1, self.read_sample(ram, self.mr_comb1))
            + mul_16(self.v_comb2, self.read_sample(ram, self.mr_comb2))
            + mul_16(self.v_comb3, self.read_sample(ram, self.mr_comb3))
            + mul_16(self.v_comb4, self.read_sample(ram, self.mr_comb4));
          // ___Late Reverb APF1 (All Pass Filter 1, with input from COMB)________________
          // Lout=Lout-vAPF1*[mLAPF1-dAPF1], [mLAPF1]=Lout, Lout=Lout*vAPF1+[mLAPF1-dAPF1]
          // Rout=Rout-vAPF1*[mRAPF1-dAPF1], [mRAPF1]=Rout, Rout=Rout*vAPF1+[mRAPF1-dAPF1]

        l_out -= mul_16(
            self.v_apf1,
            self.read_sample(ram, self.ml_apf1 - self.d_apf1));

        r_out -= mul_16(
            self.v_apf1,
            self.read_sample(ram, self.mr_apf1 - self.d_apf1));

        if write_to_ram {
            self.write_sample(ram, self.ml_apf1, l_out);
            self.write_sample(ram, self.mr_apf1, r_out);
        }

        l_out = mul_16(l_out, self.v_apf1) + self.read_sample(ram, self.ml_apf1 - self.d_apf1);
        r_out = mul_16(r_out, self.v_apf1) + self.read_sample(ram, self.mr_apf1 - self.d_apf1);
          // ___Late Reverb APF2 (All Pass Filter 2, with input from APF1)________________
          // Lout=Lout-vAPF2*[mLAPF2-dAPF2], [mLAPF2]=Lout, Lout=Lout*vAPF2+[mLAPF2-dAPF2]
          // Rout=Rout-vAPF2*[mRAPF2-dAPF2], [mRAPF2]=Rout, Rout=Rout*vAPF2+[mRAPF2-dAPF2]

        l_out -= mul_16(
            self.v_apf2,
            self.read_sample(ram, self.ml_apf2 - self.d_apf2));

        r_out -= mul_16(
            self.v_apf2,
            self.read_sample(ram, self.mr_apf2 - self.d_apf2));

        if write_to_ram {
            self.write_sample(ram, self.ml_apf2, l_out);
            self.write_sample(ram, self.mr_apf2, r_out);
        }

        l_out = mul_16(l_out, self.v_apf2) + self.read_sample(ram, self.ml_apf2 - self.d_apf2);
        r_out = mul_16(r_out, self.v_apf2) + self.read_sample(ram, self.mr_apf2 - self.d_apf2);

          // ___Output to Mixer (Output volume multiplied with input from APF2)___________
          // LeftOutput  = Lout*vLOUT
          // RightOutput = Rout*vROUT

        self.l_out = mul_16(l_out, i32::from(self.output_volume_left));
        self.r_out = mul_16(r_out, i32::from(self.output_volume_right));
          // ___Finally, before repeating the above steps_________________________________
          // BufferAddress = MAX(mBASE, (BufferAddress+2) AND 7FFFEh)
        self.current_buffer_address = ((self.current_buffer_address + 2) & 0x7FFFE).max(self.m_base);
          // Wait one 22050Hz cycle, then repeat the above stuff
    }
}

#[derive(Default, Copy, Clone)]
pub struct Voice {
    volume_left: i16,
    volume_right: i16,
    //Sample rate (0=stop, 1000h=44100Hz, 4000h=fastest, 4001h..FFFFh=usually same as 4000h)
    adpcm_sample_rate: u16,
    adpcm_start_address: u16,
    envelope: AdsrEnvelope,
    current_address: usize,
    repeat_address: usize,
    pitch_counter: u16,
    decode_buffer: [i16; 28],
    key_on: bool,
    // key_off: bool,
    current_buffer_idx: usize,
    current_sample: i16,
    old_sample: i16,
    older_sample: i16,
    sample_history: [i16; 4],
    noise_mode: bool,
    reverb_mode: bool,
    modulation_enabled: bool,
    ignore_loop_address: bool,
    pub reached_loop_end: bool,

}

impl Voice {
    pub fn set_start_address(&mut self, address: u16) {
        self.adpcm_start_address = address;
    }
    pub fn set_repeat_address(&mut self, address: u16) {
        self.repeat_address = (address as usize ) * 8;
        self.ignore_loop_address = true;
    }

    pub fn key_on(&mut self, sound_ram: &SoundRam) {
        self.key_on = true;
        self.current_address = (self.adpcm_start_address as usize) << 3;
        self.pitch_counter = 0;
        self.current_buffer_idx = 0;
        self.sample_history.fill(0);
        self.old_sample = 0;
        self.older_sample = 0;
        self.ignore_loop_address = false;
        self.reached_loop_end = false;
        self.decode_next_block(sound_ram);
        self.envelope.key_on();
    }

    pub fn key_off(&mut self) {
        self.envelope.key_off();
        self.key_on = false;
    }

    pub fn decode_next_block(&mut self, sound_ram: &SoundRam) {
        // grab the next 16-byte block
        let block = &sound_ram[self.current_address..self.current_address + 16];

        // Decode the 28 samples
        decode_adpcm_block(
            block,
            &mut self.decode_buffer,
            &mut self.old_sample,
            &mut self.older_sample,
        );

        // Parse loop flags from the second header byte
        let loop_end = block[1] & 1 != 0;
        let loop_repeat = block[1] & 2 != 0;
        let loop_start = block[1] & 4 != 0;

        if loop_start && !self.ignore_loop_address {
            self.repeat_address = self.current_address;
        }
        if loop_end {
            self.reached_loop_end = true;
            self.current_address = self.repeat_address;

            if !loop_repeat {
                self.envelope.level = 0;
                self.envelope.key_off();
            }
        } else {
            self.current_address += 16;
        }
    }

    pub fn clock(&mut self, sound_ram: &SoundRam, previous_voice_output: i16) {
        let mut pitch_counter_step = self.adpcm_sample_rate;

        if self.modulation_enabled {
            // Convert previous voice output from i16 range to u16 range
            let multiplier = i32::from(previous_voice_output) + 0x8000;
            // Apply previous voice output as a multiplier to step, N/0x8000
            let adjusted_step = (i32::from(pitch_counter_step) * multiplier) >> 15;
            pitch_counter_step = adjusted_step as u16;
        }


        // Effective sample rate cannot be larger than 0x4000 (176400 Hz)
        pitch_counter_step = std::cmp::min(0x4000, pitch_counter_step);

        // In a full implementation, pitch modulation would be applied right here
        self.pitch_counter += pitch_counter_step;

        // Step through samples while pitch counter bits 12-15 are non-zero
        while self.pitch_counter >= 0x1000 {
            self.pitch_counter -= 0x1000;
            self.current_buffer_idx += 1;

            // check if end of block was reached
            if self.current_buffer_idx == 28 {
                self.current_buffer_idx = 0;
                self.decode_next_block(sound_ram);
            }
            // .samples_history[0] = if voice.noise_enabled {
            //             noise_sample
            self.sample_history[0] = self.decode_buffer[self.current_buffer_idx];
            self.sample_history.rotate_left(1);
        }

        let interpolation_idx = ((self.pitch_counter >> 4) & 0xFF) as usize;
        let samples = self.sample_history.map(i32::from);

        // Perform interpolation; do math using signed 32-bit integers to avoid overflow in intermediate calculations
        // The right shifts by 15 are because each multiplier N represents (N / 0x8000)
        let mut interpolated = (GAUSSIAN_TABLE[0x0FF - interpolation_idx] * samples[0]) >> 15;
        interpolated += (GAUSSIAN_TABLE[0x1FF - interpolation_idx] * samples[1]) >> 15;
        interpolated += (GAUSSIAN_TABLE[0x100 + interpolation_idx] * samples[2]) >> 15;
        interpolated += (GAUSSIAN_TABLE[interpolation_idx] * samples[3]) >> 15;

        // update current sample
        self.current_sample = interpolated as i16; //
        // self.current_sample = self.decode_buffer[self.current_buffer_idx as usize];
        self.envelope.clock();
    }
}

// TODO: implement sweep volume
pub struct Spu {
    main_volume_left: i16,
    main_volume_right: i16,

    voice_key_on: u32,
    voice_key_off: u32,

    control: SpuControl,
    cd_audio_input_volume_left: i16,
    cd_audio_input_volume_right: i16,
    external_audio_input_volume_left: i16,  // (-8000h..+7FFFh)
    external_audio_input_volume_right: i16, // (-8000h..+7FFFh)

    data_transfer_type: DataTransferType,
    data_transfer_address: u16, // address divided by 8
    current_address: usize,     // current address for dma transfer
    // 512kb
    memory: SoundRam,
    voices: [Voice; 24],
    reverb: Reverb,
    noise: NoiseGenerator,
    last_irq_line: bool,
    capture_buffer_idx: usize,
}

impl Spu {
    pub fn new() -> Self {
        let voices = [Default::default(); 24];
        Spu {
            main_volume_left: 0,
            main_volume_right: 0,

            voice_key_on: 0,
            voice_key_off: 0,
            control: Default::default(),
            cd_audio_input_volume_left: 0,
            cd_audio_input_volume_right: 0,
            external_audio_input_volume_left: 0,
            external_audio_input_volume_right: 0,
            data_transfer_type: DataTransferType::Normal,
            data_transfer_address: 0,
            current_address: 0,
            voices,
            reverb: Default::default(),
            memory: SoundRam { ram: vec![0; 512 * 1024].into_boxed_slice(), irq_enabled: false, irq_address: 0, irq: Cell::default() },
            noise: NoiseGenerator::default(),
            last_irq_line: false,
            capture_buffer_idx: 0,
        }
    }

    pub fn clock(memory_bus: &mut MemoryBus) {
        let spu = &mut memory_bus.spu;
        let mut prev_output: i16 =  0;
        for voice in &mut spu.voices {
            voice.clock(&spu.memory, prev_output);
            prev_output = voice.current_sample;
        }
        spu.noise.clock();

        let irq_line = spu.memory.irq.get();
        if !spu.last_irq_line && irq_line && spu.control.irq9_enabled() && spu.control.enabled() {
            memory_bus.irqctl.status.set_spu(true);
        }
        spu.last_irq_line = irq_line;
    }

    pub fn mix(&mut self, cdrom: &mut CDRom) -> [i16; 2] {
        let mut mixed_sample_l: i32 = 0;
        let mut mixed_sample_r: i32 = 0;
        let mut mixed_reverb = [0i32;2];

        let (cd_l, cd_r) = if self.control.cd_audio_enabled() {
            (cdrom.get_audio_sample(), cdrom.get_audio_sample())
        } else 
        { (0_i16, 0_i16) };

        self.write_capture_buffer(cd_l, 0x000);
        self.write_capture_buffer(cd_r, 0x400);

        let cd_l = apply_volume(cd_l, self.cd_audio_input_volume_left);
        let cd_r = apply_volume(cd_r, self.cd_audio_input_volume_right);

        if !self.control.enabled() {
            return [cd_l, cd_r];
        }

        let noise_sample = self.noise.lfsr.cast_signed();

        for i in 0..24 {
            // Apply ADSR envelope first
            let voice = &mut self.voices[i];
            let envelope_sample = if voice.noise_mode {
                apply_volume(noise_sample, voice.envelope.level as i16)
            } else {
                apply_volume(voice.current_sample, voice.envelope.level as i16)
            };

            // Apply L/R volumes second
            let output_l = apply_volume(envelope_sample, voice.volume_left);
            let output_r = apply_volume(envelope_sample, voice.volume_right);

            if voice.reverb_mode {
                mixed_reverb[0] += i32::from(apply_volume(envelope_sample, voice.volume_left));
                mixed_reverb[1] += i32::from(apply_volume(envelope_sample, voice.volume_right));
            }


            if i == 1 {
                self.write_capture_buffer(envelope_sample, 0x800);
            }
            if i == 3 {
                self.write_capture_buffer(envelope_sample, 0xC00);
            }

            mixed_sample_l += output_l as i32;
            mixed_sample_r += output_r as i32;
        }
        self.capture_buffer_idx = (self.capture_buffer_idx + 2) & 0x3FF;

        if self.control.cd_audio_reverb() {
            mixed_reverb[0] += i32::from(cd_l);
            mixed_reverb[1] += i32::from(cd_r);
        }

        // TODO: this is incorrect when reverb is enabled for cd audio only?
        self.reverb.tick(mixed_reverb, &mut self.memory, self.control.reverb_master_enabled());

        mixed_sample_l += i32::from(cd_l) + self.reverb.l_out;
        mixed_sample_r += i32::from(cd_r) + self.reverb.r_out;

        if !self.control.unmuted() {
            return [cd_l, cd_r];
        }
        

        let clamped_l = mixed_sample_l.clamp(-0x8000, 0x7FFF) as i16;
        let clamped_r = mixed_sample_r.clamp(-0x8000, 0x7FFF) as i16;

        let output_l = apply_volume(clamped_l, self.main_volume_left);
        let output_r = apply_volume(clamped_r, self.main_volume_right);
        [output_l, output_r]
    }

    pub fn write_capture_buffer(&mut self, sample: i16, offset: usize) {
        let bytes = sample.to_le_bytes();
        self.memory[self.capture_buffer_idx + offset] = bytes[0];
        self.memory[self.capture_buffer_idx + offset + 1] = bytes[1];
    }

    // 8bit/16bit/32bit reads
    pub fn load<T: Addressable>(&self, address: u32) -> T {
        let width = T::width() as usize;
        // let addr = address as usize;
        // let mut buffer = [0u8;4];
        // buffer[..width].copy_from_slice(&self.data[addr..addr+width]);
        // T::from_u32(u32::from_le_bytes(buffer))
        match address {
            0x1F801C00..=0x1F801D7F => {
                let addr = address - 0x1F801C00;
                let i = (addr / 0x10) as usize;
                let r = addr % 0x10;
                match r {
                    0x0 => return T::from_u32(self.voices[i].volume_left as u32), // FIXME: sweep!
                    0x2 => return T::from_u32(self.voices[i].volume_right as u32),
                    0x4 => return T::from_u32(self.voices[i].adpcm_sample_rate as u32),
                    0x6 => return T::from_u32(self.voices[i].adpcm_start_address as u32), // in 8 bytes units
                    0x8 => return T::from_u32(self.voices[i].envelope.reg.0),
                    0xA => return T::from_u32((self.voices[i].envelope.reg.0 >> 16) as u32),
                    0xC => return T::from_u32(self.voices[i].envelope.level as u32),
                    0xE => return T::from_u32(self.voices[i].repeat_address as u32), // self.voices[ip

                    _ => panic!(
                        "unhandled {:?} load from voices, offset: 0x{:X}",
                        T::width(),
                        r
                    ),
                }
            }
            0x1F801E00..=0x1F801E5F => {
                let addr = address - 0x1F801E00;
                let i = (addr / 0x4) as usize;
                let r = addr % 0x4;
                match r {
                    // current volume internal registers
                    0 => return T::from_u32(self.voices[i].volume_left as u32),
                    2 => return T::from_u32(self.voices[i].volume_right as u32),
                    _ => panic!(
                        "unhandled {:?} load from voices, offset: 0x{:X}",
                        T::width(),
                        r
                    ),
                }
            },

            0x1F801D80 => return T::from_u32(self.main_volume_left as u32),
            0x1F801D82 => return T::from_u32(self.main_volume_right as u32),
            0x1F801D84 => return T::from_u32(self.reverb.output_volume_left as u32),
            0x1F801D86 => return T::from_u32(self.reverb.output_volume_right as u32),
            0x1F801D90 => return T::from_u32(self.get_pitch_modulation_enabled::<0>() as u32),
            0x1F801D92 => return T::from_u32(self.get_pitch_modulation_enabled::<1>() as u32),
            0x1F801D94 => return T::from_u32(self.get_noise_mode_enabled::<0>() as u32),
            0x1F801D96 => return T::from_u32(self.get_noise_mode_enabled::<1>() as u32),
            0x1F801D98 => return T::from_u32(self.get_reverb_mode::<0>() as u32),
            0x1F801D9A => return T::from_u32(self.get_reverb_mode::<1>() as u32),
            // 0x1F801DA2 => return T::from_u32(self.reverb.set_base_address(val),
            0x1F801DA4 => return T::from_u32(self.memory.irq_address as u32),
            0x1F801DB0 => return T::from_u32(self.cd_audio_input_volume_left as u32), //  (for normal CD-DA, and compressed XA-ADPCM)
            0x1F801DB2 => return T::from_u32(self.cd_audio_input_volume_right as u32), //  (for normal CD-DA, and compressed XA-ADPCM)
            0x1F801DB4 => return T::from_u32(self.external_audio_input_volume_left as u32),
            0x1F801DB6 => return T::from_u32(self.external_audio_input_volume_right as u32),
            0x1F801DC0 => return T::from_u32(self.reverb.d_apf1 as u32),
            0x1F801DC2 => return T::from_u32(self.reverb.d_apf2 as u32),
            // 0x1F801DA8 => return T::from_u32(self.push_to_data_transfer_fifo(val),
            0x1F801DC4 => return T::from_u32(self.reverb.v_iir as u32),
            0x1F801DC6 => return T::from_u32(self.reverb.v_comb1 as u32),
            0x1F801DC8 => return T::from_u32(self.reverb.v_comb2 as u32),
            0x1F801DCA => return T::from_u32(self.reverb.v_comb3 as u32),
            0x1F801DCC => return T::from_u32(self.reverb.v_comb4 as u32),
            0x1f801DCE => return T::from_u32(self.reverb.v_wall as u32),
            0x1f801DD0 => return T::from_u32(self.reverb.v_apf1 as u32),
            0x1f801DD2 => return T::from_u32(self.reverb.v_apf2 as u32),
            0x1f801DD4 => return T::from_u32(self.reverb.ml_same as u32),
            0x1f801DD6 => return T::from_u32(self.reverb.mr_same as u32),
            0x1f801DD8 => return T::from_u32(self.reverb.ml_comb1 as u32),
            0x1f801DDA => return T::from_u32(self.reverb.mr_comb1 as u32),
            0x1f801DDC => return T::from_u32(self.reverb.ml_comb2 as u32),
            0x1f801DDE => return T::from_u32(self.reverb.mr_comb2 as u32),
            0x1f801DE0 => return T::from_u32(self.reverb.dl_same as u32),
            0x1f801DE2 => return T::from_u32(self.reverb.dr_same as u32),
            0x1f801DE4 => return T::from_u32(self.reverb.ml_diff as u32),
            0x1f801DE6 => return T::from_u32(self.reverb.mr_diff as u32),
            0x1f801DE8 => return T::from_u32(self.reverb.ml_comb3 as u32),
            0x1f801DEA => return T::from_u32(self.reverb.mr_comb3 as u32),
            0x1f801DEC => return T::from_u32(self.reverb.ml_comb4 as u32),
            0x1f801DEE => return T::from_u32(self.reverb.mr_comb4 as u32),
            0x1f801DF0 => return T::from_u32(self.reverb.dl_diff as u32),
            0x1f801DF2 => return T::from_u32(self.reverb.dr_diff as u32),
            0x1f801DF4 => return T::from_u32(self.reverb.ml_apf1 as u32),
            0x1f801DF6 => return T::from_u32(self.reverb.mr_apf1 as u32),
            0x1f801DF8 => return T::from_u32(self.reverb.ml_apf2 as u32),
            0x1f801DFA => return T::from_u32(self.reverb.mr_apf2 as u32),
            0x1f801DFC => return T::from_u32(self.reverb.input_volume_left as u32),
            0x1f801DFE => return T::from_u32(self.reverb.input_volume_right as u32),


            0x1F801D88 => return T::from_u32(self.voice_key_on),
            0x1F801D8A => return T::from_u32(self.voice_key_on >> 16),
            0x1F801D8C => return T::from_u32(self.voice_key_off),
            0x1F801D8E => return T::from_u32(self.voice_key_off >> 16),
            0x1F801DA6 => return T::from_u32(self.data_transfer_address as u32),
            0x1F801D9C => return T::from_u32(self.endx::<0>() as u32),
            0x1F801D9E => return T::from_u32(self.endx::<1>() as u32),
            0x1F801DAA => return T::from_u32(self.control.0 as u32),
            0x1F801DAE => return T::from_u32(self.spu_stat()),
            0x1F801DAC => return T::from_u32(self.get_sound_ram_data_transfer_control() as u32),
            0x1F801DB8 => return T::from_u32(self.main_volume_left as u32),
            0x1F801DBA => return T::from_u32(self.main_volume_right as u32),
            _ => panic!(
                "unhandled {:?} load from spu, address: 0x{:X}",
                T::width(),
                address
            ),
        }
        // panic!("unhandled {:?} load from spu, address: 0x{:X}", T::width(), address);
    }

    pub fn spu_stat(&self) -> u32 {
        // TODO: populate!
        let mut status: SpuStatus = Default::default();
        status.set_irq9_flag(self.memory.irq.get());
        status.set_write_to_second_half(self.capture_buffer_idx > 0x200);
        status.0 as u32
    }

    // 16 bit writes, 32 bit writes unstable,
    // 8 bit writes on odd addresses are ignored,
    // 8 bit writes on even addresses are executed as 16 bit writes
    pub fn store<T: Addressable>(&mut self, address: u32, value: T) {
        let width = T::width() as usize;
        // let addr = address as usize;
        // let bytes = value.as_u32().to_le_bytes();
        // self.control.enabled();
        let val = value.as_u32() as u16;
        match address {
            0x1F801C00..=0x1F801D7F => {
                let addr = address - 0x1F801C00;
                let i = (addr / 0x10) as usize;
                let r = addr % 0x10;
                match r {
                    0x0 => self.voices[i].volume_left = (val as i16) << 1, // FIXME: sweep!
                    0x2 => self.voices[i].volume_right = (val as i16) << 1,
                    0x4 => self.voices[i].adpcm_sample_rate = val,
                    0x6 => self.voices[i].set_start_address(val), // in 8 bytes units
                    0x8 => self.voices[i].envelope.set_reg::<0>(val),
                    0xA => self.voices[i].envelope.set_reg::<1>(val),
                    0xC => self.voices[i].envelope.level = val,
                    0xE => self.voices[i].set_repeat_address(val), // self.voices[ip

                    _ => panic!(
                        "unhandled {:?} store to voices, offset: 0x{:X}, value: 0x{:X}",
                        T::width(),
                        r,
                        value.as_u32()
                    ),
                }
            }
            0x1F801D80 => self.main_volume_left = (val << 1) as i16,
            0x1F801D82 => self.main_volume_right = (val << 1) as i16,
            0x1F801D84 => self.reverb.output_volume_left = val as i16,
            0x1F801D86 => self.reverb.output_volume_right = val as i16,
            0x1F801D88 => self.voice_key_on::<0>(val),
            0x1F801D8A => self.voice_key_on::<1>(val),
            0x1F801D8C => self.voice_key_off::<0>(val),
            0x1F801D8E => self.voice_key_off::<1>(val),
            0x1F801D90 => self.pitch_modulation_enable::<0>(val),
            0x1F801D92 => self.pitch_modulation_enable::<1>(val),
            0x1F801D94 => self.noise_mode_enable::<0>(val),
            0x1F801D96 => self.noise_mode_enable::<1>(val),
            0x1F801D98 => self.reverb_mode::<0>(val),
            0x1F801D9A => self.reverb_mode::<1>(val),
            0x1F801D9C => {},// ro
            0x1F801D9E => {},// ro
            0x1F801DA2 => self.reverb.set_base_address(val),
            0x1F801DA4 => self.memory.irq_address = (val as usize) * 8,
            0x1F801DB0 => self.cd_audio_input_volume_left = val as i16, //  (for normal CD-DA, and compressed XA-ADPCM)
            0x1F801DB2 => self.cd_audio_input_volume_right = val as i16, //  (for normal CD-DA, and compressed XA-ADPCM)
            0x1F801DB4 => self.external_audio_input_volume_left = val as i16,
            0x1F801DB6 => self.external_audio_input_volume_right = val as i16,
            0x1F801DC0 => self.reverb.d_apf1 = (val as usize) * 8,
            0x1F801DC2 => self.reverb.d_apf2 = (val as usize) * 8,
            0x1F801DA6 => self.set_data_transfer_address(val),
            0x1F801DA8 => self.push_to_data_transfer_fifo(val),
            0x1F801DAC => self.set_sound_ram_data_transfer_control(val), // Sound ram data transfer control (should be 0004h)
            0x1F801DAA => self.set_control(val),
            0x1F801DC4 => self.reverb.v_iir = val as i16 as i32,
            0x1F801DC6 => self.reverb.v_comb1 = val as i16 as i32,
            0x1F801DC8 => self.reverb.v_comb2 = val as i16 as i32,
            0x1F801DCA => self.reverb.v_comb3 = val as i16 as i32,
            0x1F801DCC => self.reverb.v_comb4 = val as i16 as i32,
            0x1f801DCE => self.reverb.v_wall = val as i16 as i32,
            0x1f801DD0 => self.reverb.v_apf1 = val as i16 as i32,
            0x1f801DD2 => self.reverb.v_apf2 = val as i16 as i32,
            0x1f801DD4 => self.reverb.ml_same = (val as usize) * 8,
            0x1f801DD6 => self.reverb.mr_same = (val as usize) * 8,
            0x1f801DD8 => self.reverb.ml_comb1 = (val as usize) * 8,
            0x1f801DDA => self.reverb.mr_comb1 = (val as usize) * 8,
            0x1f801DDC => self.reverb.ml_comb2 = (val as usize) * 8,
            0x1f801DDE => self.reverb.mr_comb2 = (val as usize) * 8,
            0x1f801DE0 => self.reverb.dl_same = (val as usize) * 8,
            0x1f801DE2 => self.reverb.dr_same = (val as usize) * 8,
            0x1f801DE4 => self.reverb.ml_diff = (val as usize) * 8,
            0x1f801DE6 => self.reverb.mr_diff = (val as usize) * 8,
            0x1f801DE8 => self.reverb.ml_comb3 = (val as usize) * 8,
            0x1f801DEA => self.reverb.mr_comb3 = (val as usize) * 8,
            0x1f801DEC => self.reverb.ml_comb4 = (val as usize) * 8,
            0x1f801DEE => self.reverb.mr_comb4 = (val as usize) * 8,
            0x1f801DF0 => self.reverb.dl_diff = (val as usize) * 8,
            0x1f801DF2 => self.reverb.dr_diff = (val as usize) * 8,
            0x1f801DF4 => self.reverb.ml_apf1 = (val as usize) * 8,
            0x1f801DF6 => self.reverb.mr_apf1 = (val as usize) * 8,
            0x1f801DF8 => self.reverb.ml_apf2 = (val as usize) * 8,
            0x1f801DFA => self.reverb.mr_apf2 = (val as usize) * 8,
            0x1f801DFC => self.reverb.input_volume_left = val as i16,
            0x1f801DFE => self.reverb.input_volume_right = val as i16,
            _ => panic!(
                "unhandled {:?} store to spu, address: 0x{:X}, value: 0x{:X}",
                T::width(),
                address,
                value.as_u32()
            ),
        }

        // self.data[addr..addr+width].copy_from_slice(&bytes[..width]);
        // panic!("unhandled {:?} store to spu, address: 0x{:X}, value: 0x{:X}", T::width(), address, value.as_u32());
    }

    pub fn set_control(&mut self, v: u16) {
        self.control.0 = v;
        self.noise.update_frequency(self.control.noise_frequency_step(), self.control.noise_frequency_shift());
        self.memory.irq_enabled = self.control.irq9_enabled();
        if !self.control.irq9_enabled() {
            self.memory.irq.set(false);
        }
    }
    pub fn endx<const HIGH: usize>(&self) -> u16 {
        let base = HIGH * 16;
        let count = if HIGH == 1 { 8 } else { 16 };

        (0..count).fold(0, |acc, i| {
            acc | (u16::from(self.voices[base + i].reached_loop_end) << i)
        })

    }

    // starts adsr envelope and automatically initializes ADSR volume to zero
    pub fn voice_key_on<const HIGH: usize>(&mut self, value: u16) {
        write_half::<HIGH>(&mut self.voice_key_on, value);
        let mut v = value;
        let base = HIGH * 16;
        let count = if HIGH == 1 { 8 } else { 16 };
        for i in 0..count {
            if v & 0x1 == 1 {
                self.voices[base + i].key_on(&self.memory);
                // println!("voice key on {}", base + i);
                // keying on resets both the envelope state and adpcm decoding state
            }
            v = v >> 1;
        }
    }
    pub fn voice_key_off<const HIGH: usize>(&mut self, value: u16) {
        write_half::<HIGH>(&mut self.voice_key_off, value);
        let mut v = value;
        let base = HIGH * 16;
        let count = if HIGH == 1 { 8 } else { 16 };
        for i in 0..count {
            if v & 0x1 == 1 {
                self.voices[base + i].key_off();
                // println!("voice key off {}", base + i);
            }
            v = v >> 1;
        }
    }

    pub fn pitch_modulation_enable<const HIGH: usize>(&mut self, value: u16) {
        let mut v = value;
        if HIGH == 0 {
            v = v >> 1; // bit 0 unused
        }
        let base = HIGH * 16;
        let count = if HIGH == 1 { 8 } else { 15 };
        let start = if HIGH == 1 { 0 } else { 1 };
        for i in start..count {
            self.voices[base + i].modulation_enabled = v & 0x1 == 1;
            if v & 0x1 == 1 {
                // TODO: voice base + i modulation enabled
                // TODO: 0 - normal, 1 - modulate by n-1
            }
            v = v >> 1;
        }
    }

    pub fn get_pitch_modulation_enabled<const HIGH: usize>(&self) -> u16 {
        let mut v = 0_u16;
        let base = HIGH * 16;
        let count = if HIGH == 1 { 8 } else { 15 };
        let start = if HIGH == 1 { 0 } else { 1 };
        for i in start..count {
            v |= (self.voices[base + i].modulation_enabled as u16) << i;
        }
        v
    }

    pub fn noise_mode_enable<const HIGH: usize>(&mut self, value: u16) {
        let mut v = value;
        let base = HIGH * 16;
        let count = if HIGH == 1 { 8 } else { 16 };
        for i in 0..count {
            self.voices[base + i].noise_mode = v & 0x1 == 1;
            v = v >> 1;
        }
    }

    pub fn get_noise_mode_enabled<const HIGH: usize>(&self) -> u16 {
        let mut v = 0;
        let base = HIGH * 16;
        let count = if HIGH == 1 { 8 } else { 16 };
        for i in 0..count {
            v |= (self.voices[base + i].noise_mode as u16) << i;
        }
        v
    }

    pub fn reverb_mode<const HIGH: usize>(&mut self, value: u16) {
        let mut v = value;
        let base = HIGH * 16;
        let count = if HIGH == 1 { 8 } else { 16 };
        for i in 0..count {
            self.voices[base + i].reverb_mode = v & 0x1 == 1;
            v = v >> 1;
        }
    }
    pub fn get_reverb_mode<const HIGH: usize>(&self) -> u16 {
        let mut v = 0_u16;
        let base = HIGH * 16;
        let count = if HIGH == 1 { 8 } else { 16 };
        for i in 0..count {
            v |= (self.voices[base + i].reverb_mode as u16) << i;
        }
        v
    }

    /*
    *   __Transfer Type___Halfwords in Fifo________Halfwords written to SPU RAM__
          0,1,6,7  Fill     A,B,C,D,E,F,G,H,...,X    X,X,X,X,X,X,X,X,...
          2        Normal   A,B,C,D,E,F,G,H,...,X    A,B,C,D,E,F,G,H,...
          3        Rep2     A,B,C,D,E,F,G,H,...,X    A,A,C,C,E,E,G,G,...
          4        Rep4     A,B,C,D,E,F,G,H,...,X    A,A,A,A,E,E,E,E,...
          5        Rep8     A,B,C,D,E,F,G,H,...,X    H,H,H,H,H,H,H,H,...
    */
    pub fn set_sound_ram_data_transfer_control(&mut self, value: u16) {
        let data_transfer_type = ((value >> 1) & 0x7) as u8;
        // println!("data transfer type: {}", data_transfer_type);
        match data_transfer_type {
            0 | 1 | 6 | 7 => self.data_transfer_type = DataTransferType::Fill,
            2 => self.data_transfer_type = DataTransferType::Normal,
            3 => self.data_transfer_type = DataTransferType::Rep2,
            4 => self.data_transfer_type = DataTransferType::Rep4,
            5 => self.data_transfer_type = DataTransferType::Rep8,
            _ => unreachable!("should be 0..7"),
        }
    }
    pub fn get_sound_ram_data_transfer_control(&self) -> u16 {
        let r = match self.data_transfer_type {
            DataTransferType::Fill => 0,
            DataTransferType::Normal => 2,
            DataTransferType::Rep2 => 3,
            DataTransferType::Rep4 => 4,
            DataTransferType::Rep8 => 5,
        };
        (r as u16) << 1
    }

    pub fn set_data_transfer_address(&mut self, value: u16) {
        self.data_transfer_address = value;
        self.current_address = value as usize * 8;
    }

    // TODO: implement this buffer?
    pub fn push_to_data_transfer_fifo(&mut self, value: u16) {
        self.ram_write::<2>(value as u32);
    }

    pub fn ram_write<const WIDTH: usize>(&mut self, value: u32) {
        let address = self.current_address & 0x7FFFF;
        let bytes = value.to_le_bytes();
        for i in 0..WIDTH {
            self.memory[address + i] = bytes[i];
        }

        self.current_address += WIDTH;
    }
}

pub enum DataTransferType {
    Fill,
    Normal,
    Rep2,
    Rep4,
    Rep8,
}
pub fn signed4bit(v: u8) -> i32 {
    i32::from((v as i8) << 4 >> 4)
}
fn clamped_i16(a: i32) -> i16 {
    a.clamp(-0x8000, 0x7FFF) as i16
}
// from starpsx
fn decode_adpcm_block2(
    block: &[u8],
    decoded: &mut [i16; 28],
    old_sample: &mut i16,
    older_sample: &mut i16,
) {
    let shift = block[0] & 0x0F;
    let shift = 12 - if shift > 12 { 9 } else { shift };
    let filter = ((block[0] & 0x70) >> 4).min(4);

    let f0 = POS_ADPCM_TABLE[usize::from(filter)];
    let f1 = NEG_ADPCM_TABLE[usize::from(filter)];

    for i in 0..28 {
        let old = i32::from(*old_sample);
        let older = i32::from(*older_sample);

        let t = signed4bit((block[2 + i / 2] >> (4 * (i & 1))) & 0xF);
        let s = clamped_i16((t << shift) + (old * f0 + older * f1 + 32) / 64);

        *older_sample = *old_sample;
        *old_sample = s;

        decoded[i] = s;
    }
}
// from https://jsgroth.dev/blog/posts/ps1-spu-part-1/
pub fn decode_adpcm_block(
    block: &[u8],
    decoded: &mut [i16; 28],
    old_sample: &mut i16,
    older_sample: &mut i16,
) {
    // First byte is a header byte specifying the shift value (bits 0-3) and the filter value (bits 4-6).
    // A shift value of 13-15 is invalid and behaves the same as shift=9
    let shift = block[0] & 0x0F;
    let shift = 12 - if shift > 12 { 9 } else { shift };

    // Filter values can only range from 0 to 4
    let filter = std::cmp::min(4, (block[0] >> 4) & 0x7);
    let f0 = POS_ADPCM_TABLE[usize::from(filter)];
    let f1 = NEG_ADPCM_TABLE[usize::from(filter)];

    // The second byte is another header byte specifying loop flags; ignore that for now

    // The remaining 14 bytes are encoded sample values
    for sample_idx in 0..28 {
        // Read the raw 4-bit sample value from the block.
        // Samples are stored little-endian within a byte
        let sample_byte = block[2 + sample_idx / 2];
        let sample_nibble = (sample_byte >> (4 * (sample_idx % 2))) & 0x0F;

        // Sign extended from 4 bits to 32 bits
        let raw_sample: i32 = (((sample_nibble as i8) << 4) >> 4).into();

        // Apply the sift; a shift value of N is decoded shifting left (12 - N)
        let shifted_sample = raw_sample << shift;

        // Apply the filter formula.
        let old = i32::from(*old_sample);
        let older = i32::from(*older_sample);
        let filtered_sample = shifted_sample + (f0 * old + f1 * older + 32) / 64;

        // let filtered_sample = match filter {
        //     // no filtering
        //     0 => shifted_sample,
        //     // filter using previous sample
        //     1 => shifted_sample + (60 * old + 32) / 64,
        //     // 2-4 filter using previous two samples
        //     2 => shifted_sample + (115 * old - 52 * older + 32) / 64,
        //     3 => shifted_sample + (98 * old - 55 * older + 32) / 64,
        //     4 => shifted_sample + (122 * old - 60 * older + 32) / 64,
        //     _ => unreachable!("filter is clamped to 0..4")
        // };

        // finally clamp to sighned 16 bit
        let clamped_sample = filtered_sample.clamp(-0x8000, 0x7FFF) as i16;
        decoded[sample_idx] = clamped_sample;

        // update sliding window
        *older_sample = *old_sample;
        *old_sample = clamped_sample;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FromPrimitive)]
#[repr(u8)]
enum Direction {
    #[default]
    Increasing = 0,
    Decreasing = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FromPrimitive)]
#[repr(u8)]
enum Mode {
    #[default]
    Linear = 0,
    Exponential = 1,
}
const ENVELOPE_COUNTER_MAX: u32 = 1 << (33 - 11);

bitfield::bitfield! {
    #[derive(Default, Copy, Clone)]
    struct AdsrConfig(u32);
    u8, into Mode, sustain_mode, _: 31,31;
    u8, into Direction, sustain_direction, _:30,30;
    // (0..1Fh = Fast..Slow)
    u8, sustain_shift, _: 28,24;
    // (0..3 = "+7,+6,+5,+4" or "-8,-7,-6,-5") (inc/dec)
    u8, sustain_step, _: 23,22;
 //  -     Release Direction (Fixed, always Decrease) (until Level 0000h)
    u8, into Mode, release_mode, _: 21,21;
    u8, release_shift, _: 20,16;
 //  -     Release Step      (Fixed, always "-8")
    u8, into Mode, attack_mode, _: 15,15;
 //  -     Attack Direction  (Fixed, always Increase) (until Level 7FFFh)
    u8, attack_shift, _: 14,10;
    u8, attack_step, _: 9,8;
 //  -     Decay Mode        (Fixed, always Exponential)
 //  -     Decay Direction   (Fixed, always Decrease) (until Sustain Level)
    u8, decay_shift, _: 7,4;
 //  -     Decay Step        (Fixed, always "-8")
    //;Level=(N+1)*800h
    u8, sustain_level, _: 3,0;
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
enum AdsrPhase {
    #[default]
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Default, Copy, Clone)]
struct AdsrEnvelope {
    reg: AdsrConfig,
    phase: AdsrPhase,
    level: u16,
    counter: u32,
    sustain_level: u16,
}

impl AdsrEnvelope {
    pub fn set_reg<const HIGH: usize>(&mut self, value: u16) {
        write_half::<HIGH>(&mut self.reg.0, value);
        self.sustain_level = (self.reg.sustain_level() + 1) as u16 * 0x800;
    }

    fn key_on(&mut self) {
        self.level = 0;
        self.phase = AdsrPhase::Attack;
    }
    fn key_off(&mut self) {
        self.phase = AdsrPhase::Release;
    }

    pub fn clock(&mut self) {
        let (direction, step, shift, mode) = match self.phase {
            AdsrPhase::Attack => (
                Direction::Increasing,
                self.reg.attack_step(),
                self.reg.attack_shift(),
                self.reg.attack_mode(),
            ),
            AdsrPhase::Release => (
                Direction::Decreasing,
                0,
                self.reg.release_shift(),
                self.reg.release_mode(),
            ),
            AdsrPhase::Sustain => (
                self.reg.sustain_direction(),
                self.reg.sustain_step(),
                self.reg.sustain_shift(),
                self.reg.sustain_mode(),
            ),
            AdsrPhase::Decay => (
                Direction::Decreasing,
                0,
                self.reg.decay_shift(),
                Mode::Exponential,
            ),
        };
        self.check_fore_phase_transition();
        // For a shift value of N, the envelope should update every 1 << (N - 11) cycles.
        // Accomplish this by using a counter decrement of MAX >> (N - 11)
        let mut counter_decrement = ENVELOPE_COUNTER_MAX >> shift.saturating_sub(11);

        // Quadruple the update interval if in exponential increase mode and volume is above the threshold
        if direction == Direction::Increasing && mode == Mode::Exponential && self.level > 0x6000 {
            counter_decrement >>= 2;
        }
        self.counter = self.counter.saturating_sub(counter_decrement);
        if self.counter == 0 {
            self.counter = ENVELOPE_COUNTER_MAX;
            self.update_envelope(direction, step, shift, mode);
        }
    }
    // FIXME: inaccurate, check docs
    pub fn update_envelope(&mut self, direction: Direction, step: u8, shift: u8, mode: Mode) {
        let mut step = i32::from(7 - step);
        if direction == Direction::Decreasing {
            step = !step;
        }

        step <<= 11_u8.saturating_sub(shift);

        let current_level: i32 = self.level.into();
        if direction == Direction::Decreasing && mode == Mode::Exponential {
            step = (step * current_level) >> 15;
        }

        self.level = (current_level + step).clamp(0, 0x7FFF) as u16;
    }

    pub fn check_fore_phase_transition(&mut self) {
        if self.phase == AdsrPhase::Attack && self.level == 0x7FFF {
            self.phase = AdsrPhase::Decay;
        }

        if self.phase == AdsrPhase::Decay && (self.level as u16) <= self.sustain_level {
            self.phase = AdsrPhase::Sustain;
        }
    }
}

pub struct SoundRam {
    ram: Box<[u8]>,
    irq_enabled: bool,
    irq_address: usize,
    irq: Cell<bool>
}

impl Index<usize> for SoundRam {
    type Output = u8;

    fn index(&self, index: usize) -> &Self::Output {
        if self.irq_enabled && index == self.irq_address {
            self.irq.set(true);
        }
        &self.ram[index]
    }
}

impl Index<Range<usize>> for SoundRam {
    type Output = [u8];

    fn index(&self, range: Range<usize>) -> &Self::Output {
        if self.irq_enabled && (range.contains(&self.irq_address)) {
            self.irq.set(true);
        }
        &self.ram[range]
    }
}

impl IndexMut<usize> for SoundRam {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        if self.irq_enabled && index == self.irq_address {
            self.irq.set(true);
        }
        &mut self.ram[index]
    }
}


pub const GAUSSIAN_TABLE: [i32; 512] = [
    -0x001, -0x001, -0x001, -0x001, -0x001, -0x001, -0x001, -0x001, -0x001, -0x001, -0x001, -0x001,
    -0x001, -0x001, -0x001, -0x001, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0001,
    0x0001, 0x0001, 0x0001, 0x0002, 0x0002, 0x0002, 0x0003, 0x0003, 0x0003, 0x0004, 0x0004, 0x0005,
    0x0005, 0x0006, 0x0007, 0x0007, 0x0008, 0x0009, 0x0009, 0x000A, 0x000B, 0x000C, 0x000D, 0x000E,
    0x000F, 0x0010, 0x0011, 0x0012, 0x0013, 0x0015, 0x0016, 0x0018, 0x0019, 0x001B, 0x001C, 0x001E,
    0x0020, 0x0021, 0x0023, 0x0025, 0x0027, 0x0029, 0x002C, 0x002E, 0x0030, 0x0033, 0x0035, 0x0038,
    0x003A, 0x003D, 0x0040, 0x0043, 0x0046, 0x0049, 0x004D, 0x0050, 0x0054, 0x0057, 0x005B, 0x005F,
    0x0063, 0x0067, 0x006B, 0x006F, 0x0074, 0x0078, 0x007D, 0x0082, 0x0087, 0x008C, 0x0091, 0x0096,
    0x009C, 0x00A1, 0x00A7, 0x00AD, 0x00B3, 0x00BA, 0x00C0, 0x00C7, 0x00CD, 0x00D4, 0x00DB, 0x00E3,
    0x00EA, 0x00F2, 0x00FA, 0x0101, 0x010A, 0x0112, 0x011B, 0x0123, 0x012C, 0x0135, 0x013F, 0x0148,
    0x0152, 0x015C, 0x0166, 0x0171, 0x017B, 0x0186, 0x0191, 0x019C, 0x01A8, 0x01B4, 0x01C0, 0x01CC,
    0x01D9, 0x01E5, 0x01F2, 0x0200, 0x020D, 0x021B, 0x0229, 0x0237, 0x0246, 0x0255, 0x0264, 0x0273,
    0x0283, 0x0293, 0x02A3, 0x02B4, 0x02C4, 0x02D6, 0x02E7, 0x02F9, 0x030B, 0x031D, 0x0330, 0x0343,
    0x0356, 0x036A, 0x037E, 0x0392, 0x03A7, 0x03BC, 0x03D1, 0x03E7, 0x03FC, 0x0413, 0x042A, 0x0441,
    0x0458, 0x0470, 0x0488, 0x04A0, 0x04B9, 0x04D2, 0x04EC, 0x0506, 0x0520, 0x053B, 0x0556, 0x0572,
    0x058E, 0x05AA, 0x05C7, 0x05E4, 0x0601, 0x061F, 0x063E, 0x065C, 0x067C, 0x069B, 0x06BB, 0x06DC,
    0x06FD, 0x071E, 0x0740, 0x0762, 0x0784, 0x07A7, 0x07CB, 0x07EF, 0x0813, 0x0838, 0x085D, 0x0883,
    0x08A9, 0x08D0, 0x08F7, 0x091E, 0x0946, 0x096F, 0x0998, 0x09C1, 0x09EB, 0x0A16, 0x0A40, 0x0A6C,
    0x0A98, 0x0AC4, 0x0AF1, 0x0B1E, 0x0B4C, 0x0B7A, 0x0BA9, 0x0BD8, 0x0C07, 0x0C38, 0x0C68, 0x0C99,
    0x0CCB, 0x0CFD, 0x0D30, 0x0D63, 0x0D97, 0x0DCB, 0x0E00, 0x0E35, 0x0E6B, 0x0EA1, 0x0ED7, 0x0F0F,
    0x0F46, 0x0F7F, 0x0FB7, 0x0FF1, 0x102A, 0x1065, 0x109F, 0x10DB, 0x1116, 0x1153, 0x118F, 0x11CD,
    0x120B, 0x1249, 0x1288, 0x12C7, 0x1307, 0x1347, 0x1388, 0x13C9, 0x140B, 0x144D, 0x1490, 0x14D4,
    0x1517, 0x155C, 0x15A0, 0x15E6, 0x162C, 0x1672, 0x16B9, 0x1700, 0x1747, 0x1790, 0x17D8, 0x1821,
    0x186B, 0x18B5, 0x1900, 0x194B, 0x1996, 0x19E2, 0x1A2E, 0x1A7B, 0x1AC8, 0x1B16, 0x1B64, 0x1BB3,
    0x1C02, 0x1C51, 0x1CA1, 0x1CF1, 0x1D42, 0x1D93, 0x1DE5, 0x1E37, 0x1E89, 0x1EDC, 0x1F2F, 0x1F82,
    0x1FD6, 0x202A, 0x207F, 0x20D4, 0x2129, 0x217F, 0x21D5, 0x222C, 0x2282, 0x22DA, 0x2331, 0x2389,
    0x23E1, 0x2439, 0x2492, 0x24EB, 0x2545, 0x259E, 0x25F8, 0x2653, 0x26AD, 0x2708, 0x2763, 0x27BE,
    0x281A, 0x2876, 0x28D2, 0x292E, 0x298B, 0x29E7, 0x2A44, 0x2AA1, 0x2AFF, 0x2B5C, 0x2BBA, 0x2C18,
    0x2C76, 0x2CD4, 0x2D33, 0x2D91, 0x2DF0, 0x2E4F, 0x2EAE, 0x2F0D, 0x2F6C, 0x2FCC, 0x302B, 0x308B,
    0x30EA, 0x314A, 0x31AA, 0x3209, 0x3269, 0x32C9, 0x3329, 0x3389, 0x33E9, 0x3449, 0x34A9, 0x3509,
    0x3569, 0x35C9, 0x3629, 0x3689, 0x36E8, 0x3748, 0x37A8, 0x3807, 0x3867, 0x38C6, 0x3926, 0x3985,
    0x39E4, 0x3A43, 0x3AA2, 0x3B00, 0x3B5F, 0x3BBD, 0x3C1B, 0x3C79, 0x3CD7, 0x3D35, 0x3D92, 0x3DEF,
    0x3E4C, 0x3EA9, 0x3F05, 0x3F62, 0x3FBD, 0x4019, 0x4074, 0x40D0, 0x412A, 0x4185, 0x41DF, 0x4239,
    0x4292, 0x42EB, 0x4344, 0x439C, 0x43F4, 0x444C, 0x44A3, 0x44FA, 0x4550, 0x45A6, 0x45FC, 0x4651,
    0x46A6, 0x46FA, 0x474E, 0x47A1, 0x47F4, 0x4846, 0x4898, 0x48E9, 0x493A, 0x498A, 0x49D9, 0x4A29,
    0x4A77, 0x4AC5, 0x4B13, 0x4B5F, 0x4BAC, 0x4BF7, 0x4C42, 0x4C8D, 0x4CD7, 0x4D20, 0x4D68, 0x4DB0,
    0x4DF7, 0x4E3E, 0x4E84, 0x4EC9, 0x4F0E, 0x4F52, 0x4F95, 0x4FD7, 0x5019, 0x505A, 0x509A, 0x50DA,
    0x5118, 0x5156, 0x5194, 0x51D0, 0x520C, 0x5247, 0x5281, 0x52BA, 0x52F3, 0x532A, 0x5361, 0x5397,
    0x53CC, 0x5401, 0x5434, 0x5467, 0x5499, 0x54CA, 0x54FA, 0x5529, 0x5558, 0x5585, 0x55B2, 0x55DE,
    0x5609, 0x5632, 0x565B, 0x5684, 0x56AB, 0x56D1, 0x56F6, 0x571B, 0x573E, 0x5761, 0x5782, 0x57A3,
    0x57C3, 0x57E2, 0x57FF, 0x581C, 0x5838, 0x5853, 0x586D, 0x5886, 0x589E, 0x58B5, 0x58CB, 0x58E0,
    0x58F4, 0x5907, 0x5919, 0x592A, 0x593A, 0x5949, 0x5958, 0x5965, 0x5971, 0x597C, 0x5986, 0x598F,
    0x5997, 0x599E, 0x59A4, 0x59A9, 0x59AD, 0x59B0, 0x59B2, 0x59B3,
];

const FIR_FILTER: &[i32; 39] = &[
    -0x0001, 0x0000, 0x0002, 0x0000, -0x000A, 0x0000, 0x0023, 0x0000, -0x0067, 0x0000, 0x010A,
    0000, -0x0268, 0000, 0x0534, 0000, -0x0B90, 0000, 0x2806, 0x4000, 0x2806, 0000, -0x0B90, 0000,
    0x0534, 0000, -0x0268, 0000, 0x010A, 0000, -0x0067, 0000, 0x0023, 0000, -0x000A, 0000, 0x0002,
    0000, -0x0001,
];

pub fn write_half<const HIGH: usize>(r: &mut u32, value: u16) {
    let shift = HIGH * 16;
    let mask = 0xFFFF << shift;
    *r = (*r & !mask) | ((value as u32) << shift);
}

fn apply_volume(sample: i16, volume: i16) -> i16 {
    // Do multiplication in 32 bits to avoid possible overflow
    ((i32::from(sample) * i32::from(volume)) >> 15) as i16
}

pub const POS_ADPCM_TABLE: [i32; 5] = [0, 60, 115, 98, 122];
pub const NEG_ADPCM_TABLE: [i32; 5] = [0, 0, -52, -55, -60];
const fn mul_16(a: i32, b: i32) -> i32 {
    a.saturating_mul(b) >> 15
}
