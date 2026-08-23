use crate::memory_bus::{Addressable, MemoryBus};


#[derive(Default)]
pub struct CDRom {
    hsts: HSTS,
    adpctl: ADPCTL,
    hintmsk: HINTMSK,
    mode: Mode,
    param_buffer: [u8;16],
    param_idx: usize,
    response_buffer: [u8;16],
    response_idx: usize,
    response_read_idx: usize,
    shell_open: bool,
    hinsts: HINTSTS,
    pending_interrupt: Option<(u64, u8, [u8;16], usize)>
}

impl CDRom {
    pub fn store<T:Addressable>(&mut self, offset: u32, value: T) {
        let width = T::width() as usize;
        if width != 1 {
            panic!("cdrom store width other than 1 byte not impl, got: {:?}", T::width());
        }
        let v = value.as_u32() as u8;
        match self.hsts.current_bank() {
            0 => {
                match offset {
                  0 => self.hsts.set_current_bank(v),
                  1 => self.process_command(v),
                  2 => self.push_parameter(v),
                  // 3 => {},
                  _  => panic!("Unhandled cdrom store {:?}, bank: {}, offset: {:08x}, value: 0x{:x}", T::width(),self.hsts.current_bank(),  offset, v),
                }
            },
            1 => {
                match offset {
                  0 => self.hsts.set_current_bank(v),
                  // 1 => {}, // wrdata
                  2 => {self.hintmsk.0 = v; self.hintmsk.set_reserved(0xFF);},
                  3 => self.set_hclrctl(v),
                  _  => panic!("Unhandled cdrom store {:?}, bank: {}, offset: {:08x}, value: 0x{:x}", T::width(),self.hsts.current_bank(),  offset, v),
                }
            },
            2 => {
                match offset {
                  0 => self.hsts.set_current_bank(v),
                  // 1 => {},
                  // 2 => {},
                  // 3 => {},
                  _  => panic!("Unhandled cdrom store {:?}, bank: {}, offset: {:08x}, value: 0x{:x}", T::width(),self.hsts.current_bank(),  offset, v),
                }
            },
            3 => {
                match offset {
                  0 => self.hsts.set_current_bank(v),
                  // 1 => {},
                  // 2 => {},
                  3 => self.adpctl.0 = v,
                  _  => panic!("Unhandled cdrom store {:?}, bank: {}, offset: {:08x}, value: 0x{:x}", T::width(), self.hsts.current_bank(), offset, v),
                }
            },
            _ => unreachable!("banks are 0..4"),
        };
    }

    pub fn push_parameter(&mut self, v: u8) {
        if self.param_idx < 16 {
            self.param_buffer[self.param_idx] = v;
            self.param_idx += 1;
        }
    }

    pub fn set_int(&mut self, interrupt: u8) {
        self.hinsts.set_intsts(interrupt);
    }
    pub fn push_status(&mut self) {
        self.response_read_idx = 0;
        self.response_idx = 1;
        let status = self.get_status();
        self.response_buffer[0] = status.0;
        self.hsts.set_result_read_ready(true);
        // TODO: populate status
    }

    pub fn get_status(&self) -> Status {
        let mut status = Status(0);
        status.set_shell_open(self.shell_open);
        status
    }

    pub fn process_command(&mut self, cmd: u8) {
        match cmd {
            0x00 => self.cmd_unused(),
            0x01 => self.cmd_nop(),
            0x0a => self.cmd_init(),
            0x19 => self.cmd_test(),
            0x1a => self.cmd_getid(),
            0x13 => self.cmd_gettn(),

            _ => panic!("Unhandled command: 0x{:x}", cmd),
        }
    }
    pub fn cmd_unused(&mut self) {
        self.set_int(5);
        self.response_idx = 2;
        self.response_buffer[0] = 0x11;
        self.response_buffer[1] = 0x40;
        self.response_read_idx = 0;
        self.hsts.set_result_read_ready(true);
    }

    pub fn cmd_nop(&mut self) { // nop, clears tray open bit, response: INT3: status
        self.shell_open = false;
        self.set_int(3);
        self.push_status();
    }

    pub fn cmd_test(&mut self) {
        let subcmd = self.param_buffer[0];
        match subcmd {
            0x20 => { // INT3(yy,mm,dd,ver) ;Get cdrom BIOS date/version (yy,mm,dd,ver) INT3(yy,mm,dd,ver) ;Get cdrom BIOS date/version (yy,mm,dd,ver)
                self.set_int(3);
                self.response_idx = 4;
                self.response_buffer[0] = 149;
                self.response_buffer[1] = 5;
                self.response_buffer[2] = 22;
                self.response_buffer[3] = 193;
                self.response_read_idx = 0;
                self.hsts.set_result_read_ready(true);
            },
            _ => panic!("Unhandled 0x19 (test) subcmd: 0x{:x}", subcmd)
        }
    }

    pub fn cmd_getid(&mut self) {
        self.set_int(3);
        self.push_status();
        // then we need to schedule response...
        let mut data = [0;16];
        data[0] = 0x8;
        data[1] = 0x40;
        self.pending_interrupt = Some((0x4A00, 0x5, data, 8)); // it works!!!! (no-cd)

        // let mut data = [0x02u8,0x00, 0x20,0x00, 0x53,0x43,0x45,0x41,0,0,0,0,0,0,0,0]; // na
        // let mut data = [0x02u8,0x00, 0x20,0x00, 0x53,0x43,0x45,0x45,0,0,0,0,0,0,0,0]; //eu
        //
        // self.pending_interrupt = Some((0x4A00, 0x3, data, 8));
    }

    pub fn cmd_init(&mut self) {
        // Sets mode=20h, activates drive motor, Standby, abort all commands.
        self.mode.0 = 0x20;
        self.set_int(3);
        self.push_status();
        let mut data = [0;16];
        data[0] = self.get_status().0;
        self.pending_interrupt = Some((0x4A00, 0x2, data, 1));
    }
    pub fn cmd_gettn(&mut self) { // int3 bcd
        // let mut data = [0;16];
        self.set_int(3);
        self.response_buffer[0] = self.get_status().0;
        self.response_buffer[1] = 0x1;
        self.response_buffer[2] = 0x1;
        self.response_idx = 3;
        self.response_read_idx = 0;
        self.hsts.set_result_read_ready(true);
        // self.pending_interrupt = Some((0x4A00, 0x3, data, 3));
    }



    pub fn load<T:Addressable>(&mut self, offset: u32) -> T {
        let width = T::width() as usize;
        if width != 1 {
            panic!("cdrom store width other than 1 byte not impl, got: {:?}", T::width());
        }
        let result = match self.hsts.current_bank() {
            0 => {
                match offset {
                  0 => self.getHSTS(),
                  1 => self.read_response(),
                  // 2 => {},
                  // 3 => {},
                  _  => panic!("Unhandled cdrom load {:?}, bank: {}, offset: {:08x}", T::width(),self.hsts.current_bank(),  offset),
                }
            },
            1 => {
                match offset {
                  0 => self.getHSTS(),
                  1 => self.read_response(),
                  // 2 => {},
                  3 => self.hinsts.0,
                  _  => panic!("Unhandled cdrom load {:?}, bank: {}, offset: {:08x}", T::width(),self.hsts.current_bank(),  offset),
                }
            },
            2 => {
                match offset {
                  0 => self.getHSTS(),
                  1 => self.read_response(),
                  // 2 => {},
                  // 3 => {},
                  _  => panic!("Unhandled cdrom load {:?}, bank: {}, offset: {:08x}", T::width(),self.hsts.current_bank(),  offset),
                }
            },
            3 => {
                match offset {
                  0 => self.getHSTS(),
                  1 => self.read_response(),
                  // 2 => {},
                  3 => self.hinsts.0,
                  _  => panic!("Unhandled cdrom load {:?}, bank: {}, offset: {:08x}", T::width(),self.hsts.current_bank(),  offset),
                }
            },
            _ => unreachable!("banks are 0..4"),
        };
        T::from_u32(result as u32)
    }
    pub fn getHSTS(&mut self) -> u8 {
        self.hsts.set_param_write_ready(self.param_idx < 16);
        self.hsts.set_param_empty(self.param_idx == 0);
        self.hsts.0
    }

    pub fn set_hclrctl(&mut self, v: u8) {
        let x = HCLRCTL(v);
        self.hinsts.set_intsts(self.hinsts.intsts() & !x.clrint());
        self.hinsts.set_bfempt(self.hinsts.bfempt() & !x.clrbfempt());
        self.hinsts.set_bfwrdy(self.hinsts.bfwrdy() & !x.clrbfwrdy());
    // enbfempt, _: 3;
    // enbfwrdy, _: 4;
        if x.clrint() != 0 || x.clrbfempt() || x.clrbfwrdy() {
            self.param_idx = 0;
        }
//         Setting bits 0-4 resets the corresponding flags in HINTSTS; 
        //         normally one should write 07h to reset the HC05 interrupt flags, or 1Fh to acknowledge all IRQs. 
        //         Acknowledging individual HC05 flags (e.g. writing 01h to change INT3 to INT2) is possible, 
        //         if completely useless. 
        //         After acknowledge, the result FIFO is drained and if there's been a pending command, 
        //         then that command gets send to the controller.
// Setting CHPRST will result in a complete reset of the decoder. Unclear if this also reboots the HC05 and CD-ROM DSP (the decoder has an "external reset" pin which is pulled low when setting CHPRST).
    }

    pub fn read_response(&mut self) -> u8 {
        let mut res = self.response_buffer[self.response_read_idx];
        self.response_read_idx += 1;
        if self.response_read_idx == self.response_idx {
            self.hsts.set_result_read_ready(false);
        }
        if self.response_read_idx >= self.response_idx {
            res = 0;
        }
        if self.response_read_idx == 16 {
            self.response_read_idx = 0;
        }
        res
        // clears RSLRRDY
    }

    pub fn tick(memory_bus: &mut MemoryBus) {
        if   (memory_bus.cdrom.hintmsk.enint() & memory_bus.cdrom.hinsts.intsts() != 0) 
          || (memory_bus.cdrom.hintmsk.enbfwrdy() & memory_bus.cdrom.hinsts.bfwrdy())
          || (memory_bus.cdrom.hintmsk.enbfempt() & memory_bus.cdrom.hinsts.bfempt()) {
            memory_bus.irqctl.status.set_cdrom(true);
        }
        if let Some((cycles, irq, data, len)) = memory_bus.cdrom.pending_interrupt {
            memory_bus.scheduler.schedule(crate::scheduler::Event::CDRom(irq, data, len), cycles, None); // TODO: ????
            memory_bus.cdrom.pending_interrupt = None;
        }
    }

    pub fn process_interrupt(memory_bus: &mut MemoryBus, irq: u8, response: [u8; 16], n: usize) {
        memory_bus.cdrom.hinsts.set_intsts(irq);
        memory_bus.cdrom.response_idx = n;
        memory_bus.cdrom.response_read_idx = 0; // TODO: clear ready bit...
        memory_bus.cdrom.response_buffer.copy_from_slice(&response);
        memory_bus.cdrom.hsts.set_result_read_ready(true);

        if   memory_bus.cdrom.hintmsk.enint() & memory_bus.cdrom.hinsts.intsts() != 0 {
            memory_bus.irqctl.status.set_cdrom(true);
        }
        
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
    #[derive(Default)]
    pub struct Status(u8);
    _, set_play: 7;
    _, set_seek: 6;
    _, set_read: 5;
    _, set_shell_open: 4;
    _, set_id_error: 3;
    _, set_seek_error: 2;
    _, set_spindle_motor: 1;
    _, set_error: 0;
}

bitfield::bitfield! {
    #[derive(Default)]
    pub struct HINTSTS(u8);
    intsts, set_intsts: 2,0;
    bfempt, set_bfempt: 3;
    bfwrdy, set_bfwrdy: 4;
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
    speed, _: 7;
    xa_adpcm, _: 6;
    sector_size, _: 5;
    ignore_bit, _: 4;
    xa_filter, _: 3;
    report, _: 2;
    auto_pause, _: 1;
    cdda, _: 0;
}

pub const AVG_1ST_RESP_GENERIC: u64 = 0xC4E1;
pub const AVG_1ST_RESP_INIT: u64 = 0x13CCE;

pub const AVG_2ND_RESP_GET_ID: u64 = 0x4A00;
pub const AVG_2ND_RESP_PAUSE: u64 = 0x0021_181C;
pub const AVG_2ND_RESP_SEEKL: u64 = 0x6E1CD;

pub const AVG_RATE_INT1: u64 = 0x6E1CD;

