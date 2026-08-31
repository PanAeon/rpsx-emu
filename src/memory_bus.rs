
use std::thread::JoinHandle;

use crossbeam::channel::{Receiver, Sender};

use crate::{bios::Bios, cdrom::CDRom, dma::{Direction, Dma, Port, Step, Sync}, gpu::{self, Gpu, GpuMsg}, irq::InterruptController, mdec::Mdec, ram::Ram, scheduler::Scheduler, scratchpad::Scratchpad, sio::Sio, spu::Spu, timers::Timers};

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
    pub const BIOS: Range = Range(0x1fc00000, 512 * 1024);
    pub const MEM_CTRL: Range = Range(0x1f801000, 36);
    pub const RAM_SIZE: Range = Range(0x1f801060, 4);
    pub const CACHE_CONTROL: Range = Range(0xfffe0130, 4);
    pub const RAM: Range = Range(0x0000_0000, 8 * 1024 * 1024);
    pub const SCRATCHPAD: Range = Range(0x1f80_0000, 1024);
    pub const SPU: Range = Range(0x1f801c00, 640);
    pub const EXPANSION_1: Range = Range(0x1f000000, 512 * 1024);
    pub const EXPANSION_2: Range = Range(0x1f802000, 66);
    // pub const EXPANSION_2: Range = Range(0x1f802000, 8*1024);
    pub const IRQ_CONTROL: Range = Range(0x1f801070, 8);
    pub const TIMERS: Range = Range(0x1F801100, 0x30);
    pub const DMA: Range = Range(0x1f801080, 0x80);
    pub const GPU: Range = Range(0x1f801810, 8);
    pub const JOYSTICK: Range = Range(0x1f801040, 32);
    pub const CDROM: Range = Range(0x1f801800, 4);
    pub const MDEC: Range = Range(0x1F80_1820, 8);
}

#[derive(PartialEq, Eq, Debug)]
pub enum AccessWidth {
    Byte = 1,
    Halfword = 2,
    Word = 4
}

pub trait Addressable {
    fn width() -> AccessWidth;
    fn from_u32(x:u32) -> Self;
    fn as_u32(&self) -> u32;
}

impl Addressable for u8 {
    fn width() -> AccessWidth {
        AccessWidth::Byte
    }
    fn from_u32(x:u32) -> Self {
        x as u8
    }
    fn as_u32(&self) -> u32 {
       *self as u32 
    }
}

impl Addressable for u16 {
    fn width() -> AccessWidth {
        AccessWidth::Halfword
    }
    fn from_u32(x:u32) -> Self {
        x as u16
    }
    fn as_u32(&self) -> u32 {
       *self as u32
    }
}

impl Addressable for u32 {
    fn width() -> AccessWidth {
        AccessWidth::Word
    }
    fn from_u32(x:u32) -> Self {
        x as u32
    }
    fn as_u32(&self) -> u32 {
       *self as u32
    }
}

pub struct MemoryBus {
    bios: Bios,
    pub ram: Ram,
    scratchpad: Scratchpad,
    pub dma: Dma,
    // pub gpu: Gpu,
    pub spu: Spu,
    pub irqctl: InterruptController,
    pub scheduler: Scheduler,
    pub timers: Timers,
    pub cdrom: CDRom,
    pub sio: Sio,
    pub mdec: Mdec,
    pub gpu_sender: Sender<GpuMsg>,
    pub gpu_receiver: Receiver<u32>,
    pub gpu_ctrl_receiver: Receiver<(usize, usize)>,
    pub gpu_handle: JoinHandle<()>
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

impl MemoryBus {
    pub fn new(bios: Bios, ram: Ram, scratchpad: Scratchpad, dma: Dma, spu: Spu, irqctl: InterruptController,
        scheduler: Scheduler, timers: Timers, cdrom: CDRom, sio: Sio, mdec: Mdec, gpu_sender: Sender<GpuMsg>, gpu_receiver: Receiver<u32>, gpu_ctrl_receiver: Receiver<(usize, usize)>, gpu_handle: JoinHandle<()>) -> MemoryBus {
    
        MemoryBus { bios, ram, scratchpad, dma, spu, irqctl, scheduler, timers, cdrom, sio, mdec,
        gpu_sender, gpu_receiver, gpu_ctrl_receiver, gpu_handle}
    }
    pub fn load<T:Addressable>(&mut self, addr: u32) -> T {
        let address = mask_region(addr);
        if !address.is_multiple_of(T::width() as u32) {
            panic!("unaligned load{:?} address: {:08x}", T::width(), address)
        }
        if let Some(offset) = map::RAM.contains(address) {
            return self.ram.load(offset);
        }
        if let Some(offset) = map::GPU.contains(address) {
            return gpu::load(self, offset);
            // return  self.gpu.load(offset)
        }
        if let Some(_) = map::SPU.contains(address) {
            // return crate::spu::load(&self.spu, addr);
            return self.spu.load(addr);
            // return T::from_u32(0);
        }
        if let Some(offset) = map::JOYSTICK.contains(address) {
            // println!("Unhandled read from Joystick register {:x}", addr);
            // return T::from_u32(0);
            return self.sio.load(offset);
        }
        if let Some(offset) = map::DMA.contains(address) {
            // println!("DMA read32 {:x}", addr);
            return self.dma_reg(offset);
        }
        if let Some(offset) = map::SCRATCHPAD.contains(address) {
            return self.scratchpad.load(offset);
        }
        if let Some(offset) = map::IRQ_CONTROL.contains(address) {
            return self.irqctl.load(offset);
        }
        if let Some(offset) = map::CDROM.contains(address) {
            return self.cdrom.load(offset);
            // return println!("Unhandled write to TIMERS register {:x} = {:x}", offset, value.as_u32());
        }
        if let Some(offset) = map::TIMERS.contains(address) {
            return crate::timers::load(self, offset);
            // println!("Unhandled read from TIMERS register {:x}", offset);
            // return T::from_u32(0);
        }
        if let Some(offset) = map::BIOS.contains(address) {
            return self.bios.load(offset);
        }
        if let Some(offset) = map::MDEC.contains(address) {
            return self.mdec.load(offset);
        }
        if let Some(offset) = map::EXPANSION_1.contains(address) {
            return T::from_u32(!0);
        }
        if let Some(offset) = map::EXPANSION_2.contains(address) {
            return T::from_u32(!0);
        }
        if let Some(offset) = map::MEM_CTRL.contains(address) {
            println!("Unhandled read from memctrl register {:x}", offset);
            return T::from_u32(0xFF);
        }
        if let Some(_) = map::RAM_SIZE.contains(address) {
            return T::from_u32(0x00000B88);
        }
        // if address >= 0xfffff000 {
        //     println!("Unhandled read from ??? address {:x}", address);
        //     return T::from_u32(0);
        // }
        panic!("Unhandled load{:?} address: {:08x}", T::width(), address)
    }

    pub fn store<T:Addressable>(&mut self, addr: u32, value: T) {
        let address = mask_region(addr);
        if !address.is_multiple_of(T::width() as u32) {
            panic!("unaligned store{:?} address: {:08x}", T::width(), address)
        }
        if let Some(offset) = map::RAM.contains(address) {
            return self.ram.store(offset, value);
        }
        if let Some(offset) = map::GPU.contains(address) {
            gpu::store(self, offset, value);
            // self.gpu.store(offset, value);
            // println!("GPU store32 {:x} = {:x}", offset, value);
            return;
        }
        if let Some(offset) = map::DMA.contains(address) {
            // println!("DMA store32 {:x} = {:x}", addr, value);
            return self.set_dma_reg(offset, value);
        }
        if let Some(offset) = map::SCRATCHPAD.contains(address) {
            return self.scratchpad.store(offset, value);
        }
        if let Some(_) = map::SPU.contains(address) {
            // println!("Unhandled write to SPU register {:x}", addr);
            // return;
            // return crate::spu::store(&mut self.spu, address, value);
            return self.spu.store(address, value);
        }
        if let Some(offset) = map::JOYSTICK.contains(address) {
            return crate::sio::Sio::store(self, offset, value);
            // println!("Unhandled write to Joystick register {:x}", addr);
            // return;
        }
        if let Some(offset) = map::TIMERS.contains(address) {
            return crate::timers::store(self, offset, value);
            // return println!("Unhandled write to TIMERS register {:x} = {:x}", offset, value.as_u32());
        }
        if let Some(offset) = map::CDROM.contains(address) {
            return crate::cdrom::CDRom::store(self, offset, value);
            // return println!("Unhandled write to TIMERS register {:x} = {:x}", offset, value.as_u32());
        }
        if let Some(offset) = map::MDEC.contains(address) {
            return self.mdec.store(offset, value);
        }
        if let Some(offset) = map::MEM_CTRL.contains(address) {
            let val = value.as_u32();
            match offset {
                0 => {
                    if val != 0x1f000000 {
                        panic!("Bad expansion 1 base address 0x{:08X}", val);
                    }
                }
                4 => {
                    if val != 0x1f802000 {
                        panic!("Bad expansion 2 base address 0x{:08X}", val);
                    }
                }
                _ => {}
            }
            return; //println!("Unhandled write to MEM_CTRL register");
        }
        if let Some(_) = map::RAM_SIZE.contains(address) {
            return println!("Unhandled write to RAM_SIZE register");
        }
        if let Some(_) = map::CACHE_CONTROL.contains(address) {
            return println!("Unhandled write to CACHE_CONTROL register");
        }
        if let Some(offset) = map::IRQ_CONTROL.contains(address) {
            return self.irqctl.store(offset, value);
        }
        if let Some(offset) = map::EXPANSION_1.contains(address) {
            println!("Unhandled write to expansion_1 register {:x}", addr);
            return ;
        }
        if let Some(offset) = map::EXPANSION_2.contains(address) {
            println!("Unhandled write to expansion_2 register {:x}", addr);
            return;
        }
        // if address == 0x1f00e1ec {
        //     return;
        // }
        
        panic!("Unhandled store{:?} address: {:08x}", T::width(), address)
    }

    pub fn dma_reg<T:Addressable>(&self, offset: u32) -> T {
        // if T::width() != AccessWidth::Word {
        //     println!("Unhandled {:?} DMA load", T::width())
        // }
        let align = offset & 3;
        let offset = offset & !3;
        let major = (offset & 0x70) >> 4;
        let minor = offset & 0xf;
        let r = match major {
            0..7 => {
                let channel = self.dma.channel(Port::from_index(major as u8));
                match minor {
                    0 => channel.base(),
                    4 => channel.block_control(),
                    8 => channel.control(),
                    _ => panic!("Unhandled DMA read at {:x} offset {major}.{minor}", offset)
                }
            },
            7 => {
                match minor {
                    0 => self.dma.control(),
                    4 => self.dma.interrupt(),
                    _ => panic!("Unhandled DMA read at {:x} offset {major}.{minor}", offset)
                }
            },
            _ => panic!("Unhandled DMA read at {:x} offset", offset)
        };
        T::from_u32(r >> (align * 8))
    }

    pub fn set_dma_reg<T:Addressable>(&mut self, offset: u32, val: T) {
        // if T::width() != AccessWidth::Word {
        //     panic!("Unhandled {:?} DMA store", T::width())
        // }
        let align = offset & 3;
        let offset = offset & !3;
        let value = val.as_u32();
        let value = value << (align * 8);
        let major = (offset & 0x70) >> 4;
        let minor = offset & 0xf;
        let active_port = match major {
            0..7 => {
                let port = Port::from_index(major as u8);
                let channel = self.dma.channel_mut(port);
                match minor {
                    0 => channel.set_base(value),
                    4 => channel.set_block_control(value),
                    8 => channel.set_control(value),
                    _ => panic!("Unhandled DMA read at {:x} offset", offset)
                }
                if channel.active() {
                    Some(port)
                } else {
                    None
                }
            },
            7 => {
                match minor {
                    0 => self.dma.set_control(value),
                    4 => crate::dma::Dma::set_interrupt(self, value),
                    _ => panic!("Unhandled DMA read at {:x} offset", offset)
                }
                None
            },
            _ => panic!("Unhandled DMA read at {:x} offset", offset)
        };
        if let Some(port) = active_port {
            self.do_dma(port)
        }
    }

    pub fn do_dma(&mut self, port: Port) {
        match self.dma.channel(port).sync() {
            Sync::LinkedList => self.do_dma_linked_list(port),
            _ => self.do_dma_block(port)
        }
    }
    pub fn do_dma_block(&mut self, port: Port) {
        let channel = self.dma.channel(port);

        let increment  = match channel.step() {
            Step::Increment => 4,
            Step::Decrement => -4i32 as u32,
        };
        let mut addr = channel.base();

        // transfer size in words
        let mut remsz = match channel.transfer_size() {
            Some(n) => n,
            None => panic!("Couldn't figure DMA block transfer size")
        };

        while remsz > 0 {
            let cur_addr = addr & 0x1ffffc;
            match channel.direction() {
                Direction::FromRam => {
                    let src_word = self.ram.load::<u32>(cur_addr);
                    match port {
                        Port::Gpu => {
                            gpu::store(self, 0, src_word);
                            // self.gpu.gp0(src_word);
                            // println!("GPU data: {:08x}", src_word);
                        },
                        Port::Spu => {
                            self.spu.ram_write::<4>(src_word);
                            // println!("SPU data: {:08x}", src_word);
                        },
                        Port::MdecIn => {
                            self.mdec.store(0, src_word);
                        }
                        _ => panic!("Unhandled DMA destination port {}", port as u8)
                    }
                },
                Direction::ToRam => {
                    let src_word = match port {
                        Port::Otc => match remsz {
                            1 => 0xffffff, // end of table marker
                            _ => addr.wrapping_sub(4) & 0x1fffff, // pointer to the prev entry
                        },
                        Port::Gpu => {
                            gpu::load(self, 0)
                            // self.gpu.read()
                        },
                        Port::CdRom => {
                            self.cdrom.load::<u32>(2)
                        },
                        Port::MdecOut => {
                            self.mdec.load(0)
                        },
                        _ => panic!("Unhandled DMA source port {}", port as u8)
                    };
                    self.ram.store::<u32>(cur_addr, src_word);
                }
            };



            addr = addr.wrapping_add(increment);
            remsz -= 1;
        }
        Dma::done(self, port);
    }
    pub fn do_dma_linked_list(&mut self, port: Port) {
        let channel = self.dma.channel_mut(port);
        let mut addr = channel.base() & 0x1ffffc;

        if channel.direction() == Direction::ToRam {
            panic!("Invalid channel direction ToRam for linkedList mode");
        }

        if port != Port::Gpu {
            panic!("Attempted linked list DMA on port {}", port as u8);
        }

        loop {
            let header = self.ram.load::<u32>(addr);

            let mut remsz = header >> 24;

            while remsz > 0 {
                addr = (addr + 4) & 0x1ffffc;
                let command = self.ram.load::<u32>(addr);

                // println!("GPU command: {:08X}", command);
                gpu::store(self, 0, command);
                // self.gpu.gp0(command);
                remsz -= 1;
            }
            if header & 0x800000 != 0 {
                break;
            }
            addr = header & 0x1ffffc;
        }

        Dma::done(self, port);
        // channel.done();
    }
}
