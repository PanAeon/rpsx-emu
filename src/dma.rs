use crate::system::{AccessWidth, Addressable, System};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Dma {
    dpcr: Control,
    dicr: InterruptReg,
    channels: [Channel; 7],
}

pub fn load<T: Addressable>(system: &mut System, offset: u32) -> T {
    // if T::width() != AccessWidth::Word {
    //     println!("Unhandled {:?} DMA load", T::width())
    // }
    let align = offset & 3;
    let offset = offset & !3;
    let major = (offset & 0x70) >> 4;
    let minor = offset & 0xf;
    let r = match major {
        0..7 => {
            let channel = system.dma.channel(Port::from_index(major as u8));
            match minor {
                0 => channel.maddr,
                4 => channel.block_control(),
                8 => channel.ctrl.0,
                _ => panic!("Unhandled DMA read at {:x} offset {major}.{minor}", offset),
            }
        }
        7 => match minor {
            0 => system.dma.dpcr.0,
            4 => system.dma.dicr.0,
            _ => panic!("Unhandled DMA read at {:x} offset {major}.{minor}", offset),
        },
        _ => panic!("Unhandled DMA read at {:x} offset", offset),
    };
    T::from_u32(r >> (align * 8))
}

pub fn store<T: Addressable>(system: &mut System, offset: u32, val: T) {
    // if T::width() != AccessWidth::Word {
    //     panic!("Unhandled {:?} DMA store", T::width())
    // }
    // let align = offset & 3;
    let offs = offset & !3;
    // let value = val.as_u32();
    // let value = value << (align * 8);
    let major = (offset & 0x70) >> 4;
    let minor = offs & 0xf;
    match major {
        0..7 => {
            let port = Port::from_index(major as u8);
            let channel = system.dma.channel_mut(port);
            match minor {
                0 => channel.maddr = partial_write(channel.maddr, offset, val),
                4 => {
                    let block = channel.block_control();
                    channel.set_block_control(partial_write(block, offset, val))
                }
                8 => {
                    println!("channel ctrl, channel: {}, ctrl: {:?}", major, ChanControl(val.as_u32()));
                    channel.ctrl.0 = partial_write(channel.ctrl.0, offset, val);
                    match port {
                        Port::MdecIn => do_mdec_in(system),
                        Port::MdecOut => do_mdec_out(system),
                        Port::Gpu => do_gpu(system),
                        Port::CdRom => do_cdrom(system),
                        Port::Spu => do_spu(system),
                        Port::Pio => unimplemented!(),
                        Port::Otc => do_otc(system),
                    };
                    //set_channel_control(system, port, value),
                }
                _ => panic!("Unhandled DMA write at {:x} offset", offset),
            }
            // if channel.active() { Some(port) } else { None }
        }
        7 => {
            match minor {
                0 => system.dma.dpcr.0 = partial_write(system.dma.dpcr.0, offset, val),
                4 => {
                    let mut new_dicr = InterruptReg(partial_write(system.dma.dicr.0, offset, val));
                    new_dicr.set_irq_flags(system.dma.dicr.irq_flags() & !new_dicr.irq_flags()); // write 1 to clear
                    new_dicr.set_master_irq(system.dma.dicr.master_irq()); // bit 31 is read-only
                    system.dma.dicr = new_dicr;
                    system.dma.dicr.update_master_irq();
                }
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
pub fn partial_write<T: Addressable>(base: u32, addr: u32, value: T) -> u32 {
    match T::width() {
        AccessWidth::Byte => {
            let align = addr & 3;
            let shift = align * 8;
            let mask = 0xff_u32 << shift;
            (base & !mask) | (value.as_u32() << shift)
        }
        AccessWidth::Halfword => {
            let align = addr & 2;
            let shift = align * 8;
            let mask = 0xffff_u32 << shift;
            (base & !mask) | (value.as_u32() << shift)
        }
        AccessWidth::Word => value.as_u32(),
    }
}
fn set_channel_irq(system: &mut System, port: Port, is_full_transfer: bool) {
    let chan_mask = 1_u32 << port as u8;

    if system.dma.dicr.irq_mask() & chan_mask == 0 {
        return;
    }
    let mode_bit = if is_full_transfer {
        system.dma.dicr.irq_mode() & chan_mask == 0
    } else {
        system.dma.dicr.irq_mode() & chan_mask != 0
    };

    if mode_bit {
        system
            .dma
            .dicr
            .set_irq_flags(system.dma.dicr.irq_flags() | chan_mask);
        system.dma.dicr.update_master_irq();

        if system.dma.dicr.master_irq() {
            system.irqctl.status.set_dma(true);
        }
    }
}

fn do_mdec_in(system: &mut System) {
    let chan = system.dma.channel(Port::MdecIn);
    if !chan.ctrl.start() {
        return;
    }
    if !system.mdec.dma_can_write() {
        return;
    }
    if !system.dma.dpcr.mdecin_enabled() {
        return;
    }
    let block_size = chan.block_size();
    let step = chan.addr_step();

    while chan.block.count > 0 {
        for _ in 0..block_size {
            let chan = system.dma.channel(Port::MdecIn);
            let v = system.load::<u32>(chan.maddr);
            let chan = system.dma.channel_mut(Port::MdecIn);
            chan.maddr = chan.maddr.wrapping_add(step);
            system.mdec.store(0, v);
        }
        system.dma.channel_mut(Port::MdecIn).block.count -= 1;
    }
    let chan = system.dma.channel_mut(Port::MdecIn);
    chan.ctrl.set_start(false);
    set_channel_irq(system, Port::MdecIn, true);
}

fn do_mdec_out(system: &mut System) {
    let chan = system.dma.channel(Port::MdecOut);
    if !chan.ctrl.start() {
        return;
    }
    if !system.mdec.dma_can_read() {
        return;
    }
    if !system.dma.dpcr.mdecout_enabled() {
        return;
    }
    let block_size = chan.block_size();
    let step = chan.addr_step();

    while chan.block.count > 0 {
        for _ in 0..block_size {
            let chan = system.dma.channel(Port::MdecIn);
            let v = system.mdec.load::<u32>(0);
            system.store::<u32>(chan.maddr, v);
            let chan = system.dma.channel_mut(Port::MdecIn);
            chan.maddr = chan.maddr.wrapping_add(step);
        }
        system.dma.channel_mut(Port::MdecOut).block.count -= 1;
    }
    let chan = system.dma.channel_mut(Port::MdecOut);
    chan.ctrl.set_start(false);
    set_channel_irq(system, Port::MdecOut, true);
}

fn do_gpu(system: &mut System) {
    let chan = system.dma.channel(Port::Gpu);
    if !chan.ctrl.is_active() {
        return;
    }
    match chan.ctrl.sync_mode() {
        Sync::Manual => panic!("manual sync not implemented"),
        Sync::Request => do_gpu_sync_mode_slice(system),
        Sync::LinkedList => todo!(),
    }
}

pub fn do_gpu_sync_mode_slice(system: &mut System) {
    let chan = system.dma.channel(Port::Gpu);

    let addr_inc = chan.addr_step();
    let block_size = chan.block_size();
    let block_count = chan.block_count();

    match chan.ctrl.direction() {
        Direction::ToRam => {
            for _ in 0..block_count {
                for _ in 0..block_size {
                    let v = system.gpu.read();
                    let chan = system.dma.channel(Port::Gpu);
                    system.store(chan.maddr, v);
                    let chan = system.dma.channel_mut(Port::Gpu);
                    chan.maddr = chan.maddr.wrapping_add(addr_inc);
                }
                set_channel_irq(system, Port::Gpu, false);
            }
        }
        Direction::FromRam => {
            for _ in 0..block_count {
                for _ in 0..block_size {
                    let chan = system.dma.channel(Port::Gpu);
                    let v = system.load::<u32>(chan.maddr);
                    system.gpu.gp0(v);
                    let chan = system.dma.channel_mut(Port::Gpu);
                    chan.maddr = chan.maddr.wrapping_add(addr_inc);
                }
                set_channel_irq(system, Port::Gpu, false);
            }
        }
    };

    let chan = system.dma.channel_mut(Port::Gpu);
    chan.ctrl.reset_active();
    set_channel_irq(system, Port::Gpu, true);
}

fn do_cdrom(system: &mut System) {
    todo!()
}
fn do_spu(system: &mut System) {
    todo!()
}
fn do_otc(system: &mut System) {
    let chan = system.dma.channel(Port::Otc);
    if !chan.ctrl.is_active() {
        return;
    }
    if !system.dma.dpcr.otc_enabled() {
        return;
    }
    let transfer_len = chan.block_size() * chan.block_count();

    let mut addr = chan.maddr;

    for i in 0..transfer_len {
        let next_addr = (addr - 4) & 0xffffff;
        let hdr = if i == transfer_len - 1 {0xffffff} else {next_addr};
        system.store::<u32>(addr, hdr);
        addr = next_addr;
    }
    let chan = system.dma.channel_mut(Port::Otc);
    chan.ctrl.reset_active();
    set_channel_irq(system, Port::Otc, true);
}

impl Dma {
    pub fn new() -> Dma {
        Dma {
            dpcr: Control(0x07654321),
            dicr: InterruptReg(0),
            channels: [Channel::new(); 7],
        }
    }

    // pub fn master_enabled(&self, port: Port) -> bool {
    //     let dpcr = Control(self.control);
    //     match port {
    //         Port::MdecIn => dpcr.mdecin_enabled(),
    //         Port::MdecOut => dpcr.mdecout_enabled(),
    //         Port::Gpu => dpcr.gpu_enabled(),
    //         Port::CdRom => dpcr.cdrom_enabled(),
    //         Port::Spu => dpcr.spu_enabled(),
    //         Port::Pio => dpcr.pio_enabled(),
    //         Port::Otc => dpcr.otc_enabled(),
    //     }
    // }

    // pub fn irq(&self) -> bool {
    //     let channel_irq = self.channel_irq_flags & self.channel_irq_enable;
    //     self.force_irq || (self.irq_enable && channel_irq != 0)
    // }

    // pub fn interrupt(&self) -> u32 {
    //     let mut r = 0;
    //
    //     r |= self.irq_dummy as u32;
    //     r |= (self.force_irq as u32) << 15;
    //     r |= (self.channel_irq_enable as u32) << 16;
    //     r |= (self.irq_enable as u32) << 23;
    //     r |= (self.channel_irq_flags as u32) << 24;
    //     r |= (self.irq() as u32) << 31;
    //
    //     r
    // }

    // pub fn set_interrupt(system: &mut System, val: u32) {
    //     let prev_irq = system.dma.irq();
    //     system.dma.irq_dummy = (val & 0x3f) as u8;
    //
    //     system.dma.force_irq = (val >> 15) & 1 != 0;
    //     system.dma.channel_irq_enable = ((val >> 16) & 0x7f) as u8;
    //     system.dma.irq_enable = (val >> 23) & 1 != 0;
    //
    //     // writing 1 to a flag resets it
    //     let ack = ((val >> 24) & 0x3f) as u8;
    //     system.dma.channel_irq_flags &= !ack; // TODO: (and, additionally, IRQ3 (DMA) must be acknowledged via Port 1F801070h).
    //
    //     if !prev_irq && system.dma.irq() {
    //         system.irqctl.status.set_dma(true);
    //     }
    // }

    // pub fn done(system: &mut System, port: Port) {
    //     system.dma.channel_mut(port).done();
    //
    //     let prev_irq = system.dma.irq();
    //
    //     let it_en = system.dma.channel_irq_enable & (1 << (port as usize));
    //
    //     system.dma.channel_irq_flags |= it_en;
    //
    //     if !prev_irq && system.dma.irq() {
    //         system.irqctl.status.set_dma(true);
    //     }
    //
    // }

    pub fn channel(&self, port: Port) -> Channel {
        self.channels[port as usize]
    }
    pub fn channel_mut(&mut self, port: Port) -> &mut Channel {
        &mut self.channels[port as usize]
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct Channel {
    pub maddr: u32,
    ctrl: ChanControl,
    block: BlockCtrl,
}
impl Channel {
    pub fn new() -> Channel {
        Channel {
            maddr: 0,
            ctrl: ChanControl(0),
            block: BlockCtrl { size: 0, count: 0 },
        }
    }
    pub fn block_control(&self) -> u32 {
        let bs = self.block.size as u32;
        let bc = self.block.count as u32;
        (bc << 16) | bs
    }
    pub fn set_block_control(&mut self, value: u32) {
        self.block.size = value as u16;
        self.block.count = (value >> 16) as u16;
    }

    pub fn block_size(&self) -> u32 {
        if self.block.size == 0 {
            0x10000
        } else {
            self.block.size as u32
        }
    }

    pub fn addr_step(&self) -> u32 {
        match self.ctrl.addr_inc() {
            Step::Increment => 4,
            Step::Decrement => (-4_i32) as u32,
        }
    }

    pub fn block_count(&self) -> u32 {
        match self.ctrl.sync_mode() {
            Sync::Manual => 1,
            Sync::Request => self.block.count as u32,
            Sync::LinkedList => 0,
        }
    }

    // pub fn transfer_size(&self) -> Option<u32> {
    //     let bs = self.block_size as u32;
    //     let bc = self.block_count as u32;
    //
    //     match self.sync {
    //         Sync::Manual => Some(bs),
    //         Sync::Request => Some(bc * bs),
    //         Sync::LinkedList => None
    //     }
    // }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Port {
    MdecIn = 0,
    MdecOut = 1,
    Gpu = 2,
    CdRom = 3,
    Spu = 4,
    Pio = 5,
    Otc = 6,
}

impl Port {
    pub fn from_index(index: u8) -> Port {
        match index {
            0 => Port::MdecIn,
            1 => Port::MdecOut,
            2 => Port::Gpu,
            3 => Port::CdRom,
            4 => Port::Spu,
            5 => Port::Pio,
            6 => Port::Otc,
            n => panic!("Unknown dma port {n}"),
        }
    }
}
/*
  0-2   DMA0, MDECin  Priority      (0..7; 0=Highest, 7=Lowest)
  3     DMA0, MDECin  Master Enable (0=Disable, 1=Enable)
  4-6   DMA1, MDECout Priority      (0..7; 0=Highest, 7=Lowest)
  7     DMA1, MDECout Master Enable (0=Disable, 1=Enable)
  8-10  DMA2, GPU     Priority      (0..7; 0=Highest, 7=Lowest)
  11    DMA2, GPU     Master Enable (0=Disable, 1=Enable)
  12-14 DMA3, CDROM   Priority      (0..7; 0=Highest, 7=Lowest)
  15    DMA3, CDROM   Master Enable (0=Disable, 1=Enable)
  16-18 DMA4, SPU     Priority      (0..7; 0=Highest, 7=Lowest)
  19    DMA4, SPU     Master Enable (0=Disable, 1=Enable)
  20-22 DMA5, PIO     Priority      (0..7; 0=Highest, 7=Lowest)
  23    DMA5, PIO     Master Enable (0=Disable, 1=Enable)
  24-26 DMA6, OTC     Priority      (0..7; 0=Highest, 7=Lowest)
  27    DMA6, OTC     Master Enable (0=Disable, 1=Enable)
  28-30 CPU memory access priority  (0..7; 0=Highest, 7=Lowest)
  31    No effect, should be CPU memory access enable (R/W)
*/
bitfield::bitfield! {
    #[derive(Default, Serialize, Deserialize)]
    struct Control(u32);
    mdecin_prio,    _ : 0,2;
    mdecin_enabled, _: 3;
    mdecout_prio, _:4,6;
    mdecout_enabled, _: 7;
    gpu_prio, _: 8,10;
    gpu_enabled, _: 11;
    cdrom_prio, _: 12,14;
    cdrom_enabled, _: 15;
    spu_prio, _: 16,18;
    spu_enabled, _: 19;
    pio_prio, _: 20,22;
    pio_enabled, _: 23;
    otc_prio, _: 24,26;
    otc_enabled, _: 27;
    mem_access_prio, _: 28,30;
}

bitfield::bitfield! {
    #[derive(Default, Serialize, Deserialize, Clone, Copy)]
    struct ChanControl(u32);
    impl Debug;
    u8, from into Direction, direction, set_direction: 0,0;
    u8, from into Step, addr_inc, set_addr_incr: 1,1;
    u8, into Sync, sync_mode, _: 9,10;
    chopping_dma, _: 16,18;
    chopping_cpu, _: 20,22;
    start, set_start: 24;
    force_start, set_force_start: 28;
    pause, _: 29;
    snooping, _: 30;
}

impl ChanControl {
    pub fn is_active(&self) -> bool {
        (self.start() || self.force_start()) && !self.pause()
    }
    pub fn reset_active(&mut self) {
        self.set_start(false);
        self.set_force_start(false);
    }
}

#[derive(Default, Serialize, Deserialize, Clone, Copy)]
struct BlockCtrl {
    size: u16,
    count: u16,
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Debug)]
pub enum Direction {
    ToRam = 0,
    FromRam = 1,
}

impl From<u8> for Direction {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::ToRam,
            1 => Self::FromRam,
            _ => unreachable!(),
        }
    }
}

impl From<Direction> for u8 {
    fn from(value: Direction) -> Self {
        match value {
            Direction::ToRam => 0,
            Direction::FromRam => 1,
        }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug)]
pub enum Sync {
    Manual = 0,  // burst
    Request = 1, // slice
    LinkedList = 2,
}

impl From<u8> for Sync {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::Manual,
            1 => Self::Request,
            2 => Self::LinkedList,
            _ => unreachable!("got unreachable value: 0x{:x}", value),
        }
    }
}

impl From<Sync> for u8 {
    fn from(value: Sync) -> Self {
        match value {
            Sync::Manual => 0,
            Sync::Request => 1,
            Sync::LinkedList => 2,
        }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, Eq, PartialEq, Debug)]
pub enum Step {
    Increment = 0,
    Decrement = 1,
}

impl From<u8> for Step {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::Increment,
            1 => Self::Decrement,
            _ => unreachable!(),
        }
    }
}

impl From<Step> for u8 {
    fn from(value: Step) -> Self {
        match value {
            Step::Increment => 0,
            Step::Decrement => 1,
        }
    }
}

bitfield::bitfield! {
    #[derive(Default, Serialize, Deserialize, Clone)]
    struct InterruptReg(u32);
    impl Debug;
    irq_mode, _: 0,6;
    bus_error, _: 15;
    irq_mask, _: 16,22;
    master_enable, _: 23;
    irq_flags, set_irq_flags: 24,30;
    master_irq, set_master_irq: 31;
}

impl InterruptReg {
    fn update_master_irq(&mut self) {
        let irq_pending = self.irq_flags() & self.irq_mask() != 0;
        self.set_master_irq(self.bus_error() || (self.master_enable() && irq_pending));
    }
}
