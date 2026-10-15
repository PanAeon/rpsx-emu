use std::pin::Pin;

use crate::system::{Addressable, System};
use serde::{Deserialize, Deserializer, Serialize, Serializer};


pub struct Ram {
    pub data: &'static mut [u8]
}

impl Ram {
    pub fn new(s: &'static mut [u8]) -> Ram {
        // let data = vec![0xca; 2 * 1024 * 1024].into_boxed_slice();
        for i in 0..2 * 1024 * 1024 {
            s[i] = 0xca;
        }
        
        Ram { data: s }
    }


    pub fn load<T:Addressable>(&self, address: u32) -> T {
        let width = T::width() as usize;
        let addr = address as usize;
        let addr = addr & 0x1f_ffff;
        let mut buffer = [0;4];
        buffer[..width].copy_from_slice(&self.data[addr..addr+width]);
        T::from_u32(u32::from_le_bytes(buffer))
    }

    pub fn store<T:Addressable>(system: &mut System, address: u32, value: T) {
        let ram = &mut system.ram;
        let width = T::width() as usize;
        let addr = address as usize;
        let addr = addr & 0x1f_ffff;
        let bytes = value.as_u32().to_le_bytes();

        ram.data[addr..addr+width].copy_from_slice(&bytes[..width]);
        system.block_cache.lock().unwrap().invalidate_ram(addr as u32);
    }
}

impl Serialize for Ram {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer {
        let bytes = serde_bytes::Bytes::new(&self.data);
        bytes.serialize(serializer)
    }
}

// impl<'de> Deserialize<'de> for Ram {
//     fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
//     where
//         D: Deserializer<'de> {
//         let bytes = serde_bytes::ByteBuf::deserialize(deserializer)?;
//         let data = bytes.into_boxed_slice();
//         Ok(Ram { data: Box::into_pin(data) })
//     }
// }
