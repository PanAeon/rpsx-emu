use std::pin::Pin;

use crate::system::{Addressable, System};


pub struct Ram {
    pub data: Pin<Box<[u8]>>
}

impl Ram {
    pub fn new() -> Ram {
        let data = vec![0xca; 2 * 1024 * 1024].into_boxed_slice();
        
        Ram { data: Box::into_pin(data) }
    }


    pub fn load<T:Addressable>(&self, address: u32) -> T {
        let width = T::width() as usize;
        let addr = address as usize;
        let mut buffer = [0;4];
        buffer[..width].copy_from_slice(&self.data[addr..addr+width]);
        T::from_u32(u32::from_le_bytes(buffer))
    }

    pub fn store<T:Addressable>(system: &mut System, address: u32, value: T) {
        let ram = &mut system.ram;
        let width = T::width() as usize;
        let addr = address as usize;
        let bytes = value.as_u32().to_le_bytes();

        ram.data[addr..addr+width].copy_from_slice(&bytes[..width]);
        let mut bc = system.block_cache.lock().expect("ok");
        bc.invalidate_ram(address, width);
    }
}
