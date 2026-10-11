use crate::system::Addressable;
use serde::{Deserialize, Serialize};


#[derive(Serialize, Deserialize)]
pub struct Scratchpad {
    #[serde(with = "serde_bytes")]
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
}
