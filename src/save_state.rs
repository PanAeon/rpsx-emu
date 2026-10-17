use serde::{Deserialize, Serialize};

use crate::{
    cdrom::CDRom, dma::Dma, dynarec::{Dynarec, Registers}, gpu::Gpu, gte::Gte, irq::InterruptController, mdec::Mdec, scheduler::Scheduler, scratchpad::Scratchpad, sio::Sio, spu::Spu, system::System, timers::Timers,
};
use serde::de::Deserializer;

pub fn save_state(idx: u32, system: &mut System) {
    let save_dir = "/home/vitalii/.config/rpsx-emu/saves";

    let mut fb = flexbuffers::FlexbufferSerializer::new();

    system.gpu.save_vram();

    system.serialize(&mut fb).expect("ok");

    let fbuf = fb.view();

    let filename = save_dir.to_string() + "/1.save";

    std::fs::write(&filename, fbuf).expect("ok");

    println!("save successful");
}

pub fn load_state(idx: u32, system: &mut System) {
    let save_dir = "/home/vitalii/.config/rpsx-emu/saves";
    let filename = save_dir.to_string() + "/1.save";
    let data = std::fs::read(filename).expect("ok");

    let reader = flexbuffers::Reader::get_root(data.as_slice()).expect("ok");
    let state = SystemState::deserialize(reader).expect("ok");
    let SystemState {
        ram,
        scratchpad,
        dma,
        cpu,
        spu,
        irqctl,
        scheduler,
        timers,
        cdrom,
        sio,
        mdec,
        gpu,
    } = state;
    system.ram.data.copy_from_slice(&ram);
    system.cpu.regs.set(cpu.regs);
    system.cpu.gte.set(cpu.gte);
    system.block_cache.invalidate();
    system.scratchpad = scratchpad;
    system.dma = dma;
    system.spu = spu;
    system.irqctl = irqctl;
    system.scheduler = scheduler;
    system.timers = timers;
    let cddata = system.cdrom.disk.take().unwrap();
    system.cdrom = cdrom;
    system.cdrom.disk.as_mut().unwrap().file = cddata.file;
    system.cdrom.disk.as_mut().unwrap().cache = cddata.cache;
    // system.cdrom.disk.as_mut().unwrap().data = cddata.data;
    system.sio = sio;
    system.sio.memcard1.read_file_data(); // TODO: actually read it...
    system.sio.memcard2.read_file_data();
    system.mdec = mdec;
    let sender = system.gpu.renderer_sender.take();
    let receiver = system.gpu.renderer_receiver.take();
    let handle = system.gpu.renderer_handle.take();
    system.gpu = gpu;
    system.gpu.renderer_sender = sender;
    system.gpu.renderer_receiver = receiver;
    system.gpu.renderer_handle = handle;
    system.gpu.send_drawing_area_change();
    system.gpu.load_vram();


    // TODO: patch sio....


    println!("OK!!!");
}



#[derive(Deserialize)]
pub struct CpuState {
    regs: Registers,
    gte: Gte
}

#[derive(Deserialize)]
pub struct SystemState {
    #[serde(with = "serde_bytes")]
    pub ram: Box<[u8]>,
    pub scratchpad: Scratchpad,
    pub dma: Dma,
    pub cpu: CpuState,
    pub spu: Spu,
    pub irqctl: InterruptController,
    pub scheduler: Scheduler,
    pub timers: Timers,
    pub cdrom: CDRom,
    pub sio: Sio,
    pub mdec: Mdec,
    pub gpu: Gpu,
}

// fn deserialize<'de, D>(deserializer: D) -> Result<Mdec, D::Error>
// where
//     D: Deserializer<'de> {
//      // deserializer.deserialize_struct(name, fields, visitor)
//     // let gpu = Gpu::deserialize(deserializer)?;
//     // let mdec = Mdec::deserialize(deserializer)?;
//     Ok(mdec)
//     // let bytes = serde_bytes::ByteBuf::deserialize(deserializer)?;
//     // let data = bytes.into_boxed_slice();
//     // Ok(Ram { data: Box::into_pin(data) })
// }
