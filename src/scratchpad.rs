use crate::memory_bus::Addressable;


pub struct Scratchpad {
    pub data: Vec<u8>
}

impl Scratchpad {
    pub fn new() -> Scratchpad {
        let data = vec![0xca; 1 * 1024];
        Scratchpad { data }
    }


    pub fn load<T:Addressable>(&self, address: u32) -> T {
        let width = T::width() as usize;
        let addr = address as usize;
        let mut buffer = [0u8;4];
        buffer[..width].copy_from_slice(&self.data[addr..addr+width]);
        T::from_u32(u32::from_le_bytes(buffer))
    }

    pub fn store<T:Addressable>(&mut self, address: u32, value: T) {
        let width = T::width() as usize;
        let addr = address as usize;
        let bytes = value.as_u32().to_le_bytes();

        self.data[addr..addr+width].copy_from_slice(&bytes[..width]);
    }

    // pub fn load8(&self, offset: u32) -> u8 {
    //     let offset = offset as usize;
    //     self.data[offset]
    // }
    // pub fn load16(&self, offset: u32) -> u16 {
    //     let offset = offset as usize;
    //
    //     let b0 = self.data[offset + 0] as u16;
    //     let b1 = self.data[offset + 1] as u16;
    //
    //     b0 | (b1 << 8)
    // }
    // pub fn load32(&self, offset: u32) -> u32 {
    //     let offset = offset as usize;
    //
    //     let b0 = self.data[offset + 0] as u32;
    //     let b1 = self.data[offset + 1] as u32;
    //     let b2 = self.data[offset + 2] as u32;
    //     let b3 = self.data[offset + 3] as u32;
    //
    //     b0 | (b1 << 8) | (b2 << 16) | (b3 << 24)
    // }

    // pub fn store8(&mut self, offset: u32, val: u8) {
    //     let offset = offset as usize;
    //     self.data[offset] = val;
    // }
    // pub fn store16(&mut self, offset: u32, val: u16) {
    //     let offset = offset as usize;
    //
    //     let b0 = val as u8;
    //     let b1 = (val >> 8) as u8;
    //
    //     self.data[offset + 0] = b0;
    //     self.data[offset + 1] = b1;
    //
    // }
    // pub fn store32(&mut self, offset: u32, val: u32) {
    //     let offset = offset as usize;
    //
    //     let b0 = val as u8;
    //     let b1 = (val >> 8) as u8;
    //     let b2 = (val >> 16) as u8;
    //     let b3 = (val >> 24) as u8;
    //
    //     self.data[offset + 0] = b0;
    //     self.data[offset + 1] = b1;
    //     self.data[offset + 2] = b2;
    //     self.data[offset + 3] = b3;
    //
    // }
}
