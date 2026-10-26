use serde::Serialize;

use crate::{
    bios::Bios,
    block_cache::BlockCache,
    cdrom::CDRom,
    dma::{
        Direction::{self, FromRam},
        Dma, Port, Step, Sync,
    },
    dynarec::Dynarec,
    gpu::Gpu,
    irq::InterruptController,
    mdec::Mdec,
    ram::Ram,
    scheduler::Scheduler,
    scratchpad::Scratchpad,
    sio::Sio,
    spu::Spu,
    timers::Timers,
};

pub mod map {
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
    pub const RAM: Range = Range(0x0000_0000, 8 * 1024 * 1024); // ram is mirrored 4 times..
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
    Word = 4,
}

pub trait Addressable {
    fn width() -> AccessWidth;
    fn from_u32(x: u32) -> Self;
    fn as_u32(&self) -> u32;
}

impl Addressable for u8 {
    fn width() -> AccessWidth {
        AccessWidth::Byte
    }
    fn from_u32(x: u32) -> Self {
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
    fn from_u32(x: u32) -> Self {
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
    fn from_u32(x: u32) -> Self {
        x as u32
    }
    fn as_u32(&self) -> u32 {
        *self as u32
    }
}

pub static mut SYSTEM_ADDR: u64 = 0;

#[derive(Serialize)]
pub struct System {
    #[serde(skip)]
    bios: Bios,
    pub ram: Ram,
    pub scratchpad: Scratchpad,
    pub dma: Dma,
    pub cpu: Dynarec,
    pub spu: Spu,
    pub irqctl: InterruptController,
    pub scheduler: Scheduler,
    pub timers: Timers,
    pub cdrom: CDRom,
    pub sio: Sio,
    pub mdec: Mdec,
    pub gpu: Gpu,
    #[serde(skip)]
    pub block_cache: BlockCache,
}

const REGION_MASK: [u32; 8] = [
    0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff, // KUSEG: 2048MB,
    0x7fffffff, // KSEG0: 512MB,
    0x1fffffff, // KSEG1: 512MB,
    0xffffffff, 0xffffffff, // KSEG2: 1024MB,
];
pub fn mask_region(addr: u32) -> u32 {
    let index = (addr >> 29) as usize;
    addr & REGION_MASK[index]
}

impl System {
    pub fn new(
        cpu: Dynarec,
        bios: Bios,
        ram: Ram,
        scratchpad: Scratchpad,
        dma: Dma,
        spu: Spu,
        irqctl: InterruptController,
        scheduler: Scheduler,
        timers: Timers,
        cdrom: CDRom,
        sio: Sio,
        mdec: Mdec,
        gpu: Gpu,
        block_cache: BlockCache,
    ) -> System {
        System {
            cpu,
            bios,
            ram,
            scratchpad,
            dma,
            spu,
            irqctl,
            scheduler,
            timers,
            cdrom,
            sio,
            mdec,
            gpu,
            block_cache,
        }
    }
    pub fn load<T: Addressable>(&mut self, addr: u32) -> T {
        // println!("self addr: 0x{:X}", self as *const System as u64);
        let address = mask_region(addr);
        if !address.is_multiple_of(T::width() as u32) {
            panic!("unaligned load{:?} address: {:08x}", T::width(), address)
        }
        if let Some(offset) = map::RAM.contains(address) {
            return self.ram.load(offset);
        }
        if let Some(offset) = map::GPU.contains(address) {
            // return gpu::load(self, offset);
            return self.gpu.load(offset);
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
        if let Some(_) = map::EXPANSION_1.contains(address) {
            return T::from_u32(!0);
        }
        if let Some(_) = map::EXPANSION_2.contains(address) {
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
        println!("Unhandled load{:?} address: {:08x}", T::width(), address);
        println!("pc is 0x{:X}", self.cpu.regs.pc);
        panic!("Unhandled load{:?} address: {:08x}", T::width(), address);
        // T::from_u32(0x0)
    }

    pub fn store<T: Addressable>(&mut self, addr: u32, value: T) {
        let address = mask_region(addr);
        if !address.is_multiple_of(T::width() as u32) {
            panic!("unaligned store{:?} address: {:08x}", T::width(), address)
        }
        if let Some(offset) = map::RAM.contains(address) {
            return self.ram.store( offset, value);
        }
        if let Some(offset) = map::GPU.contains(address) {
            // gpu::store(self, offset, value);
            self.gpu.store(offset, value);
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
        if let Some(_) = map::EXPANSION_1.contains(address) {
            println!("Unhandled write to expansion_1 register {:x}", addr);
            return;
        }
        if let Some(_) = map::EXPANSION_2.contains(address) {
            println!("Unhandled write to expansion_2 register {:x}", addr);
            return;
        }
        // if address == 0x1f00e1ec {
        //     return;
        // }

        panic!("Unhandled store{:?} address: {:08x}", T::width(), address)
    }

    pub fn dma_reg<T: Addressable>(&self, offset: u32) -> T {
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
                    _ => panic!("Unhandled DMA read at {:x} offset {major}.{minor}", offset),
                }
            }
            7 => match minor {
                0 => self.dma.control(),
                4 => self.dma.interrupt(),
                _ => panic!("Unhandled DMA read at {:x} offset {major}.{minor}", offset),
            },
            _ => panic!("Unhandled DMA read at {:x} offset", offset),
        };
        T::from_u32(r >> (align * 8))
    }

    pub fn set_dma_reg<T: Addressable>(&mut self, offset: u32, val: T) {
        // if T::width() != AccessWidth::Word {
        //     panic!("Unhandled {:?} DMA store", T::width())
        // }
        let align = offset & 3;
        let offset = offset & !3;
        let value = val.as_u32();
        let value = value << (align * 8);
        let major = (offset & 0x70) >> 4;
        let minor = offset & 0xf;
        match major {
            0..7 => {
                let port = Port::from_index(major as u8);
                let channel = self.dma.channel_mut(port);
                match minor {
                    0 => channel.set_base(value),
                    4 => channel.set_block_control(value),
                    8 => self.set_channel_control(port, value),
                    _ => panic!("Unhandled DMA read at {:x} offset", offset),
                }
                // if channel.active() { Some(port) } else { None }
            }
            7 => {
                match minor {
                    0 => self.dma.set_control(value),
                    4 => crate::dma::Dma::set_interrupt(self, value),
                    _ => panic!("Unhandled DMA read at {:x} offset", offset),
                }
                // None
            }
            _ => panic!("Unhandled DMA read at {:x} offset", offset),
        };
        // if let Some(port) = active_port {
        //     self.do_dma(port, 64)
        // }
    }

    pub fn set_channel_control(&mut self, port: Port, value: u32) {
        let mut ctrl = ChannelControl(value);
        if port == Port::Otc {
            ctrl.0 &= 0x51_000_000;
            ctrl.0 |= 2;
        }
        let channel = self.dma.channel_mut(port);
        let was_enabled = channel.enable;
        let is_enabled = ctrl.is_enabled();
        if was_enabled ^ is_enabled {
            // Channel was started or stopped
            if is_enabled {
                channel.set_control(ctrl.0);
                self.start_dma(port);
            } else {
                channel.enable = false;
                self.run_channel(port, 128 * 16);
                self.dma.channel_mut(port).set_control(ctrl.0);
                // channel.set_control(ctrl.0);
            }
        } else {
            channel.set_control(ctrl.0);
            // psx.dma[port].control = ctrl;
        }
    }

    pub fn dma_event(system: &mut System) {
        // continue where we left off
        // println!("dma!");
        system.run_channel(Port::Gpu, 128); // actually ellapsed
        system.run_channel(Port::Spu, 128); // actually ellapsed
        system.run_channel(Port::Pio, 128); // actually ellapsed
        system.run_channel(Port::Otc, 128); // actually ellapsed
        system.run_channel(Port::CdRom, 128); // actually ellapsed
        system.run_channel(Port::MdecOut, 128); // actually ellapsed
        system.run_channel(Port::MdecIn, 128); // actually ellapsed

    }

    pub fn start_dma(&mut self, port: Port) {
        let channel = self.dma.channel_mut(port);
        channel.remaining_words = 0;
        channel.clock_counter = 0;
        // continue where we left off
        self.run_channel(port, 64);
    }

    // pub fn do_dma(&mut self, port: Port, cycles: u64) {
    // match self.dma.channel(port).sync() {
    //     Sync::LinkedList => self.do_dma_linked_list(port, cycles),
    //     self.do_dma_block(port, cycles)
    // }
    // }
    pub fn run_channel(&mut self, port: Port, cycles: u64) {
        self.dma.channel_mut(port).clock_counter += cycles;
        let channel = self.dma.channel(port);
        let direction = channel.direction();
        let sync_mode = channel.sync();

        let increment = match channel.step() {
            Step::Increment => 4,
            Step::Decrement => -4i32 as u32,
        };
        let mut addr = channel.base();

        while self.dma.channel_mut(port).clock_counter > 0 {
            let mut do_copy = true;
            let channel = self.dma.channel(port);

            if channel.remaining_words == 0 {
                if !channel.enable {
                    break;
                }
                if !self.dma.master_enabled(port) {
                    break;
                }
                if !self.can_run(port, direction  == Direction::FromRam) {
                //     // The device is not ready
                    break;
                }

                // Initialize RAM pointer
                self.dma.channel_mut(port).curr_addr = channel.base;
                // psx.dma[port].cur_address = psx.dma[port].base;
                let cur_addr = channel.base;

                match sync_mode {
                    Sync::Manual => {
                        self.dma.channel_mut(port).remaining_words = channel.block_size;
                    }
                    Sync::Request => {
                        let channel = self.dma.channel_mut(port);
                        channel.remaining_words = channel.block_size;

                        channel.block_count -= 1;

                        // XXX From mednafen, only GPU timings are implemented so far
                        if port == Port::Gpu {
                            channel.clock_counter -= 7;
                        }
                    }
                    Sync::LinkedList => {
                        let overflow = cur_addr & 0x80_0000 != 0;

                        if overflow {
                            unimplemented!();
                        }
                        let channel = self.dma.channel_mut(port);

                        let header: u32 = self.ram.load::<u32>(cur_addr & 0x1f_fffc);
                        channel.curr_addr = (cur_addr + 4) & 0xff_ffff;
                        channel.base = header & 0xff_ffff;

                        let remw = (header >> 24) as u16;
                        channel.remaining_words = remw;

                        // Timings from mednafen
                        channel.clock_counter -= if remw > 0 { 15 } else { 10 };

                        // Mednafen skips the copy in this case because `remaining_words` might be 0
                        // and that wouldn't work correctly because in other situations
                        // `remaining_words` == 0 is the same as setting it to 0x1_0000 (i.e. it wraps
                        // around).
                        do_copy = false;
                    }
                }
            } else if channel.chop {
                self.dma.channel_mut(port).curr_addr = channel.base;
                self.dma.channel_mut(port).remaining_words = channel.block_size;
            }

            if do_copy {
                let cur_addr = self.dma.channel_mut(port).curr_addr & 0x1f_fffc;
                let channel = self.dma.channel(port);

                let overflow = cur_addr & 0x80_0000 != 0;
                if overflow {
                    unimplemented!("DMA address overflow");
                }

                let delay = if channel.direction == FromRam {
                    let v = self.ram.load::<u32>(cur_addr);
                    self.port_store(port, v)
                } else {
                    let (v, offset, read_delay) = self.port_load(port);
                    self.ram.store::<u32>((cur_addr.wrapping_add(offset)) & 0x1f_fffc, v);
                    read_delay
                };
                
                self.dma.channel_mut(port).clock_counter -= delay;

                self.dma.channel_mut(port).curr_addr = 0xff_ffff
                    & if channel.step == Step::Decrement {
                      self.dma.channel_mut(port).curr_addr.wrapping_sub(4)
                    } else {
                        self.dma.channel_mut(port).curr_addr.wrapping_add(4)
                    };

                self.dma.channel_mut(port).clock_counter -= 1;
                self.dma.channel_mut(port).remaining_words -= 1;
            }

            if channel.chop {
                let channel = self.dma.channel_mut(port);

                channel.base = channel.curr_addr;
                channel.block_size = channel.remaining_words;
            }

            if self.dma.channel(port).remaining_words == 0 {
                if !channel.enable {
                    break;
                }

                let end_of_dma = match sync_mode {
                    Sync::Manual => {
                        // We do the transfer all at once, we're done
                        true
                    }
                    Sync::Request => {
                        let channel = self.dma.channel_mut(port);

                        channel.base = channel.curr_addr;

                        channel.block_count == 0
                    }
                    Sync::LinkedList => {
                        // Check for end-of-list marker
                        self.dma.channel_mut(port).base == 0xff_ffff
                    }
                };

                if end_of_dma {
                    Dma::done(self, port);
                    // let irq = psx.dma.end_of_dma(port);
                    // irq::set_level(psx, irq::Interrupt::Dma, irq.is_active());
                }
            }
        }

        
    if self.dma.channel_mut(port).clock_counter > 0 {
        self.dma.channel_mut(port).clock_counter = 0;
    }

        // transfer size in words
        /*let mut remsz = match channel.transfer_size() {
            Some(n) => n,
            None => panic!("Couldn't figure DMA block transfer size"),
        };

        while remsz > 0 {
            let cur_addr = addr & 0x1ffffc;
            match direction {
                Direction::FromRam => {
                    let src_word = self.ram.load::<u32>(cur_addr);
                    match port {
                        Port::Gpu => {
                            // gpu::store(self, 0, src_word);
                            self.gpu.gp0(src_word);
                            // println!("GPU data: {:08x}", src_word);
                        }
                        Port::Spu => {
                            self.spu.ram_write::<4>(src_word);
                            // println!("SPU data: {:08x}", src_word);
                        }
                        Port::MdecIn => {
                            self.mdec.store(0, src_word);
                        }
                        _ => panic!("Unhandled DMA destination port {}", port as u8),
                    }
                }
                Direction::ToRam => {
                    let src_word = match port {
                        Port::Otc => match remsz {
                            1 => 0xffffff,                        // end of table marker
                            _ => addr.wrapping_sub(4) & 0x1fffff, // pointer to the prev entry
                        },
                        Port::Gpu => {
                            // gpu::load(self, 0)
                            self.gpu.read()
                        }
                        Port::CdRom => self.cdrom.load::<u32>(2),
                        Port::MdecOut => self.mdec.load(0),
                        Port::Spu => self.spu.ram_read::<4>(),
                        _ => panic!("Unhandled DMA source port {}", port as u8),
                    };
                    Ram::store::<u32>(self, cur_addr, src_word);
                }
            };

            addr = addr.wrapping_add(increment);
            remsz -= 1;
        }
        Dma::done(self, port);*/
    }

    pub fn port_store(&mut self, port: Port, v: u32) -> u64 {
        match port {
            Port::MdecIn => {
                self.mdec.store(0, v);
                0
            }
            Port::Gpu => {
                self.gpu.gp0(v);
                0
            }
            Port::Spu => {
                self.spu.ram_write::<4>(v);
                47
            }
            _ => unimplemented!("DMA port store"),
        }
    }
    pub fn port_load(&mut self, port: Port) -> (u32, u32, u64) {
        let mut offset = 0;
        let mut delay = 0;

        let v = match port {
            Port::Otc => {
                let channel = self.dma.channel(port);

                if channel.remaining_words == 1 {
                    // Last entry contains the end of table marker
                    0xff_ffff
                } else {
                    // Pointer to the previous entry
                    channel.curr_addr.wrapping_sub(4) & 0x1f_ffff
                }
            }
            // XXX latency taken from mednafen
            Port::CdRom => {
                delay = 8;
                // println!("dma load from cd");
                self.cdrom.load::<u32>(2)
                // cd::dma_load(psx)
            }
            Port::Spu => self.spu.ram_read::<4>(),
            Port::MdecOut => {
                let v = self.mdec.load::<u32>(0);
                // offset = off;
                v
            }
            Port::Gpu => self.gpu.read(),
            _ => unimplemented!("DMA port load {:?}", port),
        };

        (v, offset, delay)
    }

    // Check if the device targeted by `port` can either be read from of written to
    fn can_run(&mut self, port: Port, write: bool) -> bool {
        if write {
            match port {
                Port::Spu => true,
                Port::Gpu => true, //gpu::dma_can_write(psx),
                Port::MdecIn => self.mdec.dma_can_write(),
                _ => unimplemented!("Can write {:?}?", port),
            }
        } else {
            match port {
                Port::Spu => true,
                // XXX This is from mednafen but I'm not entirely sure why this doesn't simply return
                // `true`
                Port::Otc => true, //psx.dma[port].control.is_started(),
                // XXX Does this make sense? Can the CDC block the DMA if no sector has been read?
                Port::CdRom => true,
                Port::MdecOut => self.mdec.dma_can_read(),
                Port::Gpu => true,
                _ => unimplemented!("Can read {:?}?", port),
            }
        }
    }
    /*pub fn do_dma_linked_list(&mut self, port: Port, cycles: u64) {
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
                // gpu::store(self, 0, command);
                self.gpu.gp0(command);
                remsz -= 1;
            }
            if header & 0x800000 != 0 {
                break;
            }
            addr = header & 0x1ffffc;
        }

        Dma::done(self, port);
        // channel.done();
    }*/
}

#[derive(serde::Serialize, serde::Deserialize, Copy, Clone)]
struct ChannelControl(u32);

impl ChannelControl {
    fn new(v: u32) -> ChannelControl {
        ChannelControl(v)
    }

    fn get(self) -> u32 {
        self.0
    }

    /// Returns true if the 'enable' bit is set
    fn is_enabled(self) -> bool {
        self.0 & (1 << 24) != 0
    }

    /// Clear the enable bit
    fn disable(&mut self) {
        self.0 &= !(1 << 24);
    }

    /// Returns true if the 'start' bit is set
    fn is_started(self) -> bool {
        self.0 & (1 << 28) != 0
    }

    /// Returns true if transfer is from RAM to device, false otherwise
    fn is_from_ram(self) -> bool {
        self.0 & 1 != 0
    }

    fn sync_mode(self) -> SyncMode {
        match (self.0 >> 9) & 3 {
            0 => SyncMode::Manual,
            1 => SyncMode::Request,
            2 => SyncMode::LinkedList,
            _ => unimplemented!("Unknown DMA sync mode"),
        }
    }

    fn is_chopped(self) -> bool {
        self.0 & (1 << 8) != 0
    }

    fn is_backwards(self) -> bool {
        self.0 & (1 << 1) != 0
    }

    /// Called when the channel is stopped
    fn stop(&mut self) {
        // Clear enabled bit
        self.0 &= !(1 << 24);
        // Clear start/trigger bit
        self.0 &= !(1 << 28);
    }
}

#[derive(serde::Serialize, serde::Deserialize, PartialEq, Eq, Clone, Copy, Debug)]
pub enum SyncMode {
    /// Transfer starts when the CPU writes to the Trigger bit and transfers everything at once
    Manual = 0,
    /// Sync blocks to DMA requests
    Request = 1,
    /// Used to transfer GPU command lists
    LinkedList = 2,
}

pub extern "C" fn store_word(system_ptr: *mut System, addr: u32, val: u32) {
    let system = unsafe { system_ptr.as_mut().expect("ok") };
    let addr = addr.saturating_sub(0x10000);
    // println!("store word: addr: {:X}, val: {:X}", addr, val);
    System::store(system, addr, val);
}

pub extern "C" fn store_half_word(system_ptr: *mut System, addr: u32, val: u16) {
    // println!("are we here??");
    let system = unsafe { system_ptr.as_mut().expect("ok") };
    let addr = addr.saturating_sub(0x10000);
    System::store(system, addr, val);
}

pub extern "C" fn store_byte(system_ptr: *mut System, addr: u32, val: u8) {
    let system = unsafe { system_ptr.as_mut().expect("ok") };
    let addr = addr.saturating_sub(0x10000);
    System::store(system, addr, val);
}

pub extern "C" fn load_word(system_ptr: *mut System, addr: u32) -> u32 {
    // let masked_address = mask_region(addr);
    // if let Some(_) = map::RAM.contains(addr) {
    //         println!("system ram load !!!! where it should not be");
    // }
    // println!("load word slow..., system address: 0x{:X}", system_ptr as u64);
    // if system_ptr as u64 != 0x1488 {
    //     let ptr = (addr + 0x10000) as *const u32;
    //     unsafe {
    //         return *ptr;
    // }
    // } else {
    // println!("yepp!");
    unsafe {
        let addr = addr.saturating_sub(0x10000);
        // let system_ptr = SYSTEM_ADDR as *mut System;
        let system = system_ptr.as_mut().expect("ok");
        System::load(system, addr as u32)
    }
    // }
}

pub extern "C" fn load_half_word(system_ptr: *mut System, addr: u32) -> u32 {
    let addr = addr.saturating_sub(0x10000);
    let system = unsafe { system_ptr.as_mut().expect("ok") };
    System::load::<u16>(system, addr) as u32
}

pub extern "C" fn load_byte(system_ptr: *mut System, addr: u32) -> u32 {
    let addr = addr.saturating_sub(0x10000);
    let system = unsafe { system_ptr.as_mut().expect("ok") };
    System::load::<u8>(system, addr) as u32
}
