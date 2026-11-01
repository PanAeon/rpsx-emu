use serde::{Deserialize, Serialize};

use crate::system::Addressable;


#[derive(Serialize, Deserialize)]
pub struct Sio2 {
    mode: u16,
    ctrl: u16,
    misc: u16,
    baud: u16,
    // status: u16,
    // transfer: Option<u8>,
    // received: ArrayVec<u8, 8>,

    // move out?
    // state: State,
    // pub gamepad1: Gamepad,
    // pub gamepad2: Gamepad,
    // pub memcard1: Memcard,
    // pub memcard2: Memcard,
}
impl Sio2 {
    pub fn new() -> Sio2 {
        Sio2 { ctrl: 0, baud: 0, mode: 0, misc: 0 }
    }
    pub fn load<T: Addressable>(&self, offset: u32) -> T {
        let res = match offset {
            0x0 => 0xff, // data, disconnected RX line idles high
            0x4 => 0x05, // TX FIFO not full, TX idle, RX empty, no IRQ (stat)
            0x8 => self.mode,
            0xA => self.ctrl,
            0xC => self.misc,
            0xE => self.baud,
            _ => panic!("unimplemented")
        };
        T::from_u32(res as u32)
    }
    pub fn store<T: Addressable>(system: &mut crate::system::System, offset: u32, value: T) {
        let sio = &mut system.sio2;
        let val = value.as_u32() as u16;
        match offset {
            0x0 => {}, // data, no serial peer attached
            0x8 => {sio.mode = val & 0x01ff}
            0xA => sio.write_ctrl(val),
            0xC => sio.misc = val,
            0xE => sio.baud = val,
            _ => panic!("unimplemented")
        }
    }

    fn write_ctrl(&mut self, val: u16) {
        if (val & (1 << 6)) != 0 {
            self.mode = 0;
            self.ctrl = 0;
            self.misc = 0;
            self.baud = 0;
            return;
        }

        self.ctrl = val & 0x3fff;
        self.ctrl &= !(1 << 4); // acknowledge is write-only
    }

}
