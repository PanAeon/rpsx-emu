use std::backtrace::Backtrace;
use std::ffi::CString;
use std::mem::MaybeUninit;
use std::ptr;

use cranelift::codegen::{ir::BlockArg, isa::CallConv};
use cranelift::prelude::*;
use cranelift_jit::{ArenaMemoryProvider, JITBuilder, JITMemoryProvider, JITModule};
use cranelift_module::{DataDescription, FuncId, Linkage, Module};
// use cranelift::codegen::
// use cranelift_codegen::ir::types::*;
use cranelift::codegen::ir::{BlockCall, FuncRef, MachMemFlags};
use cranelift::codegen::verifier::verify_function;

use iced_x86::{Code, Decoder, DecoderOptions, Encoder, Instruction};
use nix::poll::{PollFd, PollFlags, poll};

use libc::truncate;
use target_lexicon::PointerWidth;

use crate::system::{load_word, load_word_fast};

pub static mut FASTMEM_LOAD32_ADDR: u64 = 0;
pub static mut SLOWMEM_LOAD32_ADDR: u64 = 0;

#[allow(static_mut_refs)]
pub extern "C" fn segfault_handler(sig: i32, si: *mut libc::siginfo_t, ctx: *mut std::ffi::c_void) {
    unsafe {
        let load_word_addr = load_word as *const u8 as u64;
        //FASTMEM_LOAD32_ADDR = load_word_fast as *const u8 as u64;
        let si = (&mut *si) as &mut libc::siginfo_t;
        println!("caught segfault at address 0x{:X}", si.si_addr() as u64);
        // println!("caught segfault at address {:?}", si.si_addr());

        let mut uctx = (&mut *(ctx as *mut libc::ucontext_t)) as &mut libc::ucontext_t;
        let rip = uctx.uc_mcontext.gregs[libc::REG_RIP as usize];
        let mut rsp = uctx.uc_mcontext.gregs[libc::REG_RSP as usize];
        let rdi = uctx.uc_mcontext.gregs[libc::REG_RDI as usize];
        let rbp = uctx.uc_mcontext.gregs[libc::REG_RBP as usize];
        let rsi = uctx.uc_mcontext.gregs[libc::REG_RSI as usize];

        println!("faulting RIP: 0x{:X}, RSP: 0x{:X}", rip, rsp);
        println!("faulting RDI: 0x{:X}, RSI: 0x{:X}", rdi, rsi);

        let addr = si.si_addr() as u64;

        if (addr as u64) >= 0x10_000 && (addr as u64) < 4 * GB as u64 + 0x10_000 {
            println!("caught fastmem fault")
        } else {
            println!("caught some other shitty fault");
            let bt = Backtrace::force_capture();

            // 2. Format into a message string
            let msg = format!(
                "\n--- Caught Signal {} (SIGSEGV) ---\nStack Trace:\n{}\n",
                sig, bt
            );

            // 3. Use standard error raw file descriptor to avoid Rust's buffered stdout/stderr macros
            let stderr_fd = libc::STDERR_FILENO;
            let _ = libc::write(stderr_fd, msg.as_ptr() as *const libc::c_void, msg.len());

            std::process::exit(1);
        }

        // uctx.uc_mcontext.gregs[libc::REG_RSI as usize] -= 0x10_000;
        // restore caller stack
        // movq %rbp, %rsp
        //
        // rsp = rbp;
        // uctx.uc_mcontext.gregs[libc::REG_RSP as usize] = rbp;

        // let stack = (rsp) as *const u64;
        // let stack = std::slice::from_raw_parts_mut(stack as *mut u64, 5 * 8);
        // for i in 0..5 {
        //     println!("stack [{}]: 0x{:X}", i, stack[i]);
        // }
        // popq rbp
        // uctx.uc_mcontext.gregs[libc::REG_RBP as usize] = stack[0] as i64;
        // retq
        // uctx.uc_mcontext.gregs[libc::REG_RSP as usize] += 4;

        // ok
        // let instr_addr = stack[1] - 22; // moveabsq, moveabsq, callq
        // println!("return addr:   0x{:X}", instr_addr);
        let instr_addr = rip as u64;

        // now unprotect the instruction...
        unprotect_instr(instr_addr).expect("ok");
        // decode the instruction and the next one
        let bytes = std::slice::from_raw_parts_mut(instr_addr as *mut u8, 64 );
        let mut instruction = iced_x86::Instruction::default();
        let mut decoder = Decoder::with_ip(64, bytes, instr_addr, DecoderOptions::NONE);
        decoder.decode_out(&mut instruction);
        decoder.decode_out(&mut instruction);
        // if instruction.op_code().is_nop() {
        //     println!("3 byte instr not handled now... exiting");
        //     std::process::exit(1);
        // }  
        let movq_address = instr_addr - 10;
        let movq_bytes = std::slice::from_raw_parts(movq_address as *const u8, 64 );
        let mut decoder = Decoder::with_ip(64, movq_bytes, movq_address, DecoderOptions::NONE);
        decoder.decode_out(&mut instruction);

        let reg = instruction.op0_register();

        println!("got reg: {:?}", reg);

        // now we need to gen 2 byte callq...
        let mut encoder = Encoder::new(64);
        let ins = Instruction::with1(Code::Call_rm64, reg).expect("ok");
        let len = encoder.encode(&ins, instr_addr).expect("ok");

        println!("encoded len: {}", len);
        let buff = encoder.take_buffer();
        bytes[0..len].copy_from_slice(&buff);
        // modify the instruction
        // let instr_ptr = instr_addr as *mut u8;
        // let instr_slice = std::slice::from_raw_parts_mut(instr_ptr as *mut u8, 10);
        // println!("instr old: {:?}", instr_slice);
        // let offset = (instr_addr as i64).wrapping_sub(SLOWMEM_LOAD_ADDR as i64);
        // let offset = (SLOWMEM_LOAD32_ADDR as i64).wrapping_sub((instr_addr + 5) as i64);
        // let bytes = (0x1488_u64).to_le_bytes();
        // instr_slice[2] = bytes[0];
        // instr_slice[3] = bytes[1];
        // instr_slice[4] = bytes[2];
        // instr_slice[5] = bytes[3];
        // instr_slice[6] = bytes[4];
        // instr_slice[7] = bytes[5];
        // instr_slice[8] = bytes[6];
        // instr_slice[9] = bytes[7];
        // println!("instr new: {:?}", instr_slice);
        // jump to instr
        // uctx.uc_mcontext.gregs[libc::REG_RIP as usize] = (instr_addr) as i64;
        // fix stack (ok, it was fixed implicitly) code after callq gets executed twice..
        // wait woot?
        // uctx.uc_mcontext.gregs[libc::REG_RSP as usize] = rsp;
        // protect the instruction
        protect_instr(instr_addr).expect("ok");

        // println!("fastmem addr: 0x{:X}", FASTMEM_LOAD32_ADDR);
        // println!("slowmem addr: 0x{:X}", SLOWMEM_LOAD32_ADDR);
        println!("---\n");
        // println!("block addr:   0x{:X}", BLOCK_ADDR);

        // MEM_PROVIDER.as_mut().unwrap().allocate(size, align, kind)
        // MEM_PROVIDER.as_mut().unwrap().finalize(cranelift_jit::BranchProtection::None).expect("ok");
        // std::process::exit(0);
    }
}

pub fn install_segfault_handler() {
    let sa = MaybeUninit::<libc::sigaction>::zeroed();
    unsafe {
        let mut sa = sa.assume_init();
        sa.sa_sigaction = segfault_handler as *const u8 as usize;
        sa.sa_flags = libc::SA_SIGINFO;

        libc::sigaction(libc::SIGSEGV, &sa, 0 as *mut libc::sigaction);
    }
}

pub fn unprotect_instr(instr_addr: u64) -> Result<(), std::io::Error> {
    let prot = libc::PROT_READ | libc::PROT_WRITE;
    // let flags = libc::MAP_SHARED | libc::MAP_ANONYMOUS;

    let addr = instr_addr & !0xFFF;
    let addr = addr as *mut std::ffi::c_void;

    unsafe {
        let ptr = libc::mprotect(addr, 4096, prot);
        if ptr == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

pub fn protect_instr(instr_addr: u64) -> Result<(), std::io::Error> {
    let prot = libc::PROT_READ | libc::PROT_EXEC;

    let addr = instr_addr & !0xFFF;
    let addr = addr as *mut std::ffi::c_void;

    unsafe {
        let ptr = libc::mprotect(addr, 4096, prot);
        if ptr == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

fn create_fixed_anon_map(
    addr: *mut std::ffi::c_void,
    len: usize,
    fd: i32,
) -> Result<*mut std::ffi::c_void, std::io::Error> {
    let prot = libc::PROT_READ | libc::PROT_WRITE;
    let flags = libc::MAP_SHARED; //libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED;

    unsafe {
        let ptr = libc::mmap(addr, len, prot, flags, fd, 0);
        if ptr == libc::MAP_FAILED {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(ptr)
        }
    }
}

fn reserve_address_space(start: u64, length: usize) -> Result<*mut std::ffi::c_void, std::io::Error> {
    let prot = libc::PROT_NONE; //libc::PROT_READ | libc::PROT_WRITE;
    let flags = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED;

    let addr = start as *mut std::ffi::c_void;
    // let addr = 0x810_000 as *mut std::ffi::c_void;
    // let len = 4 * GB - 8 * MB;

    unsafe {
        let ptr = libc::mmap(addr, length, prot, flags, -1, 0);
        if ptr == libc::MAP_FAILED {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(ptr)
        }
    }
}

fn create_memfd() -> Result<i32, std::io::Error> {
    unsafe {
        let ram_descr = libc::memfd_create("RAM\0".as_ptr() as *const i8, libc::MFD_CLOEXEC);
        if ram_descr == -1 {
            let err = std::io::Error::last_os_error();
            let raw_code = err.raw_os_error(); // e.g., EMFILE, ENOMEM, EINVAL
            eprintln!("memfd_create error: {:?} (raw errno: {:?})", err, raw_code);
            return Err(err);
        }
        return Ok(ram_descr);
    }
}

fn truncate_mem(fd: i32, length: i64) -> Result<(), std::io::Error> {
    unsafe {
        let res = libc::ftruncate(fd, length);
        if res == -1 {
            let err = std::io::Error::last_os_error();
            return Err(err);
        }
        return Ok(());
    }
}
unsafe fn extend_lifetime<'a, T>(r: &'a mut T) -> &'static mut T {
    unsafe { std::mem::transmute::<&'a mut T, &'static mut T>(r) }
}

/*
  KUSEG     KSEG0     KSEG1
  00000000h 80000000h A0000000h  2048K  Main RAM (first 64K reserved for BIOS)
  1F000000h 9F000000h BF000000h  8192K  DEV0 expansion (ROM/RAM)
  1F800000h 9F800000h    --      1K     Scratchpad (D-Cache used as Fast RAM)
  1F801000h 9F801000h BF801000h  4K     I/O Ports
  1F802000h 9F802000h BF802000h  8K     DEV8 expansion (I/O Ports)
  1FA00000h 9FA00000h BFA00000h  2048K  DEV1 expansion (SRAM BIOS region for DTL cards)
  1FC00000h 9FC00000h BFC00000h  512K   BIOS ROM (Kernel) (4096K max)
        FFFE0000h (in KSEG2)     0.5K   Internal CPU control registers (Cache Control)

*/

pub fn create_memory_mappings() -> (&'static mut [u8], &'static mut [u8]) {
    println!("<<fastmem create memory map>>");

    // install_segfault_handler();

    // first map ram to 65536..65536+2mb
    // or 0x10_000 - 0x210_000
    //
    let memfd = create_memfd().expect("ok");
    truncate_mem(memfd, 2 * MB as i64).expect("ok");

    let mem_start = 0x10_000 as *mut std::ffi::c_void;
    let mem_ptr = create_fixed_anon_map(mem_start, 2 * MB, memfd).expect("mem to be allocated");

    println!("mem ptr::: 0x{:X}", mem_ptr as u64);

    // TODO: remap memory mirrors x3
    // let mem_mirr_1_ptr = create_fixed_anon_map(0x210_000 as *mut std::ffi::c_void, 2 * MB, memfd).expect("mem to be allocated");
    // let mem_mirr_2_ptr =create_fixed_anon_map(0x410_000 as *mut std::ffi::c_void, 2 * MB, memfd).expect("mem to be allocated");
    // let mem_mirr_3_ptr =create_fixed_anon_map(0x610_000 as *mut std::ffi::c_void, 2 * MB, memfd).expect("mem to be allocated");
    // 80_000_000
    let kseg0_ptr = create_fixed_anon_map(0x80_010_000 as *mut std::ffi::c_void, 2 * MB, memfd)
        .expect("mem to be allocated");
    let kseg1_ptr = create_fixed_anon_map(0xA0_010_000 as *mut std::ffi::c_void, 2 * MB, memfd)
        .expect("mem to be allocated");

    // now bios...
    //
    let biosfd = create_memfd().expect("ok");
    truncate_mem(biosfd, 512 * KB as i64).expect("ok");
    // let kuseg_bios = create_fixed_anon_map(0xDF_C10_000 as *mut std::ffi::c_void, 2 * MB, biosfd)
    //     .expect("mem to be allocated");
    let kuseg_bios = create_fixed_anon_map(0x1F_C10_000 as *mut std::ffi::c_void, 2 * MB, biosfd)
        .expect("mem to be allocated");
    let kseg0_bios = create_fixed_anon_map(0x9F_C10_000 as *mut std::ffi::c_void, 2 * MB, biosfd)
        .expect("mem to be allocated");
    let kseg1_bios = create_fixed_anon_map(0xBF_C10_000 as *mut std::ffi::c_void, 2 * MB, biosfd)
        .expect("mem to be allocated");

    let slice = unsafe { std::slice::from_raw_parts_mut(mem_ptr as *mut u8, 2 * MB) };
    let bios_slice = unsafe { std::slice::from_raw_parts_mut(kuseg_bios as *mut u8, 512 * KB) };

    // reserve psx address space
    // reserve_address_space(0x810_000,0x1FC10_000 - 0x810_000).expect("ok"); // TODO: actually reserve address space..
    // reserve_address_space(0x80_010_000,0x9F_C10_000 - 0x80_010_000).expect("ok"); // TODO: actually reserve address space..
    // reserve_address_space(0xA0_010_000,0xBF_C10_000 - 0xA0_010_000).expect("ok"); // TODO: actually reserve address space..
    // reserve_address_space(0xC0_000_000, 1 * GB).expect("ok"); // TODO: actually reserve address space..

    (slice, bios_slice )
}

//     let mem_provider = ArenaMemoryProvider::new_with_size(32 * MB).expect("ok");
//     let mut mem_provider = Box::new(mem_provider);
//     unsafe {
//         let static_mem_provider = extend_lifetime(&mut mem_provider);
//         MEM_PROVIDER = Some(static_mem_provider);
//     }
//     let mut compiler = Compiler::new(mem_provider);
//
//     let fastmem_load = compiler.compile_fastmem_load();
//     let slowmem_load = compiler.compile_slowmem_load();
//
//     let block = compiler.compile_block(fastmem_load, slowmem_load);
//
//     unsafe {
//         BLOCK_ADDR = block as u64;
//     }
//     let jit_fn: extern "C" fn() -> u32 = unsafe { std::mem::transmute(block) };
//     let res = jit_fn();
//     println!("result: {}", res);
//
//
//     // let slice = unsafe { std::slice::from_raw_parts_mut(mem_mirr_3_ptr as *mut u8, 2 * MB) };
//     for i in 0..32 {
//         println!("[{}]", slice[i]);
//     }
//
//     println!("great success");
// }

// pub fn run_block() -> u64 {
//     // self.debug_print();
//     let block = bc.get_block(idx);
//     let jit_fn: extern "C" fn() -> i32 = unsafe { std::mem::transmute(block.ptr) };
//     jit_fn() as u64
// }
const KB: usize = 1024;
const MB: usize = 1024 * KB;
const GB: usize = 1024 * MB;
