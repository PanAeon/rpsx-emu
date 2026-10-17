use std::{collections::HashSet, pin::Pin};

use crate::system::{System, map, mask_region};

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct CacheEntry {
    pub block_idx: Option<u32>,
}

pub struct Page {
    blocks: Vec<u32>, // blockid
    protected: bool,
}

pub struct Block {
    pub ptr: *const u8,
    pub addr: u32,
    pub length: u32,
    pub mem: Vec<u8>,
    pub dirty: bool,
}

unsafe impl Send for Block {}

// we got RAM 2mb and bios 512kb.
pub struct BlockCache {
    bios: Box<[CacheEntry]>,
    pub ram: Pin<Box<[CacheEntry]>>,
    pub blocks: Vec<Block>,
    pub in_flight: HashSet<u32>,
    pub pages: Vec<Page>,
}

impl BlockCache {
    pub fn new() -> BlockCache {
        let mut pages = Vec::with_capacity(512);
        for _ in 0..512 {
            pages.push(Page {
                blocks: vec![],
                protected: false,
            })
        }
        BlockCache {
            bios: vec![CacheEntry::default(); 128 * 1024].into_boxed_slice(),
            ram: Box::into_pin(vec![CacheEntry::default(); 2 * 256 * 1024].into_boxed_slice()),
            blocks: vec![],
            in_flight: HashSet::new(),
            pages,
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
            return entry.block_idx.map(|_| entry);
        }
        if let Some(offset) = map::BIOS.contains(address) {
            let entry = self.bios[(offset / 4) as usize];
            return entry.block_idx.map(|_| entry);
        }
        panic!("unhandled load{:?} address: {:08x}", 4, address)
    }

    pub fn is_block_dirty(&self, idx: u32) -> bool {
        self.blocks[idx as usize].dirty
    }

    pub fn mark_fresh(&mut self, idx: u32) {
        let block = &mut self.blocks[idx as usize];
        block.dirty = false;
        // we need to protect the page again..
        let addr = mask_region(block.addr);
        if let Some(_) = crate::system::map::RAM.contains(addr) {
            let first_page = addr >> 12; // */4096;
            let last_page = (addr + block.length) >> 12; // */4096;
            for p in first_page..=last_page {
                self.pages[p as usize].blocks.push(idx);
                if !self.pages[p as usize].protected {
                    protect_page(p as usize);
                }
            }
        }
    }

    pub fn get_block(&self, idx: u32) -> &Block {
        &self.blocks[idx as usize]
    }

    pub fn invalidate_addr(&mut self, addr: u32) {
        let page_idx = addr >> 12;
        let page = &mut self.pages[page_idx as usize];
        for i in &page.blocks {
            self.blocks[*i as usize].dirty = true;
        }
        page.blocks.clear();
        page.protected = false;
    }
}

pub fn is_block_valid(system: &System, block: &Block) -> bool {
    let addr = mask_region(block.addr);
    if let Some(_) = crate::system::map::RAM.contains(addr) {
        let addr = addr as usize;
        let current = &system.ram.data[addr..addr + block.mem.len()];
        &block.mem == current
    } else {
        true // asume bios
    }
}

pub fn insert_block(system: &mut System, addr: u32, block: Block) {
    // first check if the block is still valid...
    if !is_block_valid(system, &block) {
        return;
    }
    let block_len = block.length;
    let idx = system
        .block_cache
        .get_entry(addr)
        .map(|x| x.block_idx)
        .flatten();
    let bc = &mut system.block_cache;
    let idx = if let Some(idx) = idx {
        bc.blocks[idx as usize] = block;
        idx
    } else {
        bc.blocks.push(block);
        bc.blocks.len() as u32 - 1
    };

    let entry = bc.get_entry_mut(addr);
    entry.block_idx = Some(idx);

    // protect ram now.. I guess

    let addr = mask_region(addr);
    if let Some(_) = crate::system::map::RAM.contains(addr) {
        let first_page = addr >> 12; // */4096;
        let last_page = (addr + block_len) >> 12; // */4096;
        for p in first_page..=last_page {
            system.block_cache.pages[p as usize].blocks.push(idx);
            if !system.block_cache.pages[p as usize].protected {
                protect_page(p as usize);
            }
        }
    }
}

fn protect_page(p: usize) {
    crate::fastmem::protect_page(p).expect("page is protected");
}
