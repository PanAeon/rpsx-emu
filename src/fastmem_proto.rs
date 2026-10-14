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

use nix::poll::{poll, PollFd, PollFlags};

use libc::truncate;
use target_lexicon::PointerWidth;
use userfaultfd::{Uffd, UffdBuilder, Event};

// enable usefaultfd: 
//  sudo chmod 666 /dev/userfaultfd
//  sudo sysctl -w vm.unprivileged_userfaultfd=1 (only for kernel mode)

static mut FASTMEM_LOAD_ADDR: u64 = 0;
static mut SLOWMEM_LOAD_ADDR: u64 = 0;
static mut FASTMEM_LOAD_LEN: u64 = 0;
static mut BLOCK_ADDR: u64 = 0;
static mut BLOCK_LEN: u64 = 0;
static mut MEM_PROVIDER: Option<&mut Box<ArenaMemoryProvider>> = None;

#[allow(static_mut_refs)]
pub extern "C" fn segfault_handler(sig: i32, si: *mut libc::siginfo_t, ctx: *mut std::ffi::c_void) {
    unsafe {
        let si = (&mut *si) as &mut libc::siginfo_t;
        println!("caught segfault at address {:?}", si.si_addr());

        let mut uctx = (&mut * (ctx as *mut libc::ucontext_t)) as &mut libc::ucontext_t;
        let rip = uctx.uc_mcontext.gregs[libc::REG_RIP as usize];
        let rsp = uctx.uc_mcontext.gregs[libc::REG_RSP as usize];
        let rdi = uctx.uc_mcontext.gregs[libc::REG_RDI as usize];

        let stack = (rsp) as *const u64;
        let stack =  std::slice::from_raw_parts_mut(stack as *mut u64, 5 * 8) ;
        for i in 0..5 {
            println!("stack [{}]: 0x{:X}", i, stack[i]);
        }
        // ok 
        let instr_addr = stack[1] - 5; // callq
        println!("return addr:   0x{:X}", instr_addr);

        // now unprotect the instruction...
        unprotect_instr(instr_addr).expect("ok");
        // modify the instruction
        let instr_ptr = instr_addr as *mut u8;
        let instr_slice = std::slice::from_raw_parts_mut(instr_ptr as *mut u8, 5) ;
        println!("instr old: {:?}", instr_slice);
        // let offset = (instr_addr as i64).wrapping_sub(SLOWMEM_LOAD_ADDR as i64);
        let offset = (SLOWMEM_LOAD_ADDR as i64).wrapping_sub((instr_addr + 5)  as i64);
        let bytes = (offset as u32).to_le_bytes();
        instr_slice[1] = bytes[0];
        instr_slice[2] = bytes[1];
        instr_slice[3] = bytes[2];
        instr_slice[4] = bytes[3];
        println!("instr new: {:?}", instr_slice);
        // jump to instr
        uctx.uc_mcontext.gregs[libc::REG_RIP as usize] = instr_addr as i64;
        // fix stack (ok, it was fixed implicitly) code after callq gets executed twice..
        // wait woot?
        // uctx.uc_mcontext.gregs[libc::REG_RSP as usize] = rsp;
        // protect the instruction
        protect_instr(instr_addr).expect("ok");

        println!("faulting RIP: 0x{:X}, RSP: 0x{:X}", rip, rsp);
        println!("faulting RDI: 0x{:X}", rdi);

        println!("---");
        println!("fastmem addr: 0x{:X}", FASTMEM_LOAD_ADDR);
        println!("slowmem addr: 0x{:X}", SLOWMEM_LOAD_ADDR);
        println!("block addr:   0x{:X}", BLOCK_ADDR);

        
        // MEM_PROVIDER.as_mut().unwrap().allocate(size, align, kind)
        // MEM_PROVIDER.as_mut().unwrap().finalize(cranelift_jit::BranchProtection::None).expect("ok");
        // std::process::exit(0);
    }
}

pub fn install_segfault_handler() {

    let sa = MaybeUninit::<libc::sigaction>::zeroed();;
    unsafe {
        let mut sa = sa.assume_init();
        sa.sa_sigaction = segfault_handler as *const u8 as usize; 
        sa.sa_flags = libc::SA_SIGINFO;

        libc::sigaction(libc::SIGSEGV, &sa, 0 as *mut libc::sigaction);
    }
}

fn unprotect_instr(instr_addr: u64) -> Result<(), std::io::Error> {
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

fn protect_instr(instr_addr: u64) -> Result<(), std::io::Error> {
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
    fd: i32
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

// note that we might want to also allocate bios and scratchpad (won't fit)
// btw how's kuseg doing?
fn reserve_address_space(
) -> Result<*mut std::ffi::c_void, std::io::Error> {
    let prot = libc::PROT_NONE; //libc::PROT_READ | libc::PROT_WRITE;
    let flags = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED;

    let addr = 0x810_000 as *mut std::ffi::c_void;
    let len = 4 * GB - 8 * MB;

    unsafe {
        let ptr = libc::mmap(addr, len, prot, flags, -1, 0);
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
            return Err(err)
        }
        return Ok(ram_descr);
    }
}

fn truncate_mem(fd: i32, length: i64) -> Result<(), std::io::Error> {
    unsafe {
        let res = libc::ftruncate(fd, length);
        if res == -1 {
            let err = std::io::Error::last_os_error();
            return Err(err)
        }
        return Ok(());
    }
}
unsafe fn extend_lifetime<'a, T>(r: &'a mut T) -> &'static mut  T {
    unsafe {
       std::mem::transmute::<&'a mut T, &'static mut T>(r)
    }
}


pub fn run() {
    println!("<<fastmem prototype>>");

    install_segfault_handler();

    // first map ram to 65536..65536+2mb
    // or 0x10_000 - 0x210_000
    //
    let memfd = create_memfd().expect("ok");
    truncate_mem(memfd, 2 * MB as i64).expect("ok");

    let mem_start = 0x10_000 as *mut std::ffi::c_void;
    let mem_ptr = create_fixed_anon_map(mem_start, 2 * MB, memfd).expect("mem to be allocated");

    // then remap memory mirrors x3
    let mem_mirr_1_ptr = create_fixed_anon_map(0x210_000 as *mut std::ffi::c_void, 2 * MB, memfd).expect("mem to be allocated");
    let mem_mirr_2_ptr =create_fixed_anon_map(0x410_000 as *mut std::ffi::c_void, 2 * MB, memfd).expect("mem to be allocated");
    let mem_mirr_3_ptr =create_fixed_anon_map(0x610_000 as *mut std::ffi::c_void, 2 * MB, memfd).expect("mem to be allocated");
    let slice = unsafe { std::slice::from_raw_parts_mut(mem_ptr as *mut u8, 2 * MB) };

    // let uffd = UffdBuilder::new()
    //     .close_on_exec(true)
    //     .non_blocking(true)
    //     .user_mode_only(true)
    //     .create()
    //     .expect("uffd creation");

    // reserve psx address space 
    reserve_address_space().expect("ok");

    // let addr = 0x810_000 as *mut std::ffi::c_void;
    // let len = 4 * GB - 8 * MB;
    // uffd.register(addr, len).expect("uffd.register()");



    // let _s = std::thread::spawn(move || fault_handler_thread(uffd));

    for i in 0..2 * MB {
        slice[i] = i as u8;
    }
    let mem_provider = ArenaMemoryProvider::new_with_size(32 * MB).expect("ok");
    let mut mem_provider = Box::new(mem_provider);
    unsafe {
        let static_mem_provider = extend_lifetime(&mut mem_provider);
        MEM_PROVIDER = Some(static_mem_provider);
    }
    let mut compiler = Compiler::new(mem_provider);

    let fastmem_load = compiler.compile_fastmem_load();
    let slowmem_load = compiler.compile_slowmem_load();

    let block = compiler.compile_block(fastmem_load, slowmem_load);

    unsafe {
        BLOCK_ADDR = block as u64;
    }
    let jit_fn: extern "C" fn() -> () = unsafe { std::mem::transmute(block) };
    jit_fn();


    // let slice = unsafe { std::slice::from_raw_parts_mut(mem_mirr_3_ptr as *mut u8, 2 * MB) };
    for i in 0..32 {
        println!("[{}]", slice[i]);
    }

    println!("great success");
}

    // pub fn run_block() -> u64 {
    //     // self.debug_print();
    //     let block = bc.get_block(idx);
    //     let jit_fn: extern "C" fn() -> i32 = unsafe { std::mem::transmute(block.ptr) };
    //     jit_fn() as u64
    // }

pub struct Constants {
    io_fun: FuncId
}
struct Compiler {
    /// The function builder context, which is reused across multiple
    /// FunctionBuilder instances.
    builder_context: FunctionBuilderContext,

    /// The main Cranelift context, which holds the state for codegen. Cranelift
    /// separates this from `Module` to allow for parallel compilation, with a
    /// context per thread, though this isn't in the simple demo here.
    ctx: codegen::Context,

    /// The data description, which is to data objects what `ctx` is to functions.
    #[allow(dead_code)]
    data_description: DataDescription,

    /// The module, with the jit backend, which manages the JIT'd
    /// functions.
    module: JITModule,

    constants: Constants,
}

impl Compiler {

    pub fn new(mem_provider: Box<ArenaMemoryProvider>) -> Compiler {
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").unwrap();
        flag_builder.set("is_pic", "false").unwrap();
        let isa_builder = cranelift_native::builder().unwrap_or_else(|msg| {
            panic!("host machine is not supported: {}", msg);
        });
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .unwrap();
        let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        builder.memory_provider(mem_provider);
        // builder.memory_provider(provider)

        builder.symbol("io_fun", io_fun as *const u8);
        

        let mut module = JITModule::new(builder);

        // define function signature
        let mut sig_external = module.make_signature();
        sig_external.params.push(AbiParam::new(types::I32));
        // sig_external.params.push(AbiParam::new(types::I64));
        // sig_external.params.push(AbiParam::new(types::I32));
        sig_external.call_conv = module.target_config().default_call_conv;
        let io_fun = module
            .declare_function("io_fun", Linkage::Import, &sig_external)
            .unwrap();

        Compiler {
            builder_context: FunctionBuilderContext::new(),
            ctx: module.make_context(),
            data_description: DataDescription::new(),
            module,
            constants: Constants {
                io_fun
            },
        }
    }


    pub fn compile_fastmem_load(&mut self) -> FuncId {

        self.ctx
            .func
            .signature
            .params
            .push(AbiParam::new(types::I32));

        self.ctx
            .func
            .signature
            .returns
            .push(AbiParam::new(types::I32));

        // Create the builder to build a function.
        let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut self.builder_context);

        // Create the entry block, to start emitting code in.
        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        // Tell the builder to emit code in this block.
        builder.switch_to_block(entry_block);

        // And, tell the builder that this block will have no further
        // predecessors. Since it's the entry block, it won't have any
        // predecessors.
        builder.seal_block(entry_block);

        let address = builder.block_params(entry_block)[0];
        let address = builder.ins().uextend(types::I64, address);

        let v = builder.ins().load(types::I32, MachMemFlags::trusted(), address, 0x10_000);

        builder.ins().return_(&[v]);

        // translate... everything...
        builder.finalize(isa::TargetFrontendConfig {
            default_call_conv: CallConv::Fast,
            pointer_width: PointerWidth::U64,
            page_size_align_log2: 12,
        });

        let flags = settings::Flags::new(settings::builder());
        match verify_function(&self.ctx.func, &flags) {
            Ok(()) => {}
            Err(x) => {
                println!("{}", self.ctx.func.display());
                panic!("{}", x);
            }
        };
        let id = self
            .module
            .declare_function("fastmem_load", Linkage::Export, &self.ctx.func.signature)
            // .declare_anonymous_function(&self.ctx.func.signature)
            // .declare_function(&name, Linkage::Export, &self.ctx.func.signature)
            .expect("ok");

        // Define the function to jit. This finishes compilation, although
        // there may be outstanding relocations to perform. Currently, jit
        // cannot finish relocations until all functions to be called are
        // defined. For this toy demo for now, we'll just finalize the
        // function below.
        self.module.define_function(id, &mut self.ctx).expect("OK");

        // Now that compilation is finished, we can clear out the context state.
        self.module.clear_context(&mut self.ctx);

        // Finalize the functions which we just defined, which resolves any
        // outstanding relocations (patching in addresses, now that they're
        // available).
        self.module.finalize_definitions().unwrap();


        // We can now retrieve a pointer to the machine code.
        let code = self.module.get_finalized_function(id);

        unsafe {
            FASTMEM_LOAD_ADDR = code as u64;
        }

        id


    }

    pub fn compile_slowmem_load(&mut self) -> FuncId {

        self.ctx
            .func
            .signature
            .params
            .push(AbiParam::new(types::I32));

        self.ctx
            .func
            .signature
            .returns
            .push(AbiParam::new(types::I32));

        // Create the builder to build a function.
        let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut self.builder_context);

        // Create the entry block, to start emitting code in.
        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        // Tell the builder to emit code in this block.
        builder.switch_to_block(entry_block);

        // And, tell the builder that this block will have no further
        // predecessors. Since it's the entry block, it won't have any
        // predecessors.
        builder.seal_block(entry_block);

        let address = builder.block_params(entry_block)[0];

        let io_fun = self
                    .module
                    .declare_func_in_func(self.constants.io_fun, &mut builder.func);


        let _ = builder.ins().call(io_fun, &[address]);


        let v = builder.ins().iconst(types::I32, 0);
        builder.ins().return_(&[v]);

        // translate... everything...
        builder.finalize(isa::TargetFrontendConfig {
            default_call_conv: CallConv::Fast,
            pointer_width: PointerWidth::U64,
            page_size_align_log2: 12,
        });

        let flags = settings::Flags::new(settings::builder());
        match verify_function(&self.ctx.func, &flags) {
            Ok(()) => {}
            Err(x) => {
                println!("{}", self.ctx.func.display());
                panic!("{}", x);
            }
        };
        let id = self
            .module
            .declare_function("slowmem_load", Linkage::Local, &self.ctx.func.signature)
            // .declare_anonymous_function(&self.ctx.func.signature)
            // .declare_function(&name, Linkage::Export, &self.ctx.func.signature)
            .expect("ok");

        // Define the function to jit. This finishes compilation, although
        // there may be outstanding relocations to perform. Currently, jit
        // cannot finish relocations until all functions to be called are
        // defined. For this toy demo for now, we'll just finalize the
        // function below.
        self.module.define_function(id, &mut self.ctx).expect("OK");

        // Now that compilation is finished, we can clear out the context state.
        self.module.clear_context(&mut self.ctx);

        // Finalize the functions which we just defined, which resolves any
        // outstanding relocations (patching in addresses, now that they're
        // available).
        self.module.finalize_definitions().unwrap();

        // We can now retrieve a pointer to the machine code.
        let code = self.module.get_finalized_function(id);

        unsafe {
            SLOWMEM_LOAD_ADDR = code as u64;
        }

        id


    }


    pub fn compile_block(&mut self, fastmem_load: FuncId, slowmem_load: FuncId) -> *const u8 {

        // self.ctx
        //     .func
        //     .signature
        //     .returns
        //     .push(AbiParam::new(types::I32));

        // Create the builder to build a function.
        let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut self.builder_context);

        // Create the entry block, to start emitting code in.
        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        // Tell the builder to emit code in this block.
        builder.switch_to_block(entry_block);

        // And, tell the builder that this block will have no further
        // predecessors. Since it's the entry block, it won't have any
        // predecessors.
        builder.seal_block(entry_block);

        let fastmem_load = self
                    .module
                    .declare_func_in_func(fastmem_load, &mut builder.func);

        let slowmem_load = self
                    .module
                    .declare_func_in_func(slowmem_load, &mut builder.func);


        // read ram at 63 and insert it at 0...
        let p = builder.ins().iconst(types::I32, 63);
        // let p = builder.ins().iadd_imm_u(p, 0x10_000);
        // let v = builder.ins().load(types::I64, MachMemFlags::trusted(), p, 0);
        let res = builder.ins().call(fastmem_load, &[p]);
        let v = builder.inst_results(res)[0];

        // now store it at 0
        let p = builder.ins().iconst(types::I64, 0x10_000);
        builder.ins().store(MachMemFlags::trusted(), v, p, 0);

        // ok, this is what we want it to be:
        // let p = builder.ins().iconst(types::I32, 0x901_000);
        // builder.ins().call(slowmem_load, &[p]);

        // but instead we got fastmem..
        let p = builder.ins().iconst(types::I32, 0x901_000);
        builder.ins().call(fastmem_load, &[p]);

        // now trigger memfault
        // let p = builder.ins().iconst(types::I64, 63);
        // let p = builder.ins().iadd_imm_u(p, 0x880_000);
        // let v = builder.ins().load(types::I64, MachMemFlags::trusted(), p, 0);
        // let p = builder.ins().iconst(types::I64, 0x10_000);
        // builder.ins().store(MachMemFlags::trusted(), v, p, 0);


        builder.ins().return_(&[]);

        // translate... everything...
        builder.finalize(isa::TargetFrontendConfig {
            default_call_conv: CallConv::Fast,
            pointer_width: PointerWidth::U64,
            page_size_align_log2: 12,
        });

        let flags = settings::Flags::new(settings::builder());
        match verify_function(&self.ctx.func, &flags) {
            Ok(()) => {}
            Err(x) => {
                println!("{}", self.ctx.func.display());
                panic!("{}", x);
            }
        };
        let id = self
            .module
            .declare_anonymous_function(&self.ctx.func.signature)
            // .declare_function(&name, Linkage::Export, &self.ctx.func.signature)
            .expect("ok");

        // Define the function to jit. This finishes compilation, although
        // there may be outstanding relocations to perform. Currently, jit
        // cannot finish relocations until all functions to be called are
        // defined. For this toy demo for now, we'll just finalize the
        // function below.
        self.module.define_function(id, &mut self.ctx).expect("OK");

        // Now that compilation is finished, we can clear out the context state.
        self.module.clear_context(&mut self.ctx);

        // Finalize the functions which we just defined, which resolves any
        // outstanding relocations (patching in addresses, now that they're
        // available).
        self.module.finalize_definitions().unwrap();

        // We can now retrieve a pointer to the machine code.
        let code = self.module.get_finalized_function(id);

        code


    }

}


pub extern "C" fn io_fun(address: u32) {
    println!("IO!!!!, address: 0x{:X}", address);
}

const MB: usize = 1024 * 1024;
const GB: usize = 1024 * MB;

fn fault_handler_thread(uffd: Uffd) {
    println!("caught user fault!");

        let mut fds = [PollFd::new(&uffd, PollFlags::POLLIN)];
        let nready = poll(&mut fds, -1).expect("poll");
        let pollfd = fds[0];

        println!("\nfault_handler_thread():");
        let revents = pollfd.revents().unwrap();
        println!(
            "    poll() returns: nready = {}; POLLIN = {}; POLLERR = {}",
            nready,
            revents.contains(PollFlags::POLLIN),
            revents.contains(PollFlags::POLLERR),
        );

        // Read an event from the userfaultfd
        let event = uffd
            .read_event()
            .expect("read uffd_msg")
            .expect("uffd_msg ready");

        // We expect only one kind of event; verify that assumption

        if let Event::Pagefault { addr, .. } = event {
            // Display info about the page-fault event

            println!("    UFFD_EVENT_PAGEFAULT event: {:?}", event);


            // Copy the page pointed to by 'page' into the faulting region. Vary the contents that are
            // copied in, so that it is more obvious that each fault is handled separately.

            // for c in unsafe { std::slice::from_raw_parts_mut(page as *mut u8, page_size) } {
            //     *c = b'A' + fault_cnt % 20;
            // }
            // fault_cnt += 1;

            // let dst = (addr as usize & !(page_size - 1)) as *mut c_void;
            // let copy = unsafe { uffd.copy(page, dst, page_size, true).expect("uffd copy") };

            // println!("        (uffdio_copy.copy returned {})", copy);
        } else {
            panic!("Unexpected event on userfaultfd");
        }
    std::process::exit(1);
}
