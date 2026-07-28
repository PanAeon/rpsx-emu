use std::fs;
use std::io;
use std::path::Path;

use crate::bios::Bios;
use crate::ram::Ram;
mod bios;
mod ram;
mod cpu;
mod memory_bus;

fn main() -> io::Result<()>{

    let bios = Bios::new(Path::new("/foo/SCPH1001.BIN"))?;
    let ram = Ram::new();

    // let bytes = fs::read("/foo/SCPH1001.BIN")?;
    for i in (0..40).step_by(4) {
        print!("0x{:02X}", bios.data[i+3]);
        print!("{:02X}", bios.data[i+2]);
        print!("{:02X}", bios.data[i+1]);
        print!("{:02X}", bios.data[i+0]);
        println!();
    }

    let memory_bus = memory_bus::MemoryBus::new(bios, ram);
    let mut cpu = cpu::Cpu::new(memory_bus);
    loop {
        cpu.run_next_instruction();
        // cpu.debug_print();
    }
    // Ok(())
}
