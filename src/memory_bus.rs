use crate::{bios::Bios, ram::Ram};

mod map {
    pub struct Range(u32, u32);

    impl Range {
        pub fn contains(self, addr: u32) -> Option<u32> {
            let Range(start, length) = self;

            if addr >= start && addr < start + length {
                Some(addr - start)
            } else {
                None
            }
        }
    }
    pub const BIOS: Range = Range(0xbfc00000, 512 * 1024);
    pub const MEM_CTRL: Range = Range(0x1f801000, 36);
    pub const RAM_SIZE: Range = Range(0x1f801060, 4);
    pub const CACHE_CONTROL: Range = Range(0xfffe0130, 4);
    pub const RAM: Range = Range(0xa000_0000, 2*1024*1024);
}

pub struct MemoryBus {
    bios:Bios,
    ram: Ram
}

impl MemoryBus {
    pub fn new(bios: Bios, ram: Ram) -> MemoryBus {
        MemoryBus { bios, ram}
    }

    pub fn load32(&self, address: u32) -> u32 {
        if address % 4 != 0 {
            panic!("unaligned load32 address: {:08x}", address)
        }
        // bios
        if let Some(offset) = map::BIOS.contains(address) {
            return self.bios.load32(offset)
        }
        if let Some(offset) = map::RAM.contains(address) {
            return self.ram.load32(offset)
        }
        panic!("Unhandled load32 address: {:08x}", address)

    }
    pub fn store32(&mut self, address: u32, value: u32) {
        if address % 4 != 0 {
            panic!("unaligned load32 address: {:08x}", address)
        }
        if let Some(offset) = map::RAM.contains(address) {
            return self.ram.store32(offset, value);
        }
        if let Some(offset) = map::MEM_CTRL.contains(address) {
            match offset {
                0 => if value != 0x1f000000 {
                    panic!("Bad expansion 1 base address 0x{:08X}", value);
                },
                4 => if value != 0x1f802000 {
                    panic!("Bad expansion 2 base address 0x{:08X}", value);
                },
                _ => {}
            }
            return println!("Unhandled write to MEM_CTRL register")
        }
        if let Some(_) = map::RAM_SIZE.contains(address) {
            return println!("Unhandled write to RAM_SIZE register")
        }
        if let Some(_) = map::CACHE_CONTROL.contains(address) {
            return println!("Unhandled write to CACHE_CONTROL register")
        }
        panic!("Unhandled store32 address: {:08x}", address)
    }
    
}
