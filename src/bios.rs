use std::{fs::File, io::{Error, Read}, path::Path};


pub struct Bios {
    pub data: Vec<u8>
}

const BIOS_SIZE: u64 = 512 * 1024;
impl Bios {
    pub fn new(path: &Path) -> Result<Bios, Error> {
        let file = File::open(path)?;
        let mut data: Vec<u8> = Vec::new();
        file.take(BIOS_SIZE).read_to_end(&mut data);
        if data.len() == BIOS_SIZE as usize {
            Ok(Bios { data })
        } else {
            Err(Error::new(std::io::ErrorKind::InvalidInput, "Invalid BIOS size."))

        }
    }

    pub fn load32(&self, offset: u32) -> u32 {
        let offset = offset as usize;

        let b0 = self.data[offset + 0] as u32;
        let b1 = self.data[offset + 1] as u32;
        let b2 = self.data[offset + 2] as u32;
        let b3 = self.data[offset + 3] as u32;

        b0 | (b1 << 8) | (b2 << 16) | (b3 << 24)
    }


}
