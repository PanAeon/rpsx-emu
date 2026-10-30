use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::ops::Div;

use arrayvec::ArrayVec;

use crate::cd_cache::CdCache;
use crate::cdxa::{self, AdpcmHistory, HighResResampler, LowResResampler, decode_audio_sector};
use crate::iso9660;
use crate::{
    system::{Addressable, System},
};

pub const SECTOR_SIZE: usize = 0x930;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackType {
    Audio,
    Mode2352,
}

#[derive(PartialEq, Eq, Serialize, Deserialize, Debug, Clone)]
pub struct TrackIndex {
    pub id: u8,
    pub lba: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Track {
    pub id: u8,
    pub track_type: TrackType,
    pub indexes: Vec<TrackIndex>,
    pub filename: String,
    pub len: usize,
}

impl Track {
    pub fn get_start(&self) -> usize {
        let start = if self.indexes[0].id == 1 {
            self.indexes[0].lba
        } else {
            self.indexes[1].lba
        };
        start
    }
    pub fn get_start_mut(&mut self) -> &mut usize {
        if self.indexes[0].id == 1 {
            &mut self.indexes[0].lba
        } else {
            &mut self.indexes[1].lba
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Image {
    read_head: usize,
    // #[serde(skip)]
    // pub data: Box<[u8]>,
    pub tracks: Vec<Track>,
    // #[serde(skip)]
    // pub file: Option<File>,
    #[serde(skip)]
    pub cache: Option<CdCache>,
    pub serial: Option<String>,
}

impl Image {
    pub fn new(path: String) -> Self {
        // let path = f.unwrap();
        // let path = "/foo/psx/Spyro the Dragon (USA).bin";
        // let path = "/foo/psx/Silent Hill (USA).bin";
        // let path = "/foo/psx/celeste-collection.bin";
        // let path = "/foo/psx/CTR - Crash Team Racing (USA).bin";
        // let path = "/foo/psx/Crash Bandicoot (USA).bin";
        // let path = "/foo/psx/Metal Gear Solid (USA) (Disc 1).bin";
        // let path = "/foo/psx/Earthworm Jim 2 (Europe) (Track 01).bin";
        // let path = "/foo/psx/Mortal Kombat Trilogy (USA) (v1.1) (Track 01).bin";
        // let path = "/foo/psx/Final Fantasy VII (USA) (Disc 1).bin";
        // let path = "/foo/psx/Mega Man X4 (USA).bin";
        // let file = match File::open(path) {
        //     Ok(f) => f,
        //     Err(_) => panic!("can't read file")
        // };
        let (tracks, mut cache) = if path.ends_with(".bin") {
            let f = std::fs::File::open(&path).expect("ok");
            let len = f.metadata().expect("ok").len() as usize;
            let tracks = vec![Track {
                id: 1,
                track_type: TrackType::Mode2352,
                indexes: vec![TrackIndex { id: 1, lba: 0 }],
                filename: path.to_string(),
                len,
            }];

            (tracks.clone(), CdCache::new(tracks))
        } else if path.ends_with(".cue") {
            let mut tracks = crate::cue::parse_cue(&path);
            let mut prev_lba = 0;
            for t in &mut tracks {
                *t.get_start_mut() = prev_lba;
                prev_lba += t.len;
            }
            for t in &tracks {
                println!(">> track: {:?}", t);
            }
            let cache = CdCache::new(tracks.clone());
            (tracks, cache)
        } else {
            println!("don't know how to read : {}", path);
            std::process::exit(1);
        };
        // let mut data: Vec<u8> = vec![0u8; 2 * 75 * SECTOR_SIZE];
        // let mut data: Vec<u8> = vec![];
        // match file.read_to_end(&mut data) {
        //     Ok(_) => {},
        //     Err(_) => panic!("file read error")
        // };
        // file.read_to_end(&mut data);

        // Okay
        //
        // let mut iso = iso9660::open_image(&mut cache);

        // iso.entries.iter().for_each(|e| {
        //     println!(">>>>>> {} {}", String::from_utf8(e.name().to_vec()).expect("ok"), e.is_dir());
        // });

        // iso.ls();

        // println!(".....");
        let serial = extract_serial_number(&mut cache);

        let serial = serial.map(|x| String::from_utf8(x.0.to_vec()).expect("ok"));

        if let Some(serial) = &serial {
            println!("serial number: {}", serial);
        }

        // std::process::exit(0);

        //
        //
        Self {
            read_head: 0,
            // file: Some(file),
            cache: Some(cache),
            // data: data.into_boxed_slice(),
            tracks,
            serial,
        }
    }

    pub fn seek(&mut self, sectors: u32) {
        self.read_head = sectors as usize * SECTOR_SIZE;
    }
    pub fn seek_location(&mut self, mins: u8, secs: u8, sect: u8) {
        let sectors = ((mins as usize) * 75 * 60) + ((secs as usize) * 75) + (sect as usize);
        self.read_head = sectors * SECTOR_SIZE;
    }

    pub fn advance_sector(&mut self) -> Option<Vec<u8>> {
        let start = self.read_head;
        // println!("cdrom debug: advance_sector {}", start/SECTOR_SIZE);
        self.read_head += SECTOR_SIZE;
        if start < 2 * 75 * SECTOR_SIZE {
            Some(vec![0x0; SECTOR_SIZE])
        } else {
            self.cache
                .as_mut()
                .unwrap()
                .read_sector(start - 2 * 75 * SECTOR_SIZE)
                .map(|x| x.to_vec())
            // self.data[start..self.read_head].to_vec()
        }
    }

    pub fn current_sector(&mut self) -> Option<Vec<u8>> {
        let start = self.read_head;
        if start < 2 * 75 * SECTOR_SIZE {
            Some(vec![0x0; SECTOR_SIZE])
        } else {
            self.cache
                .as_mut()
                .unwrap()
                .read_sector(start - 2 * 75 * SECTOR_SIZE)
                .map(|x| x.to_vec())
        }
    }

    pub fn reset_read_head(&mut self) {
        self.read_head = SECTOR_SIZE * 75 * 2;
    }

    pub fn first_track_id(&self) -> u8 {
        self.tracks.first().expect("first track").id
    }
    pub fn last_track_id(&self) -> u8 {
        self.tracks.last().expect("last track").id
    }

    pub fn track_mm_ss_ff(&self, track_id: u8) -> (u8, u8, u8) {
        let track = &self.tracks[track_id as usize - 1];

        let start = if track.indexes[0].id == 1 {
            track.indexes[0].lba
        } else {
            track.indexes[1].lba
        };
        self.mm_ss_ff(start)
    }

    pub fn last_track_end(&self) -> (u8, u8, u8) {
        let last_track = self.tracks.last().unwrap();
        let track_idx = last_track.indexes.iter().find(|x| x.id != 0).unwrap();
        // should we add prelude?
        // let length = 2 * 75 * SECTOR_SIZE + last_track.len + track_idx.lba;
        let length = last_track.len + track_idx.lba;
        // let length = self.file.as_ref().unwrap().metadata().unwrap().len();
        self.mm_ss_ff(length as usize)
        // self.mm_ss_ff(self.data.len())
    }

    pub fn mm_ss_ff(&self, read_head: usize) -> (u8, u8, u8) {
        let sectors = read_head / SECTOR_SIZE;
        let secs = sectors / 75;
        let sect = sectors % 75;
        let mins = secs / 60;
        let secs = mins % 60;
        (mins as u8, secs as u8, sect as u8)
    }
    // INT3(amm,ass,asect,mode,file,channel,sm,ci)
    pub fn get_current_sector_info(&mut self) -> [u8; 8] {
        // let (amm, ass, asect) = self.mm_ss_ff(self.read_head);

        let sector = self.current_sector().expect("ok");

        let amm = sector[0xC];
        let ass = sector[0xD];
        let asect = sector[0xE];
        let sector_mode = sector[0xF];
        let file = sector[0x10];
        let channel = sector[0x11];
        let submode = sector[0x12];
        let ci = sector[0x13];
        // let is_realtime_audio = (submode & 0x44) == 0x44;
        [amm, ass, asect, sector_mode, file, channel, submode, ci]
    }

    pub fn current_track(&self) -> Track {
        let current_track = self
            .tracks
            .partition_point(|t| t.get_start() <= self.read_head)
            .saturating_sub(1);
        self.tracks[current_track].clone()
    }
    pub fn current_position_info(&self) -> [u8; 8] {
        let current_track = self
            .tracks
            .partition_point(|t| t.get_start() <= self.read_head)
            .saturating_sub(1);

        let current_track = &self.tracks[current_track];

        let current_index = if current_track.indexes[0].id == 1 {
            0
        } else {
            1
        };

        // let current_index = current_track
        //     .indexes
        //     .partition_point(|i| i.lba <= self.read_head)
        //     .saturating_sub(1);

        let current_index = &current_track.indexes[current_index];

        let track_pos = self.mm_ss_ff(self.read_head.saturating_sub(current_index.lba));
        let disk_pos = self.mm_ss_ff(self.read_head);

        [
            current_track.id,
            current_index.id,
            track_pos.0,
            track_pos.1,
            track_pos.2,
            disk_pos.0,
            disk_pos.1,
            disk_pos.2,
        ]
    }

    fn get_track_by_number(&self, track_num: u8) -> Option<&Track> {
        if track_num == 0 {
            self.tracks.last()
        } else {
            self.tracks.iter().find(|x| x.id == track_num)
        }
    }

    fn is_at_valid_location(&self) -> bool {
        // TODO: implement me
        true
    }
}

#[derive(Serialize, Deserialize)]
pub struct CDRom {
    addr: AddressReg,
    stat: StatusReg,
    mode: ModeReg,
    req: RequestReg,

    mute: bool,

    cmd: Option<u8>,
    cmd_state: CmdState,
    pending_cmd: Option<u8>,
    cmd_delay: u32,
    pending_params: ArrayVec<u8, 16>,
    cmd_params: ArrayVec<u8, 16>,
    results: ArrayVec<u8, 16>,

    pub disk: Option<Image>,
    pub seekloc: Option<u32>,
    data_buffer: VecDeque<u8>,
    data_pos: u32,
    last_sector_header: [u8; 8],
    read_state: ReadState,
    read_delay: u32,
    irq_mask: IrqMask,
    irq_pending: IrqPending,

    pending_volume: VolumeReg,
    volume: VolumeReg,
    // hinsts: HINTSTS,
    // audio_muted: bool,
    filter_file: u8,
    filter_channel: u8,
    audio_buffer: VecDeque<i16>,

    // Left, Right, Mono
    adpcm_history: [AdpcmHistory; 3],
    high_res_resamplers: [HighResResampler; 3],
    low_res_resamplers: [LowResResampler; 3],
}

impl CDRom {
    pub fn new(file: Option<String>) -> Self {
        let image = file.map(|path| Image::new(path));
        Self {
            addr: AddressReg(0x18),
            stat: StatusReg(0),
            mode: ModeReg::default(),
            req: RequestReg::default(),

            mute: false,
            cmd: None,
            cmd_state: CmdState::RecvCmd,
            pending_cmd: None,
            cmd_delay: 0,
            pending_params: ArrayVec::default(),
            cmd_params: ArrayVec::default(),
            results: ArrayVec::default(),

            // hinsts: HINTSTS::default(),
            disk: image,
            seekloc: None,
            data_buffer: VecDeque::default(),
            data_pos: 0,
            last_sector_header: [0; 8],
            read_state: ReadState::Idle,
            read_delay: 0,
            irq_mask: IrqMask(0),
            irq_pending: IrqPending(0),

            pending_volume: VolumeReg {
                l2l_volume: 0x80,
                l2r_volume: 0x0,
                r2l_volume: 0x80,
                r2r_volume: 0x0,
            },
            volume: VolumeReg {
                l2l_volume: 0x80,
                l2r_volume: 0x0,
                r2l_volume: 0x80,
                r2r_volume: 0x0,
            },

            filter_file: 0,
            filter_channel: 0,
            audio_buffer: VecDeque::default(),
            adpcm_history: Default::default(),
            high_res_resamplers: Default::default(),
            low_res_resamplers: Default::default(),
        }
    }

    // ---------------------- Regs IO --------------------------------------------------
    pub fn write_request(&mut self, v: u8) {
        self.req.0 = v;

        if !self.req.want_data() {
            self.data_pos = 0;
        }
    }

    pub fn read_addr(&mut self) -> u8 {
        let addr = &mut self.addr;
        addr.set_param_empty(self.pending_params.is_empty());
        addr.set_param_write_ready(!self.pending_params.is_full());
        addr.set_result_read_ready(!self.results.is_empty());
        addr.set_data_request(self.req.want_data() && !self.data_buffer.is_empty());
        addr.set_busy_status(self.cmd_state != CmdState::RecvCmd);
        addr.0
    }

    pub fn load<T: Addressable>(&mut self, offset: u32) -> T {
        let width = T::width() as usize;
        // println!("cdrom load {}",offset);
        if offset == 2 {
            return self.read_sector_data::<T>();
        }
        if width != 1 {
            println!(
                "cdrom load width other than 1 byte not impl, got: {:?}",
                T::width()
            );
        }
        let result = match offset {
            0 => self.read_addr(),
            1 => self.consume_result_byte(),
            3 => match self.addr.current_bank() {
                0 | 2 => self.irq_mask.0,
                1 | 3 => self.irq_pending.0 | 0xe0, // bits 5-7 are always 1
                _ => unreachable!(""),
            },
            _ => unreachable!(""),
        };
        T::from_u32(result as u32)
    }

    pub fn write_param(&mut self, param: u8) {
        self.pending_params.push(param);
    }

    pub fn ack_interrupt(system: &mut System, v: u8) {
        let cdrom = &mut system.cdrom;
        let ack = AckInterrupt(v);

        cdrom.irq_pending.0 &= !ack.0;
        // let ints = cdrom.irq_pending.ints();
        // println!("ack: int clear: 0b{:b}", ack.int_clear());
        // irq
        // cdrom.irq_pending.set_ints(ints & !ack.int_clear());
        // cdrom.irq_pending.set_ints(ints & !ack.int_clear());
        // println!("irq_pending: 0b{:0}", cdrom.irq_pending.ints());
        // cdrom.irq_pending.set_ints(0);

        if ack.param_clear() {
            cdrom.pending_params.clear();
        }

        if cdrom.irq_pending.ints() == 0 && cdrom.cmd.is_none() && cdrom.pending_cmd.is_some() {
            let pending_cmd = cdrom.pending_cmd.take().unwrap();
            cdrom.results.clear();
            Self::begin_command(system, pending_cmd);
        }
    }

    pub fn write_adpcm_ctrl(&mut self, v: u8) {
        let ctrl = ADPCTL(v);
        if ctrl.chngatv() {
            self.volume = self.pending_volume.clone()
        }
    }

    pub fn store<T: Addressable>(system: &mut System, offset: u32, value: T) {
        let cdrom = &mut system.cdrom;
        let width = T::width() as usize;
        if width != 1 {
            panic!(
                "cdrom store width other than 1 byte not impl, got: {:?}",
                T::width()
            );
        }
        let v = value.as_u32() as u8;
        let bank_index = cdrom.addr.current_bank();

        match offset {
            0 => cdrom.addr.set_current_bank(v),
            1 => match bank_index {
                0 => Self::write_command(system, v),
                1 => {
                    println!("WRDATA not implemented");
                } // wrdata
                2 => {
                    println!("CI not implemented!")
                }
                3 => cdrom.pending_volume.r2r_volume = v, // FIXME: volume pending?
                _ => unreachable!(),
            },
            2 => match bank_index {
                0 => cdrom.write_param(v),
                1 => {
                    // println!("cdrom debug: set int enable: {}", v);
                    cdrom.irq_mask.0 = v;
                    // cdrom.irq_mask = cdrom.irq_mask.set_int_enable(v);
                    // println!("irq mask: 0b{:b}", cdrom.irq_mask.int_enable());
                }
                2 => cdrom.pending_volume.l2l_volume = v,
                3 => cdrom.pending_volume.r2l_volume = v,
                _ => unreachable!(),
            },
            3 => match bank_index {
                0 => cdrom.write_request(v),
                1 => Self::ack_interrupt(system, v),
                2 => cdrom.pending_volume.l2r_volume = v,
                3 => cdrom.write_adpcm_ctrl(v),
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
    }

    pub fn write_command(system: &mut System, opcode: u8) {
        let cdrom = &mut system.cdrom;
        if cdrom.cmd.is_some() && opcode != 0x09 {
            let active_cmd = cdrom.cmd.unwrap();
            println!(
                "warn, new command {:X} when another in progress ({:X})",
                opcode, active_cmd
            );
        }
        if cdrom.cmd.is_none() && cdrom.irq_pending.ints() == 0 {
            Self::begin_command(system, opcode);
        } else {
            system.cdrom.pending_cmd = Some(opcode);
        }
    }

    pub fn begin_command(system: &mut System, opcode: u8) {
        let cdrom = &mut system.cdrom;
        cdrom.cmd = Some(opcode);
        cdrom.cmd_state = CmdState::RecvCmd;
        cdrom.cmd_params = cdrom.pending_params.clone();
        cdrom.pending_params.clear();
        Self::step_command(system);
    }

    pub fn finish_command(&mut self) {
        self.cmd_params.clear();
        self.cmd_state = CmdState::RecvCmd;
        self.stat.set_error(false);
        self.cmd = None;
    }

    pub fn step_command(system: &mut System) {
        let cdrom = &mut system.cdrom;
        let Some(cmd) = cdrom.cmd else {
            return;
        };
        cdrom.results.clear();
        // println!(">>Unhandled command?: 0x{:x}", cmd);

        match cmd {
            // 0x00 => self.cmd_unused(),
            0x01 => Self::cmd_nop(system),
            0x02 => Self::cmd_setloc(system),
            0x03 => Self::cmd_play(system), // cmd play
            0x06 => Self::cmd_readn(system),
            0x08 => Self::cmd_stop(system),
            0x09 => Self::cmd_pause(system),
            0x0a => Self::cmd_init(system),
            0x0b => Self::cmd_mute(system),
            0x0c => Self::cmd_demute(system),
            0x0d => Self::cmd_set_filter(system),
            0x0e => Self::cmd_setmode(system),
            0x10 => Self::cmd_get_locl(system),
            0x11 => Self::cmd_get_locp(system),
            0x13 => Self::cmd_gettn(system),
            0x14 => Self::cmd_gettd(system),
            0x15 => Self::cmd_seekl(system),
            0x16 => Self::cmd_seekl(system), // seekp
            0x19 => Self::cmd_test(system),
            0x1a => Self::cmd_getid(system),
            0x1b => Self::cmd_readn(system), // TODO: should be reads
            0x1e => Self::cmd_readtoc(system), // TODO: should be reads

            _ => panic!("Unhandled command: 0x{:x}", cmd),
        };
    }

    // pub fn cmd_unused(&mut self) -> CommandResponse {
    //     // self.set_int(5);
    //     // self.response_idx = 2;
    //     // self.response_buffer[0] = 0x11;
    //     // self.response_buffer[1] = 0x40;
    //     // self.response_read_idx = 0;
    //     // self.hsts.set_result_read_ready(true);
    //     CommandResponse::new().int5([0x11, 0x40], AVG_1ST_RESP_GENERIC)
    // }

    // ---------------------------- Commands --------------------------------------=

    pub fn push_result_byte(&mut self, byte: u8) {
        if self.results.is_full() {
            println!("cdrom warn: fifo is full");
            self.results.pop();
        }
        self.results.push(byte);
    }

    pub fn push_result_slice(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.push_result_byte(*b);
        }
    }

    pub fn set_interrupt(system: &mut System, it: u8) {
        let cdrom = &mut system.cdrom;
        if cdrom.irq_pending.ints() != 0 {
            println!(
                "cdrom warn: unacknowledged interrupt: {:X} -> {:X}",
                cdrom.irq_pending.ints(),
                it
            );
        }
        cdrom.irq_pending.0 = it;
        // cdrom.irq_pending.set_ints(it);

        // let pending = cdrom.irq_pending.ints();
        // let mask = cdrom.irq_mask.int_enable();
        // println!("pending: {:b}, mask: {:b}", pending, mask);
        if cdrom.irq_pending.0 & cdrom.irq_mask.0 != 0 {
            // println!("setting irq ({})", it);
            system.irqctl.set_cdrom();
        }
    }

    pub fn push_error(&mut self, err: DriveError) {
        let mut stat = self.stat;
        stat.set_error(true);
        stat.set_seek_error(err == DriveError::SeekFailed);
        self.push_result_byte(stat.0);
        self.push_result_byte(err.code());
    }

    pub fn assert_param_count(system: &mut System, cmd_name: &str, expected: usize) -> bool {
        if system.cdrom.cmd_params.len() == expected {
            return true;
        }
        println!(
            "{}: wrong parameter count: expectd {}, got {}",
            cmd_name,
            expected,
            system.cdrom.cmd_params.len()
        );
        system.cdrom.push_error(DriveError::WrongParameterCount);
        Self::set_interrupt(system, 5);
        system.cdrom.finish_command();
        return false;
    }

    // or get_stat
    pub fn cmd_nop(system: &mut System) {
        let cdrom = &mut system.cdrom;
        match cdrom.cmd_state {
            CmdState::RecvCmd => {
                cdrom.cmd_delay = CDROM_AVG_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp1;
            }
            CmdState::Resp1 => {
                let stat = cdrom.stat;
                cdrom.stat.set_shell_open(false);
                cdrom.push_result_byte(stat.0);

                println!("cdrom debug: cmd get_stat()={:?}", stat);
                Self::set_interrupt(system, 3);
                let cdrom = &mut system.cdrom;
                cdrom.finish_command();
            }
            _ => unreachable!(),
        }
        // nop, clears tray open bit, response: INT3: status
        // if !self.parameters.is_empty() {
        //     return error_response(&self.stat, 0x20, "nop takes no params");
        // }
        // CommandResponse::new().int3([self.stat.0], AVG_1ST_RESP_GENERIC)
    }

    pub fn cmd_setloc(system: &mut System) {
        let cdrom = &mut system.cdrom;
        match cdrom.cmd_state {
            CmdState::RecvCmd => {
                cdrom.cmd_delay = CDROM_AVG_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp1;
            }
            CmdState::Resp1 => {
                if !Self::assert_param_count(system, "cmd_setloc", 3) {
                    return;
                }
                let cdrom = &mut system.cdrom;
                let ff = from_bcd(cdrom.cmd_params.pop().unwrap()).expect("ok");
                let ss = from_bcd(cdrom.cmd_params.pop().unwrap()).expect("ok");
                let mm = from_bcd(cdrom.cmd_params.pop().unwrap()).expect("ok");
                println!("cdrom debug: setloc amm: {mm} ass: {ss} asect: {ff}");

                cdrom.seekloc = Some(pos_to_sectors(mm, ss, ff));
                cdrom.push_result_byte(cdrom.stat.0);
                Self::set_interrupt(system, 3);
                system.cdrom.finish_command();
            }
            _ => unreachable!(),
        }
    }

    pub fn cmd_test(system: &mut System) {
        let cdrom = &mut system.cdrom;
        match cdrom.cmd_state {
            CmdState::RecvCmd => {
                cdrom.cmd_delay = CDROM_AVG_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp1;
            }
            CmdState::Resp1 => {
                if !Self::assert_param_count(system, "cmd_test", 1) {
                    return;
                }
                let cdrom = &mut system.cdrom;
                let subcmd = cdrom.cmd_params.pop().unwrap();
                match subcmd {
                    0x20 => {
                        // INT3(yy,mm,dd,ver) ;Get cdrom BIOS date/version (yy,mm,dd,ver) INT3(yy,mm,dd,ver) ;Get cdrom BIOS date/version (yy,mm,dd,ver)
                        println!("cdrom get version");
                        cdrom.push_result_slice(&CDROM_DATE_VERSION);
                        Self::set_interrupt(system, 3);
                        system.cdrom.finish_command();
                        // CommandResponse::new().int3([149, 5, 22, 193], AVG_1ST_RESP_GENERIC)
                    }
                    _ => panic!("Unhandled 0x19 (test) subcmd: 0x{:x}", subcmd),
                }
            }
            _ => unreachable!(),
        }
    }

    pub fn cmd_seekl(system: &mut System) {
        let cdrom = &mut system.cdrom;
        match cdrom.cmd_state {
            CmdState::RecvCmd => {
                cdrom.cmd_delay = CDROM_AVG_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp1;
                cdrom.stat.set_seek(true);
                cdrom.stat.set_spindle_motor(true);
            }
            CmdState::Resp1 => {
                let Some(loc) = cdrom.seekloc else {
                    panic!("no seekloc!")
                };
                println!("cdrom debug seekl: loc={}", loc);
                let cdrom = &mut system.cdrom;
                cdrom.push_result_byte(cdrom.stat.0);
                cdrom.cmd_delay = CDROM_SEEKL_DELAY_CYCLES;
                Self::set_interrupt(system, 3);
                system.cdrom.cmd_state = CmdState::Resp2;
            }
            CmdState::Resp2 => {
                let disk = cdrom.disk.as_mut().unwrap();
                disk.seek(cdrom.seekloc.unwrap());
                cdrom.seekloc = None;
                cdrom.push_result_byte(cdrom.stat.0);
                cdrom.stat.set_seek(false);
                Self::set_interrupt(system, 2);
                system.cdrom.finish_command();
            }
        }
    }
    // pub fn cmd_seekl(&mut self) -> CommandResponse {
    //
    //     let mut seeking = self.stat.clone();
    //     seeking.set_seek(true);
    //     self.stat.set_seek(false);
    //     println!("CDROM seekl");
    //     CommandResponse::new()
    //         .int3([seeking.0], AVG_1ST_RESP_GENERIC)
    //         .int2([self.stat.0], AVG_1ST_RESP_GENERIC + AVG_2ND_RESP_SEEKL)
    // }

    // pub fn cmd_setmode(&mut self) -> CommandResponse {
    //     if self.parameters.len() != 1 {
    //         return error_response(&self.stat, 0x20, "setmode takes 1 params");
    //     }
    //     let mode = self.parameters[0];
    //
    //     self.mode.0 = mode;
    //
    //     println!("CDROM setmode 0x{:X}, {:?}", mode, self.mode);
    //
    //     CommandResponse::new().int3([self.stat.0], AVG_1ST_RESP_GENERIC)
    // }

    pub fn cmd_setmode(system: &mut System) {
        let cdrom = &mut system.cdrom;
        match cdrom.cmd_state {
            CmdState::RecvCmd => {
                cdrom.cmd_delay = CDROM_AVG_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp1;
            }
            CmdState::Resp1 => {
                if !Self::assert_param_count(system, "cmd_setmode", 1) {
                    return;
                }
                let cdrom = &mut system.cdrom;
                let mode_byte = cdrom.cmd_params.pop().unwrap();
                cdrom.mode.0 = mode_byte;
                println!("cdrom debug: setMode mode={:?}", cdrom.mode);
                cdrom.push_result_byte(cdrom.stat.0);
                if cdrom.mode.auto_pause() {
                    println!("cdrom WARN: auto pause not implemented");
                }
                // TODO: xa set mode...
                Self::set_interrupt(system, 3);
                system.cdrom.finish_command();
            }
            _ => unreachable!(),
        }
    }
    pub fn cmd_set_filter(system: &mut System) {
        let cdrom = &mut system.cdrom;
        match cdrom.cmd_state {
            CmdState::RecvCmd => {
                cdrom.cmd_delay = CDROM_AVG_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp1;
            }
            CmdState::Resp1 => {
                if !Self::assert_param_count(system, "cmd_set_filter", 2) {
                    return;
                }
                let cdrom = &mut system.cdrom;
                let channel = cdrom.cmd_params.pop().unwrap();
                let file = cdrom.cmd_params.pop().unwrap();

                println!("DEBUG CDROM: setfilter 0x{:X}, 0x{:X}", file, channel);
                cdrom.filter_file = file;
                cdrom.filter_channel = channel;
                cdrom.push_result_byte(cdrom.stat.0);
                Self::set_interrupt(system, 3);
                system.cdrom.finish_command();
            }
            _ => unreachable!(),
        }
    }
    // pub fn cmd_set_filter(&mut self) -> CommandResponse {
    //     if self.parameters.len() != 2 {
    //         return error_response(&self.stat, 0x20, "setfilter takes 2 params");
    //     }
    //     let file = self.parameters[0];
    //     let channel = self.parameters[1];
    //
    //     self.filter_file = file;
    //     self.filter_channel = channel;
    //
    //     println!("CDROM setfilter 0x{:X}, 0x{:X}", file, channel);
    //
    //     CommandResponse::new().int3([self.stat.0], AVG_1ST_RESP_GENERIC)
    // }

    pub fn cmd_get_locp(system: &mut System) {
        let cdrom = &mut system.cdrom;
        match cdrom.cmd_state {
            CmdState::RecvCmd => {
                cdrom.cmd_delay = CDROM_AVG_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp1;
            }
            CmdState::Resp1 => {
                let cdrom = &mut system.cdrom;
                let Some(disk) = cdrom.disk.as_ref() else {
                    cdrom.push_error(DriveError::CannotRespond);
                    println!("warn locp while no disk");
                    Self::set_interrupt(system, 5);
                    system.cdrom.finish_command();
                    return;
                };

                let bytes = disk.current_position_info();
                cdrom.push_result_slice(&bytes);

                println!("DEBUG CDROM: get_locp ");
                Self::set_interrupt(system, 3);
                system.cdrom.finish_command();
            }
            _ => unreachable!(),
        }
    }
    // pub fn get_locp(&mut self) -> CommandResponse {
    //     if !self.parameters.is_empty() {
    //         return error_response(&self.stat, 0x20, "get_locp doesn't takes parameters");
    //     }
    //     println!("CDROM get_locp!!");
    //
    //     let disk = self.disk.as_ref().expect("get_locp inserted disk");
    //
    //     CommandResponse::new().int3(
    //         disk.current_position_info()
    //             .map(|x| to_bcd(x).expect("track position is valid bcd")),
    //         AVG_1ST_RESP_GENERIC,
    //     )
    // }

    pub fn cmd_get_locl(system: &mut System) {
        let cdrom = &mut system.cdrom;
        match cdrom.cmd_state {
            CmdState::RecvCmd => {
                cdrom.cmd_delay = CDROM_AVG_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp1;
            }
            CmdState::Resp1 => {
                let cdrom = &mut system.cdrom;
                if cdrom.read_state != ReadState::Reading {
                    println!("error: getlocl while not reading..");
                    cdrom.push_error(DriveError::CannotRespond);
                    Self::set_interrupt(system, 5);
                    system.cdrom.finish_command();
                    return;
                };

                println!("DEBUG CDROM: get_locl ");
                let header = cdrom.last_sector_header;
                cdrom.push_result_slice(&header);
                Self::set_interrupt(system, 3);
                system.cdrom.finish_command();
            }
            _ => unreachable!(),
        }
    }
    // Retrieves 4-byte sector header, plus 4-byte subheader of the current sector
    // pub fn get_locl(&mut self) -> CommandResponse {
    //     if !self.parameters.is_empty() {
    //         return error_response(&self.stat, 0x20, "get_locp doesn't takes parameters");
    //     }
    //     println!("CDROM get_locl!!");
    //
    //     let disk = self.disk.as_mut().expect("get_locl inserted disk");
    //
    //     CommandResponse::new().int3(
    //         disk.get_current_sector_info(),
    //         // .map(|x| to_bcd(x).expect("track position is valid bcd")),
    //         0xAAA401,
    //     )
    // }

    pub fn cmd_readn(system: &mut System) {
        let cdrom = &mut system.cdrom;
        match cdrom.cmd_state {
            CmdState::RecvCmd => {
                cdrom.cmd_delay = CDROM_AVG_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp1;
                cdrom.stat.set_spindle_motor(true);
            }
            CmdState::Resp1 => {
                let cdrom = &mut system.cdrom;
                println!("DEBUG CDROM: cmd_readn ");
                cdrom.begin_reading();

                cdrom.push_result_byte(cdrom.stat.0);
                Self::set_interrupt(system, 3);
                system.cdrom.finish_command();
            }
            _ => unreachable!(),
        }
    }
    pub fn cmd_play(system: &mut System) {
        let cdrom = &mut system.cdrom;
        match cdrom.cmd_state {
            CmdState::RecvCmd => {
                cdrom.cmd_delay = CDROM_AVG_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp1;
                cdrom.stat.set_spindle_motor(true);
            }
            CmdState::Resp1 => {
                let cdrom = &mut system.cdrom;
                println!("DEBUG CDROM: cmd_play ");
                let track_num = cdrom.cmd_params.pop();
                if let Some(track_id) = track_num {
                    let track_num = from_bcd(track_id).unwrap();
                    let track = cdrom.disk.as_ref().unwrap().get_track_by_number(track_num).expect("ok");
                    cdrom.seekloc = Some(track.get_start() as u32);
                }
                let cdrom = &mut system.cdrom;
                cdrom.begin_playing();

                cdrom.push_result_byte(cdrom.stat.0);
                Self::set_interrupt(system, 3);
                system.cdrom.finish_command();
            }
            _ => unreachable!(),
        }
    }
    // pub fn cmd_readn(&mut self) -> CommandResponse {
    //     if !self.parameters.is_empty() {
    //         return error_response(&self.stat, 0x20, "readn doesn't takes parameters");
    //     }
    //     println!("CDROM readn!!");
    //
    //     self.stat.set_read(true);
    //     CommandResponse::new()
    //         .int3([self.stat.0], AVG_1ST_RESP_GENERIC)
    //         .int1(AVG_1ST_RESP_GENERIC + self.mode.speed().transform(AVG_RATE_INT1))
    // }

    // pub fn cmd_play(&mut self) -> CommandResponse {
    //     if !self.parameters.is_empty() {
    //         let track = from_bcd(self.parameters[0]).expect("ok");
    //         println!("play set loc");
    //     };
    //
    //     // TODO: play not implemented
    //
    //     println!("CDROM play!!");
    //
    //     self.stat.set_read(true);
    //     self.stat.set_play(true);
    //     CommandResponse::new().int3([self.stat.0], AVG_1ST_RESP_GENERIC)
    //     // .int1( AVG_1ST_RESP_GENERIC + self.mode.speed().transform(AVG_RATE_INT1))
    // }

    // pub fn cmd_reads(&mut self) -> CommandResponse {
    //     if !self.parameters.is_empty() {
    //         return error_response(&self.stat, 0x20, "reads doesn't takes parameters");
    //     }
    //     println!("CDROM reads!!");
    //
    //     self.stat.set_read(true);
    //     CommandResponse::new()
    //         .int3([self.stat.0], AVG_1ST_RESP_GENERIC)
    //         .int1(AVG_1ST_RESP_GENERIC + self.mode.speed().transform(AVG_RATE_INT1))
    // }

    pub fn cmd_demute(system: &mut System) {
        let cdrom = &mut system.cdrom;
        match cdrom.cmd_state {
            CmdState::RecvCmd => {
                cdrom.cmd_delay = CDROM_AVG_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp1;
            }
            CmdState::Resp1 => {
                let cdrom = &mut system.cdrom;
                println!("DEBUG CDROM: cmd_demute");
                cdrom.mute = false;

                cdrom.push_result_byte(cdrom.stat.0);
                Self::set_interrupt(system, 3);
                system.cdrom.finish_command();
            }
            _ => unreachable!(),
        }
    }
    pub fn cmd_mute(system: &mut System) {
        let cdrom = &mut system.cdrom;
        match cdrom.cmd_state {
            CmdState::RecvCmd => {
                cdrom.cmd_delay = CDROM_AVG_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp1;
            }
            CmdState::Resp1 => {
                let cdrom = &mut system.cdrom;
                println!("DEBUG CDROM: cmd_mute");
                cdrom.mute = true;

                cdrom.push_result_byte(cdrom.stat.0);
                Self::set_interrupt(system, 3);
                system.cdrom.finish_command();
            }
            _ => unreachable!(),
        }
    }
    // pub fn cmd_demute(&mut self) -> CommandResponse {
    //     if !self.parameters.is_empty() {
    //         return error_response(&self.stat, 0x20, "pause doesn't takes parameters");
    //     }
    //     // turn on audio streaming to spu
    //     println!("CDROM demute");
    //     self.audio_muted = false;
    //
    //     CommandResponse::new().int3([self.stat.0], AVG_1ST_RESP_GENERIC)
    // }
    pub fn cmd_pause(system: &mut System) {
        let cdrom = &mut system.cdrom;
        match cdrom.cmd_state {
            CmdState::RecvCmd => {
                cdrom.cmd_delay = CDROM_AVG_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp1;
            }
            CmdState::Resp1 => {
                let cdrom = &mut system.cdrom;
                println!("DEBUG CDROM: pause");
                cdrom.push_result_byte(cdrom.stat.0);
                cdrom.cmd_delay = match cdrom.mode.speed() {
                    Speed::Normal => CDROM_PAUSE_DELAY_CYCLES,
                    Speed::Double => CDROM_PAUSE_2X_DELAY_CYCLES,
                };
                cdrom.cmd_state = CmdState::Resp2;

                Self::set_interrupt(system, 3);
            }
            CmdState::Resp2 => {
                cdrom.push_result_byte(cdrom.stat.0);
                cdrom.reset_read_state();
                Self::set_interrupt(system, 2);
                system.cdrom.finish_command();
            }
        }
    }
    // pub fn cmd_pause(&mut self) -> CommandResponse {
    //     if !self.parameters.is_empty() {
    //         return error_response(&self.stat, 0x20, "pause doesn't takes parameters");
    //     }
    //     println!("CDROM pause!!");
    //
    //     let current = self.stat.clone();
    //     self.stat.set_read(false);
    //     CommandResponse::new()
    //         .int3([current.0], AVG_1ST_RESP_GENERIC)
    //         .int2([self.stat.0], AVG_1ST_RESP_GENERIC + AVG_2ND_RESP_PAUSE)
    // }

    pub fn cmd_getid(system: &mut System) {
        let cdrom = &mut system.cdrom;
        match cdrom.cmd_state {
            CmdState::RecvCmd => {
                cdrom.cmd_delay = CDROM_AVG_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp1;
            }
            CmdState::Resp1 => {
                let cdrom = &mut system.cdrom;
                cdrom.cmd_delay = CDROM_AVG_DELAY_CYCLES;
                println!("DEBUG CDROM: getid");
                cdrom.push_result_byte(cdrom.stat.0);
                Self::set_interrupt(system, 3);
                system.cdrom.cmd_state = CmdState::Resp2;
            }
            CmdState::Resp2 => {
                if cdrom.disk.is_some() {
                    // println!(">>got licensed");
                    cdrom.push_result_slice(&CDROM_GETID_LICENSED);
                    Self::set_interrupt(system, 2);
                } else {
                    cdrom.push_result_slice(&CDROM_GETID_NODISK);
                    Self::set_interrupt(system, 5);
                }
                system.cdrom.finish_command();
            }
        }
    }
    // pub fn cmd_getid(&mut self) -> CommandResponse {
    //     if !self.parameters.is_empty() {
    //         println!("WARN: getid extra params");
    //         return error_response(&self.stat, 0x20, "get id takes no params");
    //     }
    //     println!("CDROM get id");
    //     if self.disk.is_none() {
    //         CommandResponse::new()
    //             .int3([self.stat.0], AVG_1ST_RESP_GENERIC)
    //             .int5([0x8, 0x40], 0x4A00)
    //         // self.pending_interrupt = Some((0x4A00, 0x5, data, 8)); // it works!!!! (no-cd)
    //     } else {
    //         CommandResponse::new()
    //             .int3([self.stat.0], AVG_1ST_RESP_GENERIC)
    //             .int2(
    //                 [0x02, 0x00, 0x20, 0x00, b'S', b'C', b'E', b'A'],
    //                 AVG_1ST_RESP_GENERIC + AVG_2ND_RESP_GET_ID,
    //             )
    //     }
    // self.set_int(3);
    // self.push_status();
    // then we need to schedule response...
    // let mut data = [0;16];
    // data[0] = 0x8;
    // data[1] = 0x40;
    // self.pending_interrupt = Some((0x4A00, 0x5, data, 8)); // it works!!!! (no-cd)

    // let  data = [0x02u8,0x00, 0x20,0x00, 0x53,0x43,0x45,0x41,0,0,0,0,0,0,0,0]; // na
    // let  data = [0x02u8,0x00, 0x20,0x00, 0x53,0x43,0x45,0x45,0,0,0,0,0,0,0,0]; //eu
    //
    // self.pending_interrupt = Some((0x4A00, 0x3, data, 8));
    // }

    pub fn cmd_init(system: &mut System) {
        let cdrom = &mut system.cdrom;
        match cdrom.cmd_state {
            CmdState::RecvCmd => {
                cdrom.cmd_delay = CDROM_INIT_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp1;
            }
            CmdState::Resp1 => {
                println!("DEBUG CDROM: cmd_init");
                cdrom.push_result_byte(cdrom.stat.0);
                cdrom.cmd_delay = CDROM_INIT_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp2;
                Self::set_interrupt(system, 3);
            }
            CmdState::Resp2 => {
                cdrom.stat.set_spindle_motor(true);
                cdrom.mode.0 = 0x20;
                cdrom.reset_read_state();
                cdrom.push_result_byte(cdrom.stat.0);
                Self::set_interrupt(system, 2);
                system.cdrom.finish_command();
            }
        }
    }
    pub fn cmd_readtoc(system: &mut System) {
        let cdrom = &mut system.cdrom;
        match cdrom.cmd_state {
            CmdState::RecvCmd => {
                cdrom.cmd_delay = CDROM_INIT_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp1;
            }
            CmdState::Resp1 => {
                println!("DEBUG CDROM: cmd_readtoc");
                cdrom.push_result_byte(cdrom.stat.0);
                cdrom.cmd_delay = 5*CDROM_INIT_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp2;
                Self::set_interrupt(system, 3);
            }
            CmdState::Resp2 => {
                cdrom.push_result_byte(cdrom.stat.0);
                Self::set_interrupt(system, 2);
                system.cdrom.finish_command();
            }
        }
    }
    // pub fn cmd_init(&mut self) -> CommandResponse {
    //     if !self.parameters.is_empty() {
    //         return error_response(&self.stat, 0x20, "init takes no params");
    //     }
    //     println!("CDROM init");
    //     // Sets mode=20h, activates drive motor, Standby, abort all commands.
    //     self.mode.0 = 0x20;
    //
    //     if let Some(disk) = self.disk.as_mut() {
    //         disk.reset_read_head();
    //     }
    //
    //     // reset read head?
    //
    //     let old_status = self.stat.clone();
    //     self.stat.set_spindle_motor(true);
    //     CommandResponse::new()
    //         .int3([old_status.0], AVG_1ST_RESP_GENERIC)
    //         .int2([self.stat.0], AVG_1ST_RESP_GENERIC + AVG_2ND_RESP_SEEKL)
    // }
    //
    pub fn cmd_gettn(system: &mut System) {
        let cdrom = &mut system.cdrom;
        // cdrom.cmd_delay = CDROM_AVG_DELAY_CYCLES;
        let Some(disk) = cdrom.disk.as_ref() else {
            cdrom.push_error(DriveError::CannotRespond);
            println!("warn gettn while no disk");
            Self::set_interrupt(system, 5);
            system.cdrom.finish_command();
            return;
        };
        let first_track = to_bcd(disk.first_track_id()).expect("valid bcd");
        let last_track = to_bcd(disk.last_track_id()).expect("valid bcd");
        cdrom.push_result_byte(cdrom.stat.0);
        cdrom.push_result_byte(first_track);
        cdrom.push_result_byte(last_track);
        println!(
            "cdrom debug gettn, first: {}, last: {}",
            first_track, last_track
        );
        Self::set_interrupt(system, 3);
        system.cdrom.finish_command();
    }
    // pub fn cmd_gettn(&mut self) -> CommandResponse {
    //     // int3 bcd
    //     println!("cdrom gettn");
    //     if !self.parameters.is_empty() {
    //         return error_response(&self.stat, 0x20, "gettn takes no params");
    //     }
    //     let disk = self.disk.as_ref().expect("gettn inserted disk");
    //
    //     let first_track = to_bcd(disk.first_track_id()).expect("valid bcd");
    //     let last_track = to_bcd(disk.last_track_id()).expect("valid bcd");
    //     CommandResponse::new().int3([self.stat.0, first_track, last_track], AVG_1ST_RESP_INIT)
    // }
    // pub fn cmd_gettd(&mut self) -> CommandResponse {
    //     // int3 bcd
    //     println!("cdrom gettd");
    //     if self.parameters.len() != 1 {
    //         return error_response(&self.stat, 0x20, "gettd takes 1 param");
    //     }
    //     let disk = self.disk.as_ref().expect("gettd inserted disk");
    //     let last_track = disk.last_track_id();
    //
    //     let Some(track) = from_bcd(self.parameters[0]).filter(|&x| x <= last_track) else {
    //         return error_response(&self.stat, 0x10, "gettd wrong track bcd");
    //     };
    //
    //     let (mm, ss, _) = if track != 0 {
    //         disk.track_mm_ss_ff(track)
    //     } else {
    //         disk.last_track_end()
    //     };
    //     // error_response(&self.status, 0x10, "gettd not impl")
    //     CommandResponse::new().int3(
    //         [
    //             self.stat.0,
    //             to_bcd(mm).expect("bcd"),
    //             to_bcd(ss).expect("bcd"),
    //         ],
    //         AVG_1ST_RESP_INIT,
    //     )
    //     // unimplemented!()
    // }

    pub fn cmd_gettd(system: &mut System) {
        let cdrom = &mut system.cdrom;
        match cdrom.cmd_state {
            CmdState::RecvCmd => {
                cdrom.cmd_delay = CDROM_AVG_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp1;
            }
            CmdState::Resp1 => {
                let track_num = from_bcd(cdrom.cmd_params.pop().unwrap_or(0)).unwrap_or(0);
                println!("DEBUG CDROM: get_td (track_num: {})", track_num);
                let Some(disk) = cdrom.disk.as_ref() else {
                    cdrom.push_error(DriveError::CannotRespond);
                    println!("warn gettd while no disk");
                    Self::set_interrupt(system, 5);
                    system.cdrom.finish_command();
                    return;
                };

                let Some(track) = disk.get_track_by_number(track_num) else {
                    cdrom.push_error(DriveError::InvalidParameter);
                    println!("warn gettd invalid track number: {}", track_num);
                    Self::set_interrupt(system, 5);
                    system.cdrom.finish_command();
                    return;
                };

                let (mm, ss, _) = if track_num != 0 {
                    let start = track.get_start();
                    disk.mm_ss_ff(start)
                } else {
                    disk.last_track_end()
                };

                println!("DEBUG CDROM: get_td reporting mm: {} ss:{}", mm, ss);
                cdrom.push_result_byte(cdrom.stat.0);
                cdrom.push_result_byte(to_bcd(mm).expect("ok"));
                cdrom.push_result_byte(to_bcd(ss).expect("ok"));

                Self::set_interrupt(system, 3);
                system.cdrom.finish_command();
            }
            _ => unreachable!(),
        }
    }

    pub fn cmd_stop(system: &mut System) {
        let cdrom = &mut system.cdrom;
        match cdrom.cmd_state {
            CmdState::RecvCmd => {
                cdrom.cmd_delay = CDROM_AVG_DELAY_CYCLES;
                cdrom.cmd_state = CmdState::Resp1;
            }
            CmdState::Resp1 => {
                println!("DEBUG CDROM: cmd_stop");
                cdrom.push_result_byte(cdrom.stat.0);
                cdrom.stat.set_read(false);
                cdrom.stat.set_play(false);
                cdrom.audio_buffer.clear();
                cdrom.cmd_delay = CDROM_AVG_DELAY_CYCLES;
                Self::set_interrupt(system, 3);
                system.cdrom.cmd_state = CmdState::Resp2;
            }
            CmdState::Resp2 => {
                cdrom.stat.set_spindle_motor(false);
                cdrom.push_result_byte(cdrom.stat.0);
                Self::set_interrupt(system, 2);
                system.cdrom.finish_command();
            }
        }
    }
    // pub fn cmd_stop(&mut self) -> CommandResponse {
    //     // int3 bcd
    //     println!("cdrom stop");
    //     if !self.parameters.is_empty() {
    //         return error_response(&self.stat, 0x20, "gettn takes no params");
    //     }
    //
    //     if let Some(disk) = self.disk.as_mut() {
    //         disk.reset_read_head();
    //     }
    //
    //     self.stat.set_read(false);
    //     let first_status = self.stat.clone();
    //     self.stat.set_spindle_motor(false);
    //
    //     let delay = match self.mode.speed() {
    //         Speed::Normal => 0x0D3_8ACA,
    //         Speed::Double => 0x18A_6076,
    //     };
    //     CommandResponse::new()
    //         .int3([first_status.0], AVG_1ST_RESP_INIT)
    //         .int2([self.stat.0], delay)
    // }

    pub fn read_sector_data<T: Addressable>(&mut self) -> T {
        let mut bytes = [0_u8; 4];
        (0..T::width() as usize).for_each(|i| {
            bytes[i] = self.pop_from_data_buffer();
        });

        T::from_u32(u32::from_le_bytes(bytes))
    }

    pub fn consume_result_byte(&mut self) -> u8 {
        if self.results.len() > 0 {
            self.results.remove(0)
        } else {
            println!("cdrom warn: read from empty result fifo");
            0
        }
    }

    pub fn pop_from_data_buffer(&mut self) -> u8 {
        let data = self.data_buffer.pop_front().unwrap_or_else(|| {
            println!("CDROM warn! pop from empty buffer");
            0
        });
        // if self.data_buffer.is_empty() {
        // self.addr.set_data_request(false);
        // self.hsts.set_result_read_ready(false);
        // }
        data
    }

    pub fn reset_read_state(&mut self) {
        self.read_state = ReadState::Idle;
        self.read_delay = 0;
        self.audio_buffer.clear();
        self.data_buffer.clear();
        self.data_pos = 0;
        self.last_sector_header = [0; 8];
        self.stat.set_seek(false);
        self.stat.set_read(false);
        self.stat.set_play(false);
        self.stat.set_seek_error(false);
        self.stat.set_error(false);
    }

    pub fn begin_playing(&mut self) {
        self.read_state = ReadState::Playing;
        if self.seekloc.is_some() {
            self.stat.set_seek(true);
            self.read_delay = CDROM_SEEKL_DELAY_CYCLES;
        } else {
            self.stat.set_play(true);
            self.read_delay = CDROM_READ_DELAY_CYCLES;
        }
    }
    pub fn begin_reading(&mut self) {
        self.read_state = ReadState::Reading;
        // advance XA...
        if self.seekloc.is_some() {
            self.stat.set_seek(true);
            self.read_delay = CDROM_SEEKL_DELAY_CYCLES;
        } else {
            self.stat.set_read(true);
            self.read_delay = match self.mode.speed() {
                Speed::Normal => CDROM_READ_DELAY_CYCLES,
                Speed::Double => CDROM_READ_2X_DELAY_CYCLES,
            };
        }
    }

    pub fn tick(system: &mut System, num_cycles: u64) {
        let cdrom = &mut system.cdrom;
        if cdrom.irq_pending.ints() != 0 {
            return;
        }
        if cdrom.cmd_delay > 0 {
            cdrom.cmd_delay = cdrom.cmd_delay.saturating_sub(num_cycles as u32);
            if cdrom.cmd_delay == 0 {
                Self::step_command(system);
            }
        } else if cdrom.read_delay > 0 {
            cdrom.read_delay = cdrom.read_delay.saturating_sub(num_cycles as u32);
            if cdrom.read_delay == 0 {
                match cdrom.read_state {
                    ReadState::Idle => unreachable!(),
                    ReadState::Reading => Self::step_reading(system),
                    ReadState::Playing => Self::step_playing(system), 
                }
            }
        }
    }

    pub fn push_to_audio_buffer(&mut self, data: &[u8]) {
        let samples: Vec<i16> = data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        self.audio_buffer.extend(samples);
    }

    pub fn get_audio_sample(&mut self) -> i16 {
        let sample = self.audio_buffer.pop_front().unwrap_or(0);
        if self.mute { 0 } else { sample }
    }

    fn step_reading(system: &mut System) {
        let cdrom = &mut system.cdrom;
        if let Some(loc) = cdrom.seekloc.take() {
            cdrom.disk.as_mut().unwrap().seek(loc);
            cdrom.stat.set_seek(false);
            cdrom.read_delay = match cdrom.mode.speed() {
                Speed::Normal => CDROM_READ_DELAY_CYCLES,
                Speed::Double => CDROM_READ_2X_DELAY_CYCLES,
            };
        } else {
            let disk = cdrom.disk.as_mut().unwrap();
            if !disk.is_at_valid_location() {
                println!("WARN cdrom: play/read failed: invalid disc location");
                cdrom.stat.set_spindle_motor(false);
                cdrom.push_error(DriveError::SeekFailed);
                cdrom.reset_read_state();
                return;
            }
            cdrom.stat.set_read(true);
            cdrom.results.clear();

            let sector = disk.advance_sector();
            let Some(sector) = sector else {
                system.cdrom.push_error(DriveError::SeekFailed);
                Self::set_interrupt(system, 5);
                system.cdrom.finish_command();
                return;
            }; 
            
            cdrom.last_sector_header.copy_from_slice(&sector[12..20]);
            cdrom.read_delay = match cdrom.mode.speed() {
                Speed::Normal => CDROM_READ_DELAY_CYCLES,
                Speed::Double => CDROM_READ_2X_DELAY_CYCLES,
            };

            if cdrom.is_xa_audio_sector(&sector) {
                cdrom.process_xa_audio_sector(&sector);
                return;
            }
            let mut sector_data = VecDeque::from(sector);
            match cdrom.mode.sector_size() {
                SectorSize::DataOnly => {
                    sector_data.drain(0x818..);
                    sector_data.drain(..0x18);
                }
                SectorSize::WholeSectorExceptSyncBytes => {
                    sector_data.drain(..0xC);
                }
            }
            cdrom.data_buffer = sector_data;
            cdrom.data_pos = 0;
            cdrom.push_result_byte(cdrom.stat.0);
            Self::set_interrupt(system, 1);
        }
    }
    fn step_playing(system: &mut System) {
        let cdrom = &mut system.cdrom;
        if let Some(loc) = cdrom.seekloc.take() {
            cdrom.disk.as_mut().unwrap().seek(loc);
            cdrom.stat.set_seek(false);
            cdrom.read_delay = CDROM_READ_DELAY_CYCLES;
        } else {
            // FIXME: check valid location..
            cdrom.stat.set_play(true);
            let sector = cdrom.disk.as_mut().unwrap().advance_sector();
            let Some(sector) = sector else {
                system.cdrom.push_error(DriveError::SeekFailed);
                Self::set_interrupt(system, 5);
                system.cdrom.finish_command();
                return;
            }; 
            cdrom.push_to_audio_buffer(&sector);
            // let samples: Vec<i16> = bytemuck::cast_vec(sector);
            cdrom.read_delay = CDROM_READ_DELAY_CYCLES;

            if cdrom.mode.play_report() {
                Self::push_play_report(system);
            }
        }
    }
    fn push_play_report(system: &mut System) {
        let cdrom = &mut system.cdrom;
        let disk = cdrom.disk.as_ref().unwrap();
        let (mm, ss, ff) = disk.mm_ss_ff(disk.read_head);
        if ff % 10 != 0 {
            return;
        }
        let abs_pos = disk.read_head;
        let track = disk.current_track();

        let in_pregap = abs_pos < track.get_start();
        let track_index = if in_pregap {0} else {1};
        let rel_pos = abs_pos - track.get_start();

        let (rel_mm, rel_ss, rel_ff) = disk.mm_ss_ff(rel_pos);

        cdrom.results.clear();
        cdrom.push_result_byte(cdrom.stat.0);
        cdrom.push_result_byte(to_bcd(track.id).expect("ok"));
        cdrom.push_result_byte(to_bcd(track_index).expect("ok"));

        if (ff / 10) % 2 == 0 {
            cdrom.push_result_byte(to_bcd(mm).expect("ok"));
            cdrom.push_result_byte(to_bcd(ss).expect("ok"));
            cdrom.push_result_byte(to_bcd(ff).expect("ok"));
        } else {
            cdrom.push_result_byte(to_bcd(rel_mm).expect("ok"));
            cdrom.push_result_byte(to_bcd(rel_ss).expect("ok"));
            cdrom.push_result_byte(to_bcd(rel_ff).expect("ok"));
        }

        // TODO: peak values
        cdrom.push_result_byte(0);
        cdrom.push_result_byte(0);
        Self::set_interrupt(system, 1);

    }

    fn is_xa_audio_sector(&self, sector: &Vec<u8>) -> bool {
        if !self.mode.xa_adpcm() {
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
        if self.mode.xa_filter() {
            if file != self.filter_file || channel != self.filter_channel {
                return false;
            }
        }
        return true;
    }
    fn process_xa_audio_sector(&mut self, sector: &Vec<u8>) {
        let audio_header = cdxa::AudioHeader(sector[0x13]);

        let audio_samples = match (audio_header.channel(), audio_header.sample_rate()) {
            (cdxa::Channel::Mono, cdxa::SampleRate::R37800) => decode_audio_sector::<false>(
                &sector,
                &mut self.adpcm_history,
                &mut self.high_res_resamplers,
            ),
            (cdxa::Channel::Mono, cdxa::SampleRate::R18900) => decode_audio_sector::<false>(
                &sector,
                &mut self.adpcm_history,
                &mut self.low_res_resamplers,
            ),
            (cdxa::Channel::Mono, cdxa::SampleRate::Reserved) => unimplemented!(),
            (cdxa::Channel::Stereo, cdxa::SampleRate::R37800) => decode_audio_sector::<true>(
                &sector,
                &mut self.adpcm_history,
                &mut self.high_res_resamplers,
            ),
            (cdxa::Channel::Stereo, cdxa::SampleRate::R18900) => decode_audio_sector::<true>(
                &sector,
                &mut self.adpcm_history,
                &mut self.low_res_resamplers,
            ),
            (cdxa::Channel::Stereo, cdxa::SampleRate::Reserved) => todo!(),
            (cdxa::Channel::Reserved, cdxa::SampleRate::R37800) => unimplemented!(),
            (cdxa::Channel::Reserved, cdxa::SampleRate::R18900) => unimplemented!(),
            (cdxa::Channel::Reserved, cdxa::SampleRate::Reserved) => unimplemented!(),
        };
        self.audio_buffer.extend(audio_samples);
    }
    /*
        pub fn process_sector(&mut self, sector: Vec<u8>) -> bool {
            if self.stat.playing() {
                assert!(self.mode.cdda());
                self.push_to_audio_buffer(&sector);
                return true;
            }

            let sector_mode = sector[0xF];
            let file = sector[0x10];
            let channel = sector[0x11];
            let submode = sector[0x12];
            let is_realtime_audio = (submode & 0x44) == 0x44;
             let is_form2 = submode & (1 << 5) != 0;
            let mode = &self.mode;

            if sector_mode == 2 {
                if (self.mode.xa_filter()
                    && (file != self.filter_file || channel != self.filter_channel))
                    || (self.mode.xa_filter() && !is_realtime_audio)
                {
                    // is data
                } else {
                    if self.mode.xa_adpcm() && is_realtime_audio {
                        let audio_header = cdxa::AudioHeader(sector[0x13]);

                        let audio_samples = match (audio_header.channel(), audio_header.sample_rate()) {
                            (cdxa::Channel::Mono, cdxa::SampleRate::R37800) => {
                                decode_audio_sector::<false>(
                                    &sector,
                                    &mut self.adpcm_history,
                                    &mut self.high_res_resamplers,
                                )
                            }
                            (cdxa::Channel::Mono, cdxa::SampleRate::R18900) => {
                                decode_audio_sector::<false>(
                                    &sector,
                                    &mut self.adpcm_history,
                                    &mut self.low_res_resamplers,
                                )
                            }
                            (cdxa::Channel::Mono, cdxa::SampleRate::Reserved) => unimplemented!(),
                            (cdxa::Channel::Stereo, cdxa::SampleRate::R37800) => {
                                decode_audio_sector::<true>(
                                    &sector,
                                    &mut self.adpcm_history,
                                    &mut self.high_res_resamplers,
                                )
                            }
                            (cdxa::Channel::Stereo, cdxa::SampleRate::R18900) => {
                                decode_audio_sector::<true>(
                                    &sector,
                                    &mut self.adpcm_history,
                                    &mut self.low_res_resamplers,
                                )
                            }
                            (cdxa::Channel::Stereo, cdxa::SampleRate::Reserved) => todo!(),
                            (cdxa::Channel::Reserved, cdxa::SampleRate::R37800) => unimplemented!(),
                            (cdxa::Channel::Reserved, cdxa::SampleRate::R18900) => unimplemented!(),
                            (cdxa::Channel::Reserved, cdxa::SampleRate::Reserved) => unimplemented!(),
                        };
                        self.audio_buffer.extend(audio_samples);
                        return true;
                    }
                }

                // if self.mode.xa_filter() && is_realtime_audio {
                //     return false;
                // }
            }

            let mut sector_data = VecDeque::from(sector);
            match mode.sector_size() {
                SectorSize::DataOnly => {
                    sector_data.drain(0x818..);
                    sector_data.drain(..0x18);
                }
                SectorSize::WholeSectorExceptSyncBytes => {
                    sector_data.drain(..0xC);
                }
            }
            self.data_buffer = sector_data;
            self.addr.set_data_request(true);
            false
        }
    */
    // pub fn tick(system: &mut System) {
    //     if   (system.cdrom.hintmsk.enint() & system.cdrom.hinsts.intsts() != 0)
    //       || (system.cdrom.hintmsk.enbfwrdy() & system.cdrom.hinsts.bfwrdy())
    //       || (system.cdrom.hintmsk.enbfempt() & system.cdrom.hinsts.bfempt()) {
    //         system.irqctl.status.set_cdrom(true);
    //     }
    //     if let Some((cycles, irq, data, len)) = system.cdrom.pending_interrupt {
    //         system.scheduler.schedule(crate::scheduler::Event::CDRom(irq, data, len), cycles, None); // TODO: ????
    //         system.cdrom.pending_interrupt = None;
    //     }
    // }
    /*
    pub fn process_response(system: &mut System, response: ResponseType) {
        let cdrom = &mut system.cdrom;

        let irq = u8::from(&response);
        cdrom.results.clear();

        match response {
            ResponseType::INT5(xs) => cdrom.results.extend(xs),
            ResponseType::INT2(xs) => cdrom.results.extend(xs),
            ResponseType::INT3(xs) => cdrom.results.extend(xs),
            ResponseType::INT1 => {
                // assert disk is inserted..
                let sector = cdrom
                    .disk
                    .as_mut()
                    .expect("int1 disk present")
                    .advance_sector();
                let is_audio = cdrom.process_sector(sector);

                if is_audio {
                    return;
                }
                // advance sector and process audio if available
                // if sector is not audio
                cdrom.results.push(cdrom.stat.0);
            }
        };

        cdrom.hinsts.set_intsts(irq);
        cdrom.addr.set_result_read_ready(true);
        if cdrom.hintmsk.enint() & cdrom.hinsts.intsts() != 0 {
            system.irqctl.status.set_cdrom(true);
        }

        // system.cdrom.hinsts.set_intsts(irq);
        // system.cdrom.response_idx = n;
        // system.cdrom.response_read_idx = 0; // TODO: clear ready bit...
        // system.cdrom.response_buffer.copy_from_slice(&response);
        // system.cdrom.hsts.set_result_read_ready(true);
        //
    }*/
}

// 0-1 RA       Current register bank (R/W)
// 2   ADPBUSY  ADPCM busy            (R, 1=playing XA-ADPCM)
// 3   PRMEMPT  Parameter empty       (R, 1=parameter FIFO empty)
// 4   PRMWRDY  Parameter write ready (R, 1=parameter FIFO not full)
// 5   RSLRRDY  Result read ready     (R, 1=result FIFO not empty)
// 6   DRQSTS   Data request          (R, 1=one or more RDDATA reads or WRDATA writes pending)
// 7   BUSYSTS  Busy status           (R, 1=HC05 busy acknowledging command)
bitfield::bitfield! {
    #[derive(Default, Serialize, Deserialize)]
    pub struct AddressReg(u8);
    current_bank, set_current_bank: 1,0;
    _, set_adpcm_busy: 2;
    _, set_param_empty: 3;
    _, set_param_write_ready: 4;
    _, set_result_read_ready: 5;
    _, set_data_request: 6;
    _, set_busy_status: 7;
}

bitfield::bitfield! {
    #[derive(Default, Serialize, Deserialize)]
    pub struct ADPCTL(u8);
    adpmute, _: 0;
    chngatv, _: 5;
}

// 0-2 CLRINT     Acknowledge HC05 interrupt "flags" (0=no change, 1=clear)
// 3   CLRBFEMPT  Acknowledge BFEMPT                 (0=no change, 1=clear)
// 4   CLRBFWRDY  Acknowledge BFWRDY                 (0=no change, 1=clear)
// 5   SMADPCLR   Clear sound map XA-ADPCM buffer    (0=no change, 1=clear/stop playback)
// 6   CLRPRM     Clear parameter FIFO               (0=no change, 1=clear)
// 7   CHPRST     Reset decoder chip                 (0=no change, 1=reset)
bitfield::bitfield! {
    #[derive(Default, Serialize, Deserialize)]
    pub struct HCLRCTL(u8);
    clrint, _: 2,0;
    clrbfempt, _: 3;
    clrbfwrdy, _: 4;
    smadpclr, _: 5;
    clrprm, _: 6;
    chprst, _: 7;
}

// 0-2 ENINT    Enable IRQ on respective INTSTS bits
// 3   ENBFEMPT Enable IRQ on BFEMPT
// 4   ENBFWRDY Enable IRQ on BFWRDY
// 5-7 -        Reserved (should be 0 when written, always 1 when read)
bitfield::bitfield! {
    #[derive(Default, Serialize, Deserialize)]
    pub struct HINTMSK(u8);
    enint, _: 2,0;
    enbfempt, _: 3;
    enbfwrdy, _: 4;
    _, set_reserved: 7,5;
}

// 7  Play          Playing CD-DA         ;\only ONE of these bits can be set
// 6  Seek          Seeking               ; at a time (ie. Read/Play won't get
// 5  Read          Reading data sectors  ;/set until after Seek completion)
// 4  ShellOpen     Once shell open (0=Closed, 1=Is/was Open)
// 3  IdError       (0=Okay, 1=GetID denied) (also set when Setmode.Bit4=1)
// 2  SeekError     (0=Okay, 1=Seek error)     (followed by Error Byte)
// 1  Spindle Motor (0=Motor off, or in spin-up phase, 1=Motor on)
// 0  Error         Invalid Command/parameters (followed by Error Byte)
bitfield::bitfield! {
    #[derive(Default, Clone, Copy, Serialize, Deserialize, Debug)]
    pub struct StatusReg(u8);
    playing, set_play: 7;
    _, set_seek: 6;
    _, set_read: 5;
    _, set_shell_open: 4;
    _, set_id_error: 3;
    _, set_seek_error: 2;
    _, set_spindle_motor: 1;
    _, set_error: 0;
}

impl StatusReg {
    pub fn with_error(&self) -> u8 {
        self.0 | 0x01
    }
}

bitfield::bitfield! {
    #[derive(Default, Serialize, Deserialize)]
    pub struct HINTSTS(u8);
    intsts, set_intsts: 2,0;
    bfempt, set_bfempt: 3;
    bfwrdy, set_bfwrdy: 4;
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum Speed {
    Normal = 0,
    Double = 1,
}

impl From<u8> for Speed {
    fn from(value: u8) -> Self {
        match value {
            0 => Speed::Normal,
            1 => Speed::Double,
            _ => unreachable!("speed could be 0/1"),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum SectorSize {
    DataOnly = 0,
    WholeSectorExceptSyncBytes = 1,
}

impl From<u8> for SectorSize {
    fn from(value: u8) -> Self {
        match value {
            0 => SectorSize::DataOnly,
            1 => SectorSize::WholeSectorExceptSyncBytes,
            _ => unreachable!("sector size could be 0/1"),
        }
    }
}

impl Speed {
    fn transform<T>(self, value: T) -> T
    where
        T: Div<u64, Output = T>,
    {
        match self {
            Speed::Normal => value,
            Speed::Double => value / 2,
        }
    }
}
// 7   Speed       (0=Normal speed, 1=Double speed)
// 6   XA-ADPCM    (0=Off, 1=Send XA-ADPCM sectors to SPU Audio Input)
// 5   Sector Size (0=800h=DataOnly, 1=924h=WholeSectorExceptSyncBytes)
// 4   Ignore Bit  (0=Normal, 1=Ignore Sector Size and Setloc position)
// 3   XA-Filter   (0=Off, 1=Process only XA-ADPCM sectors that match Setfilter)
// 2   Report      (0=Off, 1=Enable Report-Interrupts for Audio Play)
// 1   AutoPause   (0=Off, 1=Auto Pause upon End of Track) ;for Audio Play
// 0   CDDA        (0=Off, 1=Allow to Read CD-DA Sectors; ignore missing EDC)
//
bitfield::bitfield! {
    #[derive(Default, Serialize, Deserialize)]
    pub struct ModeReg(u8);
    impl Debug;
    into Speed, speed, _: 7,7;
    xa_adpcm, _: 6;
    into SectorSize, sector_size, _: 5,5;
    ignore_bit, _: 4;
    xa_filter, _: 3;
    play_report, _: 2;
    auto_pause, _: 1;
    cdda, _: 0;
}

bitfield::bitfield! {
    #[derive(Default, Serialize, Deserialize)]
    pub struct RequestReg(u8);
    smen, _: 5;
    bfwr, _: 6;
    want_data, _: 7;//bfrd
}

bitfield::bitfield! {
    #[derive(Default, Serialize, Deserialize)]
    pub struct IrqMask(u8);
    int_enable, _: 2,0;
}

bitfield::bitfield! {
    #[derive(Default, Serialize, Deserialize)]
    pub struct IrqPending(u8);
    ints, _:  2,0;
}

bitfield::bitfield! {
    #[derive(Default, Serialize, Deserialize)]
    pub struct AckInterrupt(u8);
    int_clear, _:  2,0;
    xa_adpcm_clear, _: 5;
    param_clear, _: 6;
    chip_reset, _: 7;
}

const CDROM_AVG_DELAY_CYCLES: u32 = 50_401;
const CDROM_INIT_DELAY_CYCLES: u32 = 81_102;
const CDROM_SEEKL_DELAY_CYCLES: u32 = 450_000;
const CDROM_READ_DELAY_CYCLES: u32 = 33_868_800 / 75;
const CDROM_READ_2X_DELAY_CYCLES: u32 = 33_868_800 / 150;
const CDROM_PAUSE_DELAY_CYCLES: u32 = 2_168_860;
const CDROM_PAUSE_2X_DELAY_CYCLES: u32 = 1_097_107;
const CDROM_SPINUP_DELAY_CYCLES: u32 = 33_868_800; // 1 second

const CDROM_GETID_NODISK: [u8; 8] = [0x08, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
const CDROM_GETID_REGION: u8 = b'A'; // A=America, I=Japan, E=Europe
const CDROM_GETID_LICENSED: [u8; 8] =
    [0x02, 0x00, 0x20, 0x00, b'S', b'C', b'E', CDROM_GETID_REGION];
const CDROM_DATE_VERSION: [u8; 4] = [0x94, 0x09, 0x19, 0xC0];
// const CDROM_DATE_VERSION: [u8; 4] = [0x94, 0x09, 0x19, 0xC0];

pub const AVG_1ST_RESP_GENERIC: u64 = 0xC4E1;
pub const AVG_1ST_RESP_INIT: u64 = 0x13CCE;

pub const AVG_2ND_RESP_GET_ID: u64 = 0x4A00;
pub const AVG_2ND_RESP_PAUSE: u64 = 0x0021_181C;
pub const AVG_2ND_RESP_SEEKL: u64 = 0x6E1CD;

pub const AVG_RATE_INT1: u64 = 0x6E1CD;

const fn from_bcd(val: u8) -> Option<u8> {
    let ones = val & 0xF;
    let tens = val >> 4;
    if tens <= 9 && ones <= 9 {
        Some(10 * tens + ones)
    } else {
        None
    }
}

const fn to_bcd(val: u8) -> Option<u8> {
    if val > 99 {
        return None;
    }

    let tens = val / 10;
    let ones = val % 10;

    Some((tens << 4) | ones)
}

/// Disc serial number
#[derive(serde::Serialize, serde::Deserialize, Copy, Clone, PartialEq, Eq)]
pub struct SerialNumber([u8; 10]);

impl SerialNumber {
    /// Extract a serial number from a standard PlayStation binary
    /// name of the form "aaaa_ddd.dd"
    fn from_bin_name(bin: &[u8]) -> Option<SerialNumber> {
        if bin.len() != 11 {
            return None;
            // panic!("wrong bin len, name: {}", String::from_utf8(bin.to_vec()).expect("ok"));
        }

        // Ridge Racer (NA) uses "SCUS-943.00"
        if bin[4] != b'_' && bin[4] != b'-' {
            // This will fail for the few "lightspan educational" discs since they have a serial
            // number looking like "LSP-123456". Those games are fairly obscure and some of them
            // seem to have weird and nonstandards SYSTEM.CNF anyway.
            return None;
        }

        let mut serial = [0u8; 10];

        fn to_upper(b: u8) -> u8 {
            if b.is_ascii_lowercase() {
                b - b'a' + b'A'
            } else {
                b
            }
        }

        serial[0] = to_upper(bin[0]);
        serial[1] = to_upper(bin[1]);
        serial[2] = to_upper(bin[2]);
        serial[3] = to_upper(bin[3]);
        serial[4] = b'-';
        serial[5] = bin[5];
        serial[6] = bin[6];
        serial[7] = bin[7];
        serial[8] = bin[9];
        serial[9] = bin[10];

        Some(SerialNumber(serial))
    }

    pub fn region(&self) -> Option<Region> {
        match &self.0[0..4] {
            b"SCPS" | b"SLPS" | b"SLPM" | b"PAPX" => Some(Region::Japan),
            b"SCUS" | b"SLUS" | b"LSP-" => Some(Region::NorthAmerica),
            b"SCES" | b"SCED" | b"SLES" | b"SLED" => Some(Region::Europe),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    /// Japan (NTSC): SCEI
    Japan,
    /// North America (NTSC): SCEA
    NorthAmerica,
    /// Europe (PAL): SCEE
    Europe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CmdState {
    RecvCmd,
    Resp1,
    Resp2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadState {
    Idle,
    Reading,
    Playing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriveError {
    // first response errors:
    InvalidParameter,
    WrongParameterCount,
    InvalidCommand,
    CannotRespond,
    // second response errors:
    SeekFailed,
    // async / unsolicited:
    DriveDoorOpened,
}

impl DriveError {
    pub fn code(&self) -> u8 {
        match self {
            DriveError::InvalidParameter => 0x10,
            DriveError::WrongParameterCount => 0x20,
            DriveError::InvalidCommand => 0x40,
            DriveError::CannotRespond => 0x80,
            DriveError::SeekFailed => 0x04,
            DriveError::DriveDoorOpened => 0x08,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeReg {
    l2l_volume: u8,
    l2r_volume: u8,
    r2l_volume: u8,
    r2r_volume: u8,
}

/// Attempt to extract the serial number of the disc. All officially
/// licensed PlayStation game should have a serial number.
fn extract_serial_number(image: &mut CdCache) -> Option<SerialNumber> {
    let system_cnf = read_system_cnf(image);

    parse_serial_number_from_system_cnf(&system_cnf)
}

fn parse_serial_number_from_system_cnf(system_cnf: &[u8]) -> Option<SerialNumber> {
    // Now we need to parse the SYSTEM.CNF file to get the content of the "BOOT" line
    let mut boot_path = None;

    for line in system_cnf.split(|&b| b == b'\n' || b == b'\r') {
        let words: Vec<_> = line
            .split(|&b| b == b' ' || b == b'\t' || b == b'=')
            .filter(|w| !w.is_empty())
            .collect();

        if words.len() == 2 && words[0] == b"BOOT" {
            boot_path = Some(words[1]);
            break;
        }
    }

    let boot_path = match boot_path {
        Some(b) => b,
        None => {
            panic!("Couldn't find BOOT line in SYSTEM.CNF");
            // return Err(PsxError::NoSerialNumber);
        }
    };

    // boot_path should look like "cdrom:\FOO\BAR\...\aaaa_ddd.dd;1"
    //
    // Most (but not all) paths ends with a ";1", so get rid of it here
    let boot_path = boot_path.split(|&b| b == b';').next().unwrap();

    // boot_path should look like "cdrom:\FOO\BAR\...\aaaa_ddd.dd"
    let bin_name = boot_path
        .split(|&b| b == b':' || b == b'\\')
        .next_back()
        .unwrap();

    let serial = SerialNumber::from_bin_name(bin_name);

    serial
}

fn read_system_cnf(image: &mut CdCache) -> Vec<u8> {
    let dir = iso9660::open_image(image);

    let system_cnf = dir
        .entry_by_name(b"SYSTEM.CNF;1")
        .expect("system.cnf found");

    // SYSTEM.CNF should be a small text file, 1MB should be way more
    // than necessary
    let len = system_cnf.extent_len();

    if len > 1024 * 1024 {
        let desc = format!("SYSTEM.CNF is too big: {}B", len);
        return panic!("bad disc format");
    }

    let system_cnf = system_cnf.read_file(image).expect("ok");

    // let text = String::from_utf8(system_cnf.clone()).expect("ok");
    // println!("system.cnf: \n{}", text);

    system_cnf
}

pub fn pos_to_sectors(mins: u8, secs: u8, sect: u8) -> u32 {
    ((mins as u32) * 75 * 60) + ((secs as u32) * 75) + (sect as u32)
}
