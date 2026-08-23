pub struct Gte {
    pub ofx: i32,
    pub ofy: i32,
    pub h: u16,
    pub dqa: i16,
    pub dqb: i32,
    pub zsf3: i16,
    pub zsf4: i16,
    pub matrices: [[[i16; 3]; 3]; 3],
    pub control_vectors: [[i32; 3]; 4],
    pub flags: u32,

    pub v: [[i16; 3]; 4],
    pub mac: [i32; 4],
    pub otz: u16,
    pub rgb: (u8, u8, u8, u8),
    pub ir: [i16; 4],
    pub xy_fifo: [(i16, i16); 4],
    pub z_fifo: [u16; 4],
    pub rgb_fifo: [(u8, u8, u8, u8); 3],
    pub lzcs: u8,
    pub reg_23: u32,
}


impl Default for Gte {
    fn default() -> Self {
        Self {
            ofx: 0,
            ofy: 0,
            h: 0,
            dqa: 0,
            dqb: 0,
            zsf3: 0,
            zsf4: 0,
            matrices: [[[0; 3]; 3]; 3],
            control_vectors: [[0; 3]; 4],
            flags: 0,
            v: [[0; 3]; 4],
            mac: [0; 4],
            otz: 0,
            rgb: (0, 0, 0, 0),
            ir: [0; 4],
            xy_fifo: [(0, 0); 4],
            z_fifo: [0; 4],
            rgb_fifo: [(0, 0, 0, 0); 3],
            lzcs: 32,
            reg_23: 0,
        }
    }
}

impl Gte {
    pub fn control(&self, reg: u8) -> u32 {
        match reg {
            13..=15 => {
                let index = ControlVector::BackgroundColor.index();
                let vector = &self.control_vectors[index];
                vector[reg as usize - 13] as u32
            },
            21..=23 => {
                let index = ControlVector::FarColor.index();
                let vector = &self.control_vectors[index];
                vector[reg as usize - 21] as u32
            },
            
            24 => self.ofx as u32,
            25 => self.ofy as u32,
            26 => self.h as i16 as u32,
            27 => self.dqa as u32,
            28 => self.dqb as u32,
            29 => self.zsf3 as u32,
            30 => self.zsf4 as u32,
            31 => self.flags, // TODO: u20...
            x => panic!("unhandled read from the cop2c {} register", x),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ControlVector {
    Translation = 0,
    BackgroundColor = 1,
    FarColor = 2,
    // always equal to 0
    Zero = 3
}

impl ControlVector {
    pub fn index(self) -> usize {
        self as usize
    }
}
