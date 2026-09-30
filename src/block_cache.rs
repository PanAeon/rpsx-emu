use crate::system::map;

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct CacheEntry {
    pub block_idx: u32,
    pub has_block: bool,
    pub protected: bool,
    pub dirty: bool,
}

// const _: () = assert!(core::mem::size_of::<CacheEntry>() == 4);

pub struct Block {
    pub ptr: *const u8,
    pub crc32: u32,
    pub length: u32,
}

// we got RAM 2mb and bios 512kb.
pub struct BlockCache {
    bios: Box<[CacheEntry]>,
    ram: Box<[CacheEntry]>,
    pub blocks: Vec<Block>
}

impl BlockCache {

    pub fn new() -> BlockCache {
        BlockCache {
            bios: vec![CacheEntry::default();128*1024].into_boxed_slice(),
            ram: vec![CacheEntry::default();2 * 256 * 1024].into_boxed_slice(),
            blocks: vec![] 
        }

    }

    pub fn get_entry_mut(&mut self, addr: u32) -> &mut CacheEntry {
        let address = mask_region(addr);
        if !address.is_multiple_of(4) {
            panic!("unaligned load{:?} address: {:08x}", 4, address)
        }
        if let Some(offset) = map::RAM.contains(address) {
            return &mut self.ram[(offset / 4) as usize];
        }
        if let Some(offset) = map::BIOS.contains(address) {
            return &mut self.bios[(offset / 4) as usize];
        }
        panic!("unhandled load{:?} address: {:08x}", 4, address)
    }

    pub fn get_entry(&self, addr: u32) -> Option<CacheEntry> {
        let address = mask_region(addr);
        if !address.is_multiple_of(4) {
            panic!("unaligned load{:?} address: {:08x}", 4, address)
        }
        if let Some(offset) = map::RAM.contains(address) {
            let entry = self.ram[(offset / 4) as usize];
            return entry.has_block.then(|| entry)
        }
        if let Some(offset) = map::BIOS.contains(address) {
            let entry = self.bios[(offset / 4) as usize];
            return entry.has_block.then(|| entry)
        }
        panic!("unhandled load{:?} address: {:08x}", 4, address)
    }

    pub fn insert_block(&mut self, addr: u32, block: Block) -> u32 {
        let idx = self.blocks.len();
        let block_len = block.length;
        self.blocks.push(block);

        let entry = self.get_entry_mut(addr);
        entry.block_idx = idx as u32;
        entry.has_block = true;
        entry.protected = true;
        entry.dirty = false;
        for i in 1..block_len {
            let entry = self.get_entry_mut(addr + (i * 4) as u32);
            entry.protected = true;
            entry.dirty = false;
        }


        idx as u32

        // &self.blocks[idx as usize]
    }

    pub fn get_block(&self, idx: u32) -> &Block {
        &self.blocks[idx as usize]
    }

    // wrooooong...
    pub fn invalidate_ram(&mut self, offset: u32, width: usize) {
        let idx = (offset / 4) as usize;
        let entry =  self.ram[idx];
        if entry.protected {
            println!("protected mem overwrite!");
            for i in 0..self.ram.len() {
                self.ram[i].protected = false;
                self.ram[i].has_block = false;
            }
            // entry.protected = false;
            // let start = idx.saturating_sub(256);
            // for i in start..=idx {
            //     let entry = &mut self.ram[idx];
            //     if entry.protected {
            //         let block = &mut self.blocks[entry.block_idx as usize];
            //         if i + block.length as usize >= idx {
            //             entry.dirty = true;
            //             // compute crc32 in ideal scenario
            //         }
            //     }
            // }
            // panic!("dirty code!");
        }
    }
}

const REGION_MASK: [u32; 8] = [
    // KUSEG: 2048MB,
    0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff, // KSEG0: 512MB,
    0x7fffffff, // KSEG1: 512MB,
    0x1fffffff, // KSEG2: 1024MB,
    0xffffffff, 0xffffffff,
];
pub fn mask_region(addr: u32) -> u32 {
    let index = (addr >> 29) as usize;
    addr & REGION_MASK[index]
}
