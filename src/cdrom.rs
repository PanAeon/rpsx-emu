use std::collections::VecDeque;
use std::{io::Read, ops::Div};
use std::fs::File;

use arrayvec::ArrayVec;

use crate::cdxa::{self, AdpcmHistory, HighResResampler, LowResResampler, decode_audio_sector};
use crate::{system::{Addressable, System}, scheduler::Event};

const SECTOR_SIZE: usize = 0x930;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackType {
    Audio,
    Mode2352,
}

pub struct TrackIndex {
    pub id: u8,
    pub lba: usize,
}

pub struct Track {
    pub id: u8,
    pub track_type: TrackType,
    pub indexes: Vec<TrackIndex>

}

pub struct Image {
    read_head: usize,
    data: Box<[u8]>,
    tracks: Vec<Track>,
}

impl Image {
    pub fn new() -> Self {
        // let path = "/foo/psx/Spyro the Dragon (USA).bin";
        let path = "/foo/psx/Silent Hill (USA).bin";
        // let path = "/foo/psx/celeste-collection.bin";
        // let path = "/foo/psx/Crash Bandicoot (USA).bin";
        // let path = "/foo/psx/Earthworm Jim 2 (Europe) (Track 01).bin";
        // let path = "/foo/psx/Mortal Kombat Trilogy (USA) (v1.1) (Track 01).bin";
        // let path = "/foo/psx/Final Fantasy VII (USA) (Disc 1).bin";
        // let path = "/foo/psx/Mega Man X4 (USA).bin";
        let mut file = match File::open(path) {
            Ok(f) => f,
            Err(_) => panic!("can't read file")
        };
        let mut data: Vec<u8> = vec![0u8; 2 * 75 * SECTOR_SIZE];
        // let mut data: Vec<u8> = vec![];
        match file.read_to_end(&mut data) {
            Ok(_) => {},
            Err(_) => panic!("file read error")
        };
        // file.read_to_end(&mut data);
        Self {
            read_head: 0,
            data: data.into_boxed_slice(),
            tracks: vec![Track {
                id: 0,
                track_type: TrackType::Mode2352,
                indexes: vec![TrackIndex { id: 1, lba: 0}]
            }]
        }
    }

    pub fn seek_location(&mut self, mins: u8, secs: u8, sect: u8) {
        let sectors = ((mins as usize) * 75 * 60) + ((secs as usize) * 75) + (sect as usize);
        self.read_head = sectors * SECTOR_SIZE;
    }

    pub fn advance_sector(&mut self) -> Vec<u8> {
        let start = self.read_head;
        self.read_head += SECTOR_SIZE;
        self.data[start..self.read_head].to_vec()
    }

    pub fn reset_read_head(&mut self) {
        self.read_head = SECTOR_SIZE * 75 * 2;
    }

    pub fn first_track_id(&self) -> u8 {
        self.tracks.first().expect("first track").id
    }
    pub fn last_track_id(&self) -> u8 {
        self.tracks.first().expect("last track").id
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
        self.mm_ss_ff(self.data.len())
    }

    pub fn mm_ss_ff(&self, read_head: usize) -> (u8, u8, u8) {
        let sectors = read_head / SECTOR_SIZE;
        let secs = sectors / 75;
        let sect = sectors % 75;
        let mins = secs / 60;
        let secs = mins % 60;
        (mins as u8, secs as u8, sect as u8)
    }

    pub fn current_position_info(&self) -> [u8;8] {
          let current_track = self
            .tracks
            .partition_point(|t| t.indexes[0].lba <= self.read_head)
            .saturating_sub(1);

        let current_track = &self.tracks[current_track];

        let current_index = current_track
            .indexes
            .partition_point(|i| i.lba <= self.read_head)
            .saturating_sub(1);

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
}


pub struct CDRom {
    status: Status,
    hsts: HSTS,
    adpctl: ADPCTL,
    hintmsk: HINTMSK,
    mode: Mode,
    parameters: ArrayVec<u8,1024>,
    // param_buffer: [u8;16],
    // param_idx: usize,
    results: ArrayVec<u8, 16>,
    // response_buffer: [u8;16],
    // response_idx: usize,
    // response_read_idx: usize,
    // shell_open: bool,
    hinsts: HINTSTS,
    hcpctl: HCHPCTL,
    disk: Option<Image>,
    data_buffer: VecDeque<u8>,
    audio_muted: bool,
    l2l_volume: u8,
    l2r_volume: u8,
    r2l_volume: u8,
    r2r_volume: u8,

    filter_file: u8,
    filter_channel: u8,
    audio_buffer: VecDeque<i16>,

    // Left, Right, Mono
    adpcm_history: [AdpcmHistory; 3],
    high_res_resamplers: [HighResResampler; 3],
    low_res_resamplers: [LowResResampler; 3],
    // pending_interrupt: Option<(u64, u8, [u8;16], usize)>
}

impl Default for CDRom {
    fn default() -> Self {
        Self {
            status: Status(0),
            hsts: HSTS(0x18),
            adpctl: ADPCTL::default(),
            hintmsk: HINTMSK::default(),
            mode: Mode::default(),
            parameters: ArrayVec::default(),
            results: ArrayVec::default(),
            hinsts: HINTSTS::default(),
            hcpctl: HCHPCTL::default(),
            disk: Some(Image::new()),
            data_buffer: VecDeque::default(),
            audio_muted: false,
            l2l_volume: 0,
            l2r_volume: 0,
            r2l_volume: 0,
            r2r_volume: 0,
            filter_file: 0,
            filter_channel: 0,
            audio_buffer: VecDeque::default(),
            adpcm_history: Default::default(),
            high_res_resamplers: Default::default(),
            low_res_resamplers: Default::default(),
        }
    }
}

impl CDRom {
    pub fn store<T:Addressable>(system: &mut System, offset: u32, value: T) {
        let cdrom = &mut system.cdrom;
        let width = T::width() as usize;
        if width != 1 {
            panic!("cdrom store width other than 1 byte not impl, got: {:?}", T::width());
        }
        let v = value.as_u32() as u8;
        // println!("cdrom store {:X} {:X}",offset, value.as_u32());
        match cdrom.hsts.current_bank() {
            0 => {
                match offset {
                  0 => cdrom.hsts.set_current_bank(v),
                  1 => Self::process_command(system, v),
                  2 => cdrom.push_parameter(v),
                  3 => cdrom.hcpctl.0 = v,
                  _  => panic!("Unhandled cdrom store {:?}, bank: {}, offset: {:08x}, value: 0x{:x}", T::width(),cdrom.hsts.current_bank(),  offset, v),
                }
            },
            1 => {
                match offset {
                  0 => cdrom.hsts.set_current_bank(v),
                  // 1 => {}, // wrdata
                  2 => {cdrom.hintmsk.0 = v; cdrom.hintmsk.set_reserved(0xFF);},
                  3 => cdrom.set_hclrctl(v),
                  _  => panic!("Unhandled cdrom store {:?}, bank: {}, offset: {:08x}, value: 0x{:x}", T::width(),cdrom.hsts.current_bank(),  offset, v),
                }
            },
            2 => {
                match offset {
                  0 => cdrom.hsts.set_current_bank(v),
                  // 1 => {},
                  2 => {cdrom.l2l_volume = v.as_u32() as u8;},
                  3 => {cdrom.l2r_volume = v.as_u32() as u8;},
                  _  => panic!("Unhandled cdrom store {:?}, bank: {}, offset: {:08x}, value: 0x{:x}", T::width(),cdrom.hsts.current_bank(),  offset, v),
                }
            },
            3 => {
                match offset {
                  0 => cdrom.hsts.set_current_bank(v),
                  1 => {cdrom.r2r_volume = v.as_u32() as u8;},
                  2 => {cdrom.r2l_volume = v.as_u32() as u8;},
                  3 => cdrom.adpctl.0 = v,
                  _  => panic!("Unhandled cdrom store {:?}, bank: {}, offset: {:08x}, value: 0x{:x}", T::width(), cdrom.hsts.current_bank(), offset, v),
                }
            },
            _ => unreachable!("banks are 0..4"),
        };
    }

    pub fn push_parameter(&mut self, v: u8) {
        if self.parameters.is_empty() {
            self.hsts.set_param_empty(true);
        }
        self.parameters.push(v);

        if self.parameters.is_full() {
            self.hsts.set_param_write_ready(false);
        }
    }



    pub fn process_command(system: &mut System, cmd: u8) {
        let cdrom = &mut system.cdrom;
        if let 0x08..=0x09 = cmd {
            system.scheduler.unschedule(&Event::CDRomResultIrq(ResponseType::INT1));
        }
        let response = match cmd {
            0x00 => cdrom.cmd_unused(),
            0x01 => cdrom.cmd_nop(),
            0x02 => cdrom.cmd_setloc(),
            0x06 => cdrom.cmd_readn(),
            0x08 => cdrom.cmd_stop(),
            0x09 => cdrom.cmd_pause(),
            0x0a => cdrom.cmd_init(),
            0x0c => cdrom.cmd_demute(),
            0x0e => cdrom.cmd_setmode(),
            0x0d => cdrom.cmd_set_filter(),
            0x11 => cdrom.get_locp(),
            0x14 => cdrom.cmd_gettd(),
            0x15 => cdrom.cmd_seekl(),
            0x19 => cdrom.cmd_test(),
            0x1a => cdrom.cmd_getid(),
            0x1b => cdrom.cmd_reads(),
            0x13 => cdrom.cmd_gettn(),

            _ => panic!("Unhandled command: 0x{:x}", cmd),
        };
        cdrom.parameters.clear();
        cdrom.hsts.set_result_read_ready(true);
        cdrom.hsts.set_param_write_ready(true);
        response.responses.into_iter().for_each(|(res_type, delay)| {
            let repeat = match res_type {
                ResponseType::INT1 => Some(cdrom.mode.speed().transform(AVG_RATE_INT1)),
                _ => None
            };
            system.scheduler.schedule(Event::CDRomResultIrq(res_type), delay, repeat);
        });
    }
    pub fn cmd_unused(&mut self) -> CommandResponse {
        // self.set_int(5);
        // self.response_idx = 2;
        // self.response_buffer[0] = 0x11;
        // self.response_buffer[1] = 0x40;
        // self.response_read_idx = 0;
        // self.hsts.set_result_read_ready(true);
        CommandResponse::new().int5([0x11, 0x40], AVG_1ST_RESP_GENERIC)
    }

    pub fn invalid(&mut self) -> CommandResponse {
        error_response(&self.status, 0x40, "invalid cmd")
    }

    pub fn cmd_nop(&mut self) -> CommandResponse { // nop, clears tray open bit, response: INT3: status
        if !self.parameters.is_empty() {
            return error_response(&self.status, 0x20, "nop takes no params")
        }
        CommandResponse::new().int3([self.status.0], AVG_1ST_RESP_GENERIC)
    }

    pub fn cmd_test(&mut self) -> CommandResponse {
        if self.parameters.len() != 1 {
            return error_response(&self.status, 0x20, "test expects one param")
        }
        println!("CDROM command test");
        let subcmd = self.parameters[0];
        match subcmd {
            0x20 => { // INT3(yy,mm,dd,ver) ;Get cdrom BIOS date/version (yy,mm,dd,ver) INT3(yy,mm,dd,ver) ;Get cdrom BIOS date/version (yy,mm,dd,ver)
                println!("cdrom get version");
                CommandResponse::new().int3([149, 5, 22, 193], AVG_1ST_RESP_GENERIC)
            },
            _ => panic!("Unhandled 0x19 (test) subcmd: 0x{:x}", subcmd)
        }
    }
    pub fn cmd_setloc(&mut self) -> CommandResponse {
        if self.parameters.len() != 3 {
            return error_response(&self.status, 0x20, "setloc takes 3 params")
        }
        let amm_opt = from_bcd(self.parameters[0]);
        let ass_opt = from_bcd(self.parameters[1]).filter(|x| *x < 60);
        let asect_opt = from_bcd(self.parameters[2]).filter(|x| *x < 75);
        let Some(amm) = amm_opt else {
            return error_response(&self.status, 0x10, "amm is incorrect");
        };
        let Some(ass) = ass_opt else {
            return error_response(&self.status, 0x10, "ass is incorrect");
        };
        let Some(asect) = asect_opt else {
            return error_response(&self.status, 0x10, "asect is incorrect");
        };

        self.disk.as_mut().expect("set_loc disk present")
            .seek_location(amm, ass, asect);
        // self.disk

        println!("CDROM setloc amm: {amm} ass: {ass} asect: {asect}");
        CommandResponse::new().int3([self.status.0], AVG_1ST_RESP_GENERIC)
    }

    pub fn cmd_seekl(&mut self) -> CommandResponse {
        if !self.parameters.is_empty() {
            return error_response(&self.status, 0x20, "seekl doesn't takes parameters")
        }

        let mut seeking = self.status.clone();
        seeking.set_seek(true);
        self.status.set_seek(false);
        println!("CDROM seekl");
        CommandResponse::new()
            .int3([seeking.0], AVG_1ST_RESP_GENERIC)
            .int2([self.status.0], AVG_1ST_RESP_GENERIC + AVG_2ND_RESP_SEEKL)
    }

    pub fn cmd_setmode(&mut self) -> CommandResponse {
        if self.parameters.len() != 1 {
            return error_response(&self.status, 0x20, "setmode takes 1 params")
        }
        let mode = self.parameters[0];

        self.mode.0 = mode;

        println!("CDROM setmode 0x{:X}, {:?}", mode, self.mode);



        CommandResponse::new().int3([self.status.0], AVG_1ST_RESP_GENERIC)
    }

    pub fn cmd_set_filter(&mut self) -> CommandResponse {
        if self.parameters.len() != 2 {
            return error_response(&self.status, 0x20, "setfilter takes 2 params")
        }
        let file = self.parameters[0];
        let channel = self.parameters[1];

        self.filter_file = file;
        self.filter_channel = channel;

        println!("CDROM setfilter 0x{:X}, 0x{:X}", file, channel);

        CommandResponse::new().int3([self.status.0], AVG_1ST_RESP_GENERIC)
    }

    pub fn get_locp(&mut self) -> CommandResponse {
        if !self.parameters.is_empty() {
            return error_response(&self.status, 0x20, "get_locp doesn't takes parameters")
        }
        println!("CDROM get_locp!!");

        let disk = self.disk.as_ref().expect("get_locp inserted disk");

        CommandResponse::new()
            .int3(disk.current_position_info().
                map(|x| to_bcd(x).expect("track position is valid bcd")), AVG_1ST_RESP_GENERIC)
    }

    pub fn cmd_readn(&mut self) -> CommandResponse {
        if !self.parameters.is_empty() {
            return error_response(&self.status, 0x20, "readn doesn't takes parameters")
        }
        println!("CDROM readn!!");

        self.status.set_read(true);
        CommandResponse::new()
            .int3([self.status.0], AVG_1ST_RESP_GENERIC)
            .int1( AVG_1ST_RESP_GENERIC + self.mode.speed().transform(AVG_RATE_INT1))
    }

    pub fn cmd_reads(&mut self) -> CommandResponse {
        if !self.parameters.is_empty() {
            return error_response(&self.status, 0x20, "reads doesn't takes parameters")
        }
        println!("CDROM reads!!");

        self.status.set_read(true);
        CommandResponse::new()
            .int3([self.status.0], AVG_1ST_RESP_GENERIC)
            .int1( AVG_1ST_RESP_GENERIC + self.mode.speed().transform(AVG_RATE_INT1))
    }

    pub fn cmd_demute(&mut self) -> CommandResponse {
        if !self.parameters.is_empty() {
            return error_response(&self.status, 0x20, "pause doesn't takes parameters")
        }
        // turn on audio streaming to spu
        println!("CDROM demute");
        self.audio_muted = false;

        CommandResponse::new()
            .int3([self.status.0], AVG_1ST_RESP_GENERIC)
    }
    pub fn cmd_pause(&mut self) -> CommandResponse {
        if !self.parameters.is_empty() {
            return error_response(&self.status, 0x20, "pause doesn't takes parameters")
        }
        println!("CDROM pause!!");

        let current = self.status.clone();
        self.status.set_read(false);
        CommandResponse::new()
            .int3([current.0], AVG_1ST_RESP_GENERIC)
            .int2([self.status.0], AVG_1ST_RESP_GENERIC + AVG_2ND_RESP_PAUSE)
    }

    pub fn cmd_getid(&mut self) -> CommandResponse {
        if !self.parameters.is_empty() {
            return error_response(&self.status, 0x20, "get id takes no params")
        }
        println!("CDROM get id");
        // self.set_int(3);
        // self.push_status();
        // then we need to schedule response...
        // let mut data = [0;16];
        // data[0] = 0x8;
        // data[1] = 0x40;
        // self.pending_interrupt = Some((0x4A00, 0x5, data, 8)); // it works!!!! (no-cd)
        CommandResponse::new().int3([self.status.0], AVG_1ST_RESP_GENERIC)
            .int2([0x02, 0x00, 0x20, 0x00, b'S', b'C', b'E', b'A'], 
                AVG_1ST_RESP_GENERIC + AVG_2ND_RESP_GET_ID)

        // let  data = [0x02u8,0x00, 0x20,0x00, 0x53,0x43,0x45,0x41,0,0,0,0,0,0,0,0]; // na
        // let  data = [0x02u8,0x00, 0x20,0x00, 0x53,0x43,0x45,0x45,0,0,0,0,0,0,0,0]; //eu
        //
        // self.pending_interrupt = Some((0x4A00, 0x3, data, 8));
    }

    pub fn cmd_init(&mut self) -> CommandResponse {
        if !self.parameters.is_empty() {
            return error_response(&self.status, 0x20, "init takes no params")
        }
        println!("CDROM init");
        // Sets mode=20h, activates drive motor, Standby, abort all commands.
        self.mode.0 = 0x20;

        if let Some(disk) = self.disk.as_mut() {
            disk.reset_read_head();
        }

        // reset read head?

        let old_status = self.status.clone();
        self.status.set_spindle_motor(true);
        CommandResponse::new().int3([old_status.0], AVG_1ST_RESP_GENERIC)
            .int2([self.status.0], AVG_1ST_RESP_GENERIC + AVG_2ND_RESP_SEEKL)
    }
    pub fn cmd_gettn(&mut self) -> CommandResponse { // int3 bcd
        println!("cdrom gettn");
        if !self.parameters.is_empty() {
            return error_response(&self.status, 0x20, "gettn takes no params")
        }
        let disk = self.disk.as_ref().expect("gettn inserted disk");

        let first_track = to_bcd(disk.first_track_id()).expect("valid bcd");
        let last_track = to_bcd(disk.last_track_id()).expect("valid bcd");
        CommandResponse::new().int3([self.status.0, first_track, last_track], AVG_1ST_RESP_INIT)
    }
    pub fn cmd_gettd(&mut self) -> CommandResponse { // int3 bcd
        println!("cdrom gettd");
        if self.parameters.len() != 1 {
            return error_response(&self.status, 0x20, "gettd takes 1 param")
        }
        let disk = self.disk.as_ref().expect("gettd inserted disk");
        let last_track = disk.last_track_id();

        let Some(track) = from_bcd(self.parameters[0]).filter(|&x| x <= last_track) else {
            return error_response(&self.status, 0x10, "gettd wrong track bcd")
        };

        let (mm, ss, _) = if track != 0 {
            disk.track_mm_ss_ff(track)
        } else {
            disk.last_track_end()
        };
        // error_response(&self.status, 0x10, "gettd not impl")
        CommandResponse::new().int3([self.status.0, to_bcd(mm).expect("bcd"), to_bcd(ss).expect("bcd")], AVG_1ST_RESP_INIT)
        // unimplemented!()
    }

    pub fn cmd_stop(&mut self) -> CommandResponse { // int3 bcd
        println!("cdrom stop");
        if !self.parameters.is_empty() {
            return error_response(&self.status, 0x20, "gettn takes no params")
        }

        if let Some(disk) = self.disk.as_mut() {
            disk.reset_read_head();
        }

        self.status.set_read(false);
        let first_status = self.status.clone();
        self.status.set_spindle_motor(false);

        let delay = match self.mode.speed() {
            Speed::Normal => 0x0D3_8ACA,
            Speed::Double => 0x18A_6076,
        };
        CommandResponse::new().int3([first_status.0], AVG_1ST_RESP_INIT)
            .int2([self.status.0], delay)
    }




    pub fn read_sector_data<T:Addressable>(&mut self) -> T {
        let mut bytes = [0_u8;4];
        (0..T::width() as usize).for_each(|i|{
            bytes[i] = self.pop_from_data_buffer();
        });

        T::from_u32(u32::from_le_bytes(bytes))
    }
    pub fn load<T:Addressable>(&mut self, offset: u32) -> T {
        let width = T::width() as usize;
        // println!("cdrom load {}",offset);
        if offset == 2 {
            return self.read_sector_data::<T>();
        }
        if width != 1 {
            panic!("cdrom load width other than 1 byte not impl, got: {:?}", T::width());
        }
        let result = match self.hsts.current_bank() {
            0 => {
                match offset {
                  0 => self.get_hsts(),
                  1 => self.read_response(),
                  3 => self.hintmsk.0,
                  _  => panic!("Unhandled cdrom load {:?}, bank: {}, offset: {:08x}", T::width(),self.hsts.current_bank(),  offset),
                }
            },
            1 => {
                match offset {
                  0 => self.get_hsts(),
                  1 => self.read_response(),
                  3 => self.hinsts.0,
                  _  => panic!("Unhandled cdrom load {:?}, bank: {}, offset: {:08x}", T::width(),self.hsts.current_bank(),  offset),
                }
            },
            2 => {
                match offset {
                  0 => self.get_hsts(),
                  1 => self.read_response(),
                  3 => self.hintmsk.0,
                  _  => panic!("Unhandled cdrom load {:?}, bank: {}, offset: {:08x}", T::width(),self.hsts.current_bank(),  offset),
                }
            },
            3 => {
                match offset {
                  0 => self.get_hsts(),
                  1 => self.read_response(),
                  3 => self.hinsts.0,
                  _  => panic!("Unhandled cdrom load {:?}, bank: {}, offset: {:08x}", T::width(),self.hsts.current_bank(),  offset),
                }
            },
            _ => unreachable!("banks are 0..4"),
        };
        T::from_u32(result as u32)
    }
    pub fn get_hsts(&mut self) -> u8 {
        self.hsts.0
    }

    pub fn set_hclrctl(&mut self, v: u8) {
        self.hinsts.0 &= !(v & 0x1F);
        let _ = HCLRCTL(v);
        // self.hinsts.set_intsts(self.hinsts.intsts() & !x.clrint());
        // self.hinsts.set_bfempt(self.hinsts.bfempt() & !x.clrbfempt());
        // self.hinsts.set_bfwrdy(self.hinsts.bfwrdy() & !x.clrbfwrdy());
    // enbfempt, _: 3;
    // enbfwrdy, _: 4;
        // if x.clrint() != 0 || x.clrbfempt() || x.clrbfwrdy() {
        //     self.param_idx = 0;
        // }
//         Setting bits 0-4 resets the corresponding flags in HINTSTS; 
        //         normally one should write 07h to reset the HC05 interrupt flags, or 1Fh to acknowledge all IRQs. 
        //         Acknowledging individual HC05 flags (e.g. writing 01h to change INT3 to INT2) is possible, 
        //         if completely useless. 
        //         After acknowledge, the result FIFO is drained and if there's been a pending command, 
        //         then that command gets send to the controller.
// Setting CHPRST will result in a complete reset of the decoder. Unclear if this also reboots the HC05 and CD-ROM DSP (the decoder has an "external reset" pin which is pulled low when setting CHPRST).
    }

    pub fn read_response(&mut self) -> u8 {
        let val = self.results.remove(0);
        if self.results.is_empty() {
            self.hsts.set_result_read_ready(false);
        }
        // if self.response_read_idx >= self.response_idx {
        //     res = 0;
        // }
        // if self.response_read_idx == 16 {
        //     self.response_read_idx = 0;
        // }
        val
    }

    pub fn pop_from_data_buffer(&mut self) -> u8 {
        let data = self.data_buffer.pop_front().unwrap_or_else(|| {
            println!("CDROM warn! pop from empty buffer");
            0
        });
        if self.data_buffer.is_empty() {
            self.hsts.set_data_request(false);
        }
        data
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
        if self.audio_muted { 0 } else { sample }
    }

    pub fn process_sector(&mut self, sector: Vec<u8>) -> bool {
        if self.status.playing() {
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

            if self.mode.xa_filter() &&  (file != self.filter_file || channel != self.filter_channel) {
                return false;
            }

            if self.mode.xa_adpcm() && is_realtime_audio {
                let audio_header = cdxa::AudioHeader(sector[0x13]);

                let audio_samples = match (audio_header.channel(), audio_header.sample_rate()) {
                    (cdxa::Channel::Mono, cdxa::SampleRate::R37800) => 
                        decode_audio_sector::<false>(&sector, &mut self.adpcm_history, &mut self.high_res_resamplers),
                    (cdxa::Channel::Mono, cdxa::SampleRate::R18900) => 
                        decode_audio_sector::<false>(&sector, &mut self.adpcm_history, &mut self.low_res_resamplers),
                    (cdxa::Channel::Mono, cdxa::SampleRate::Reserved) => unimplemented!(),
                    (cdxa::Channel::Stereo, cdxa::SampleRate::R37800) => 
                        decode_audio_sector::<true>(&sector, &mut self.adpcm_history, &mut self.high_res_resamplers),
                    (cdxa::Channel::Stereo, cdxa::SampleRate::R18900) => 
                        decode_audio_sector::<true>(&sector, &mut self.adpcm_history, &mut self.low_res_resamplers),
                    (cdxa::Channel::Stereo, cdxa::SampleRate::Reserved) => todo!(),
                    (cdxa::Channel::Reserved, cdxa::SampleRate::R37800) => unimplemented!(),
                    (cdxa::Channel::Reserved, cdxa::SampleRate::R18900) => unimplemented!(),
                    (cdxa::Channel::Reserved, cdxa::SampleRate::Reserved) => unimplemented!(),
                };
                self.audio_buffer.extend(audio_samples);
                return true;
            }

            if self.mode.xa_filter() && is_realtime_audio {
                return false;
            }

        }

        let mut sector_data = VecDeque::from(sector);
        match mode.sector_size() {
            SectorSize::DataOnly => {
                sector_data.drain(0x818..);
                sector_data.drain(..0x18);
            },
            SectorSize::WholeSectorExceptSyncBytes => {
                sector_data.drain(..0xC);
            },
        }
        self.data_buffer = sector_data;
        self.hsts.set_data_request(true);
        false
    }

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
                let sector = cdrom.disk.as_mut().expect("int1 disk present")
                    .advance_sector();
                let is_audio = cdrom.process_sector(sector);

                if is_audio {
                    return;
                }
                // advance sector and process audio if available
                // if sector is not audio
                cdrom.results.push(cdrom.status.0);

            }
        };

        cdrom.hinsts.set_intsts(irq);
        cdrom.hsts.set_result_read_ready(true);
        if cdrom.hintmsk.enint() & cdrom.hinsts.intsts() != 0 {
            system.irqctl.status.set_cdrom(true);
        }

        // system.cdrom.hinsts.set_intsts(irq);
        // system.cdrom.response_idx = n;
        // system.cdrom.response_read_idx = 0; // TODO: clear ready bit...
        // system.cdrom.response_buffer.copy_from_slice(&response);
        // system.cdrom.hsts.set_result_read_ready(true);
        //
        
    }
}


  // 0-1 RA       Current register bank (R/W)
  // 2   ADPBUSY  ADPCM busy            (R, 1=playing XA-ADPCM)
  // 3   PRMEMPT  Parameter empty       (R, 1=parameter FIFO empty)
  // 4   PRMWRDY  Parameter write ready (R, 1=parameter FIFO not full)
  // 5   RSLRRDY  Result read ready     (R, 1=result FIFO not empty)
  // 6   DRQSTS   Data request          (R, 1=one or more RDDATA reads or WRDATA writes pending)
  // 7   BUSYSTS  Busy status           (R, 1=HC05 busy acknowledging command)
bitfield::bitfield! {
    #[derive(Default)]
    pub struct HSTS(u8);
    current_bank, set_current_bank: 1,0;
    _, set_adpcm_busy: 2;
    _, set_param_empty: 3;
    _, set_param_write_ready: 4;
    _, set_result_read_ready: 5;
    _, set_data_request: 6;
    _, set_busy_status: 7;
}

bitfield::bitfield! {
    #[derive(Default)]
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
    #[derive(Default)]
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
    #[derive(Default)]
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
    #[derive(Default, Clone, Copy)]
    pub struct Status(u8);
    playing, set_play: 7;
    _, set_seek: 6;
    _, set_read: 5;
    _, set_shell_open: 4;
    _, set_id_error: 3;
    _, set_seek_error: 2;
    _, set_spindle_motor: 1;
    _, set_error: 0;
}

impl Status {
    pub fn with_error(&self) -> u8 {
        self.0 | 0x01
    }
}

bitfield::bitfield! {
    #[derive(Default)]
    pub struct HINTSTS(u8);
    intsts, set_intsts: 2,0;
    bfempt, set_bfempt: 3;
    bfwrdy, set_bfwrdy: 4;
}


#[derive(Clone, Copy, Debug)]
pub enum Speed {
    Normal = 0,
    Double = 1
}

impl From<u8> for Speed {
    fn from(value: u8) -> Self {
        match value {
            0 => Speed::Normal,
            1 => Speed::Double,
            _ => unreachable!("speed could be 0/1")
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SectorSize {
    DataOnly = 0,
    WholeSectorExceptSyncBytes = 1
}

impl From<u8> for SectorSize {
    fn from(value: u8) -> Self {
        match value {
            0 => SectorSize::DataOnly,
            1 => SectorSize::WholeSectorExceptSyncBytes,
            _ => unreachable!("sector size could be 0/1")
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
            Speed::Double => value / 2
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
    #[derive(Default)]
    pub struct Mode(u8);
    impl Debug;
    into Speed, speed, _: 7,7;
    xa_adpcm, _: 6;
    into SectorSize, sector_size, _: 5,5;
    ignore_bit, _: 4;
    xa_filter, _: 3;
    report, _: 2;
    auto_pause, _: 1;
    cdda, _: 0;
}

bitfield::bitfield! {
    #[derive(Default)]
    pub struct HCHPCTL(u8);
    smen, _: 5;
    bfwr, _: 6;
    bfrd, _: 7;
}

pub const AVG_1ST_RESP_GENERIC: u64 = 0xC4E1;
pub const AVG_1ST_RESP_INIT: u64 = 0x13CCE;

pub const AVG_2ND_RESP_GET_ID: u64 = 0x4A00;
pub const AVG_2ND_RESP_PAUSE: u64 = 0x0021_181C;
pub const AVG_2ND_RESP_SEEKL: u64 = 0x6E1CD;

pub const AVG_RATE_INT1: u64 = 0x6E1CD;

#[derive(PartialEq, Eq, Clone)]
pub enum ResponseType {
    INT3(ArrayVec<u8, 8>),
    INT2(ArrayVec<u8, 8>),
    INT5([u8;2]),
    INT1
}

impl From<&ResponseType> for u8 {
    fn from(value: &ResponseType) -> Self {
        match value {
            ResponseType::INT1 => 1,
            ResponseType::INT2(_) => 2,
            ResponseType::INT3(_) => 3,
            ResponseType::INT5(_) => 5,
        }
    }
}

#[derive(Default)]
pub struct CommandResponse {
    pub responses: ArrayVec<(ResponseType,u64), 2>
}

impl CommandResponse {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn int3<const N: usize>(mut self, data: [u8;N], delay: u64) -> Self {
        let arr = ArrayVec::from_iter(data);
        self.responses.push((ResponseType::INT3(arr), delay));
        self
    }

    pub fn int2<const N: usize>(mut self, data: [u8;N], delay: u64) -> Self {
        let arr = ArrayVec::from_iter(data);
        self.responses.push((ResponseType::INT2(arr), delay));
        self
    }

    pub fn int5(mut self, data: [u8;2], delay: u64) -> Self {
        self.responses.push((ResponseType::INT5(data), delay));
        self
    }

    pub fn int1(mut self, delay: u64) -> Self {
        self.responses.push((ResponseType::INT1, delay));
        self
    }
}

fn error_response(stat: &Status, err_byte: u8, err: &str) -> CommandResponse {
    println!("CDROM error {}", err);
    CommandResponse::new().int5([stat.with_error(), err_byte], AVG_1ST_RESP_INIT)
}

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
