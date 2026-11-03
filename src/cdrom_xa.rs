use serde::{Deserialize, Serialize};

use crate::{adpcm::decode_xa_block, cdrom::Image};

const XA_STEREO_SAMPLES: usize = 2016;
const XA_MONO_SAMPLES: usize = 4032;
const XA_PHASE_UNIT: u32 = 44_100;

#[derive(Serialize, Deserialize)]
pub struct XaState {
    active: bool,
    lba: u64,
    xa_adpcm_enabled: bool,
    filter_enabled: bool,
    filter_file: u8,
    filter_channel: u8,
    file: u8,
    channel: u8,
    sample_rate: u32,
    stereo: bool,
    bits_per_sample: u8,
    emphasis: bool,
    decoded_samples: usize,
    sample_index: usize,
    phase: u32,
    prev_left: [i32; 2],
    prev_right: [i32; 2],

    #[serde(with = "serde_arrays")]
    left_buf: [i16; XA_STEREO_SAMPLES],
    #[serde(with = "serde_arrays")]
    right_buf: [i16; XA_STEREO_SAMPLES],
    #[serde(with = "serde_arrays")]
    mono_buf: [i16; XA_MONO_SAMPLES],
}

impl XaState {
    pub fn new() -> Self {
        XaState {
            active: false,
            lba: 0,
            xa_adpcm_enabled: false,
            filter_enabled: false,
            filter_file: 0,
            filter_channel: 0,
            file: 0,
            channel: 0,
            sample_rate: 18_900,
            stereo: false,
            bits_per_sample: 0,
            emphasis: false,
            decoded_samples: 0,
            sample_index: 0,
            phase: 0,
            prev_left: [0; 2],
            prev_right: [0; 2],
            left_buf: [0; XA_STEREO_SAMPLES],
            right_buf: [0; XA_STEREO_SAMPLES],
            mono_buf: [0; XA_MONO_SAMPLES],
        }
    }

    pub fn reset(&mut self) {
        self.active = false;
        self.lba = 0;
        self.xa_adpcm_enabled = false;
        self.filter_enabled = false;
        self.filter_file = 0;
        self.filter_channel = 0;
        self.file = 0;
        self.channel = 0;
        self.sample_rate = 18_900;
        self.stereo = false;
        self.bits_per_sample = 0;
        self.emphasis = false;
        self.decoded_samples = 0;
        self.sample_index = 0;
        self.phase = 0;
        self.prev_left = [0; 2];
        self.prev_right = [0; 2];
    }

    pub fn begin_at(&mut self, lba: u64) {
        let xa_adpcm_enabled = self.xa_adpcm_enabled;
        let filter_enabled = self.filter_enabled;
        let filter_file = self.filter_file;
        let filter_channel = self.filter_channel;
        self.reset();
        self.xa_adpcm_enabled = xa_adpcm_enabled;
        self.filter_enabled = filter_enabled;
        self.filter_file = filter_file;
        self.filter_channel = filter_channel;
        self.active = true;
        self.lba = lba;
    }

    pub fn set_mode(&mut self, xa_enabled: bool, filter_enabled: bool) {
        self.xa_adpcm_enabled = xa_enabled;
        self.filter_enabled = filter_enabled;
    }

    pub fn set_filter(&mut self, file: u8, channel: u8) {
        self.filter_file = file;
        self.filter_channel = channel;
    }

    pub fn is_xa_audio_sector(&self, sector: &Vec<u8>) -> bool {
        if !self.xa_adpcm_enabled {
            return false;
        }
        if sector.len() != 2352 {
            return false;
        }
        if sector[0xF] != 2 {
            return false;
        }
        let file = sector[0x10];
        let channel = sector[0x11];
        let submode = sector[0x12];

        // let is_realtime_audio = (submode & 0x44) == 0x44;
        let is_form2 = submode & (1 << 5) != 0;
        let is_audio = submode & (1 << 2) != 0;
        let is_realtime = submode & (1 << 6) != 0;
        if !is_form2 {
            return false;
        }
        if !is_realtime || !is_audio {
            return false;
        }
        if self.filter_enabled {
            if file != self.filter_file || channel != self.filter_channel {
                return false;
            }
        }
        return true;
    }

    pub fn consume_sample(&mut self, disc: &mut Image) -> [i16; 2] {
        if !self.active {
            return [0, 0];
        }

        while self.sample_index >= self.decoded_samples {
            if !self.fetch_next_sector(disc) {
                self.active = false;
                return [0, 0];
            }
        }

        let sample = match self.stereo {
            true => [
                self.left_buf[self.sample_index],
                self.right_buf[self.sample_index],
            ],
            false => [
                self.mono_buf[self.sample_index],
                self.mono_buf[self.sample_index],
            ],
        };

        self.phase = self.phase.wrapping_add(self.sample_rate);

        while self.phase >= XA_PHASE_UNIT {
            self.phase -= XA_PHASE_UNIT;
            self.sample_index += 1;
        }

        sample
    }

    pub fn fetch_next_sector(&mut self, disc: &mut Image) -> bool {
        if !self.active {
            return false;
        }

        loop {
            let Some(sector) = disc.read_sector_raw_at(self.lba) else {
                return false;
            };
            self.lba += 1;

            if sector[15] != 2 {
                continue;
            }

            let subheader = parse_subheader(&sector);
            if subheader.submode.eor() {
                return false;
            }
            if !subheader.submode.form2() {
                continue;
            }
            if !subheader.submode.audio() || !subheader.submode.realtime() {
                continue;
            }

            if self.filter_enabled && (subheader.file != self.filter_file || subheader.channel != self.filter_channel) {
                continue;
            }

            return self.decode_sector(&sector[24..24 + 0x900], subheader);
        }
    }

    pub fn decode_sector(&mut self, data: &[u8], subheader: Subheader) -> bool {
        if subheader.codinginfo.stereo() > 1 || subheader.codinginfo.bits_per_sample() > 0 {
            return false;
        }
        let stereo = subheader.codinginfo.stereo() == 1;
        let sample_rate = if !subheader.codinginfo.sample_rate() {
            37_800
        } else {
            18_900
        };

        if self.file != subheader.file
            || self.channel != subheader.channel
            || self.stereo != stereo
            || self.sample_rate != sample_rate
        {
            self.prev_left = [0, 0];
            self.prev_right = [0, 0];
            self.sample_index = 0;
            self.phase = 0;
        }

        self.file = subheader.file;
        self.channel = subheader.channel;
        self.stereo = stereo;
        self.sample_rate = sample_rate;
        self.bits_per_sample = subheader.codinginfo.bits_per_sample();
        self.emphasis = subheader.codinginfo.emphasis();

        let mut left_block = [0_i16; 28];
        let mut right_block = [0_i16; 28];
        let mut block_data = [0_u8; 28];

        let mut left_pos = 0_usize;
        let mut right_pos = 0_usize;
        let mut mono_pos = 0_usize;

        for group_i in 0..18 {
            let group: &[u8; 128] = data[group_i * 128..][0..128].try_into().expect("ok");

            for blk in 0..4 {
                let left_header = group[4 + blk * 2 + 0];
                let right_header = group[4 + blk * 2 + 1];

                for sample_i in 0..28 {
                    block_data[sample_i] = group[16 + blk + sample_i * 4]
                }

                if stereo {
                    decode_xa_block(
                        left_header,
                        &block_data,
                        0,
                        &mut self.prev_left,
                        &mut left_block,
                    );
                    decode_xa_block(
                        right_header,
                        &block_data,
                        4,
                        &mut self.prev_right,
                        &mut right_block,
                    );
                    self.left_buf[left_pos..left_pos + 28].copy_from_slice(&left_block[0..]);
                    self.right_buf[right_pos..right_pos + 28].copy_from_slice(&right_block[0..]);

                    left_pos += 28;
                    right_pos += 28;
                } else {
                    decode_xa_block(
                        left_header,
                        &block_data,
                        0,
                        &mut self.prev_left,
                        &mut left_block,
                    );
                    self.mono_buf[mono_pos..mono_pos + 28].copy_from_slice(&left_block[0..]);
                    mono_pos += 28;
                    decode_xa_block(
                        right_header,
                        &block_data,
                        4,
                        &mut self.prev_left,
                        &mut left_block,
                    );
                    self.mono_buf[mono_pos..mono_pos + 28].copy_from_slice(&left_block[0..]);
                    mono_pos += 28;
                }
            }
        }

        self.decoded_samples = if stereo { left_pos } else { mono_pos };
        self.sample_index = 0;
        return true;
    }
}

bitfield::bitfield! {
    #[derive(Default, Serialize, Deserialize)]
    pub struct Submode(u8);
    eor, _:  0;
    video, _:  1;
    audio, _:  2;
    data, _:  3;
    trigger, _:  4;
    form2, _:  5;
    realtime, _:  6;
    eof, _:  7;
}

bitfield::bitfield! {
    #[derive(Default, Serialize, Deserialize)]
    pub struct CodingInfo(u8);
    stereo, _:  1,0;
    sample_rate, _:  2;
    bits_per_sample, _:  5,4;
    emphasis, _:  6;
}

pub struct Subheader {
    file: u8,
    channel: u8,
    submode: Submode,
    codinginfo: CodingInfo,
}

pub fn parse_subheader(sector: &Vec<u8>) -> Subheader {
    Subheader {
        file: sector[16],
        channel: sector[17],
        submode: Submode(sector[18]),
        codinginfo: CodingInfo(sector[19]),
    }
}
