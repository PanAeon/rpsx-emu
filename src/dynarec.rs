use std::collections::{HashMap, HashSet};
use std::pin::Pin;

use crate::block_cache::{self, CacheEntry};
use crate::system::{
    load_byte, load_half_word, load_word, store_byte, store_half_word, store_word,
};
use crate::{cpu::Instruction, gte::Gte, system::System};

use cranelift::codegen::{ir::BlockArg, isa::CallConv};
use cranelift::prelude::*;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{DataDescription, FuncId, Linkage, Module};
// use cranelift::codegen::
// use cranelift_codegen::ir::types::*;
use cranelift::codegen::ir::{BlockCall, FuncRef, MachMemFlags, ValueListPool};
use cranelift::codegen::verifier::verify_function;

use target_lexicon::PointerWidth;

const REG_HI: u32 = 32;
const REG_LO: u32 = 33;
const REG_PC: u32 = 34;
const REG_SR: u32 = 35;
const REG_CAUSE: u32 = 36;
const REG_EPC: u32 = 37;
const REG_BADDR: u32 = 38;
const REG_LOAD_REG: u32 = 39;
const REG_LOAD_VAL: u32 = 40;

#[repr(C)]
pub struct Registers {
    pub regs: [u32; 32],
    pub hi: u32,
    pub lo: u32,
    pub pc: u32, // reset value 0xBFC00000
    // pub next_pc: u32,
    // pub current_pc: u32,
    pub sr: u32,
    pub cause: u32,
    pub epc: u32,
    pub baddr: u32,
    pub load: (u32, u32), // load initiated by the current instruction
                          //branch: bool, // set by the current instruction if the branch occurred
                          //delay_slot:bool, // set if the current instruction executes in the delay slot
}
pub struct Constants {
    regs: *mut u8,
    system: *mut System,
    gte: *mut Gte,
    store_word: FuncId,
    load_word: FuncId,
    store_half_word: FuncId,
    store_byte: FuncId,
    load_half_word: FuncId,
    load_byte: FuncId,
    print_: FuncId,
    gte_command: FuncId,
    gte_data: FuncId,
    gte_control: FuncId,
    gte_set_data: FuncId,
    gte_set_control: FuncId,
    ram: *mut [u8],
    cache: *mut [CacheEntry],
}

#[derive(Default)]
pub struct Debug {
    hit_breakpoint: bool,
    i: u32,
}

pub struct Dynarec {
    pub system: Pin<Box<System>>,
    pub regs: Pin<Box<Registers>>,
    pub gte: Pin<Box<Gte>>,
    /// The function builder context, which is reused across multiple
    /// FunctionBuilder instances.
    builder_context: FunctionBuilderContext,

    /// The main Cranelift context, which holds the state for codegen. Cranelift
    /// separates this from `Module` to allow for parallel compilation, with a
    /// context per thread, though this isn't in the simple demo here.
    ctx: codegen::Context,

    /// The data description, which is to data objects what `ctx` is to functions.
    data_description: DataDescription,

    /// The module, with the jit backend, which manages the JIT'd
    /// functions.
    module: JITModule,

    constants: Constants,

    debug: Debug,
}

impl Dynarec {
    pub fn new(system: System) -> Dynarec {
        let mut regs = [0xdeadbeef; 32];
        regs[0] = 0;
        let regs = Registers {
            regs,
            hi: 0xdeadbeef,
            lo: 0xdeadbeef,
            pc: 0xBFC00000,
            // next_pc: 0xBFC00004,
            // current_pc: 0xBFC00000,
            sr: 0,
            cause: 0,
            epc: 0,
            baddr: 0,
            // out_regs: regs,
            load: (0, 0),
            // branch: false,
            // delay_slot: false,
        };
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

        builder.symbol("store_word", store_word as *const u8);
        builder.symbol("store_half_word", store_half_word as *const u8);
        builder.symbol("store_byte", store_byte as *const u8);
        builder.symbol("load_word", load_word as *const u8);
        builder.symbol("load_half_word", load_half_word as *const u8);
        builder.symbol("load_byte", load_byte as *const u8);
        builder.symbol("print_", print_ as *const u8);
        builder.symbol("gte_command", gte_command as *const u8);
        builder.symbol("gte_data", gte_data as *const u8);
        builder.symbol("gte_control", gte_control as *const u8);
        builder.symbol("gte_set_data", gte_set_data as *const u8);
        builder.symbol("gte_set_control", gte_set_control as *const u8);

        let mut module = JITModule::new(builder);

        // define function signature
        let mut sig_external = module.make_signature();
        sig_external.params.push(AbiParam::new(types::I64));
        sig_external.params.push(AbiParam::new(types::I32));
        sig_external.params.push(AbiParam::new(types::I32));
        sig_external.call_conv = module.target_config().default_call_conv;
        let store_word = module
            .declare_function("store_word", Linkage::Import, &sig_external)
            .unwrap();

        let mut sig_external = module.make_signature();
        sig_external.params.push(AbiParam::new(types::I64));
        sig_external.params.push(AbiParam::new(types::I32));
        sig_external.params.push(AbiParam::new(types::I16));
        sig_external.call_conv = module.target_config().default_call_conv;
        let store_half_word = module
            .declare_function("store_half_word", Linkage::Import, &sig_external)
            .unwrap();

        let mut sig_external = module.make_signature();
        sig_external.params.push(AbiParam::new(types::I64));
        sig_external.params.push(AbiParam::new(types::I32));
        sig_external.params.push(AbiParam::new(types::I8));
        sig_external.call_conv = module.target_config().default_call_conv;
        let store_byte = module
            .declare_function("store_byte", Linkage::Import, &sig_external)
            .unwrap();

        let mut sig_external = module.make_signature();
        sig_external.params.push(AbiParam::new(types::I64));
        sig_external.params.push(AbiParam::new(types::I32));
        sig_external.returns.push(AbiParam::new(types::I32));
        sig_external.call_conv = module.target_config().default_call_conv;
        let load_word = module
            .declare_function("load_word", Linkage::Import, &sig_external)
            .unwrap();

        let mut sig_external = module.make_signature();
        sig_external.params.push(AbiParam::new(types::I64));
        sig_external.params.push(AbiParam::new(types::I32));
        sig_external.returns.push(AbiParam::new(types::I16));
        sig_external.call_conv = module.target_config().default_call_conv;
        let load_half_word = module
            .declare_function("load_half_word", Linkage::Import, &sig_external)
            .unwrap();

        let mut sig_external = module.make_signature();
        sig_external.params.push(AbiParam::new(types::I64));
        sig_external.params.push(AbiParam::new(types::I32));
        sig_external.returns.push(AbiParam::new(types::I8));
        sig_external.call_conv = module.target_config().default_call_conv;
        let load_byte = module
            .declare_function("load_byte", Linkage::Import, &sig_external)
            .unwrap();

        let mut sig_external = module.make_signature();
        sig_external.params.push(AbiParam::new(types::I8));
        sig_external.call_conv = module.target_config().default_call_conv;
        let print_ = module
            .declare_function("print_", Linkage::Import, &sig_external)
            .unwrap();

        let mut sig_external = module.make_signature();
        sig_external.params.push(AbiParam::new(types::I64));
        sig_external.params.push(AbiParam::new(types::I32));
        sig_external.call_conv = module.target_config().default_call_conv;
        let gte_command = module
            .declare_function("gte_command", Linkage::Import, &sig_external)
            .unwrap();

        let mut sig_external = module.make_signature();
        sig_external.params.push(AbiParam::new(types::I64));
        sig_external.params.push(AbiParam::new(types::I8));
        sig_external.returns.push(AbiParam::new(types::I32));
        sig_external.call_conv = module.target_config().default_call_conv;
        let gte_data = module
            .declare_function("gte_data", Linkage::Import, &sig_external)
            .unwrap();

        let mut sig_external = module.make_signature();
        sig_external.params.push(AbiParam::new(types::I64));
        sig_external.params.push(AbiParam::new(types::I8));
        sig_external.returns.push(AbiParam::new(types::I32));
        sig_external.call_conv = module.target_config().default_call_conv;
        let gte_control = module
            .declare_function("gte_control", Linkage::Import, &sig_external)
            .unwrap();

        let mut sig_external = module.make_signature();
        sig_external.params.push(AbiParam::new(types::I64));
        sig_external.params.push(AbiParam::new(types::I8));
        sig_external.params.push(AbiParam::new(types::I32));
        sig_external.call_conv = module.target_config().default_call_conv;
        let gte_set_data = module
            .declare_function("gte_set_data", Linkage::Import, &sig_external)
            .unwrap();

        let mut sig_external = module.make_signature();
        sig_external.params.push(AbiParam::new(types::I64));
        sig_external.params.push(AbiParam::new(types::I8));
        sig_external.params.push(AbiParam::new(types::I32));
        sig_external.call_conv = module.target_config().default_call_conv;
        let gte_set_control = module
            .declare_function("gte_set_control", Linkage::Import, &sig_external)
            .unwrap();

        let mut regs = Box::pin(regs);
        let regs_ptr = unsafe { std::mem::transmute(regs.as_mut()) };
        let mut system = Box::pin(system);
        let system_ptr: *mut System = &mut *system;
        let mut gte = Box::pin(Gte::new());
        let gte_ptr: *mut Gte = &mut *gte;
        let ram_ptr: *mut [u8] = &mut *system.ram.data;
        let cache_ptr: *mut [CacheEntry] = &mut *system.block_cache.ram;
        // let system_ptr = unsafe { std::mem::transmute(&*system) };
        Dynarec {
            system,
            regs,
            gte,
            builder_context: FunctionBuilderContext::new(),
            ctx: module.make_context(),
            data_description: DataDescription::new(),
            module,
            debug: Default::default(),
            constants: Constants {
                regs: regs_ptr,
                system: system_ptr,
                ram: ram_ptr,
                cache: cache_ptr,
                gte: gte_ptr,
                store_word,
                store_half_word,
                store_byte,
                load_word,
                load_half_word,
                load_byte,
                print_,
                gte_command,
                gte_data,
                gte_control,
                gte_set_data,
                gte_set_control,
            },
        }
    }

    pub fn pc(&self) -> u32 {
        self.regs.pc
    }

    pub fn run_block(&mut self, idx: u32) -> u64 {
        // self.debug_print();
        let block = self.system.block_cache.get_block(idx);
        let jit_fn: extern "C" fn() -> i32 = unsafe { std::mem::transmute(block.ptr) };
        jit_fn() as u64
    }

    pub fn check_for_pending_interrupts(&mut self) -> bool {
        if self.system.irqctl.pending() {
            self.regs.cause |= 1 << 10;
        } else {
            self.regs.cause &= !(1 << 10);
        }
        // mask bits 8..15
        let pending = (self.regs.cause & self.regs.sr) & 0x700; //0xFF00;
        pending != 0 && (self.regs.sr & 1 != 0)
    }

    pub fn external_interrupt(&mut self) {
        self.exception(Exception::ExternalInterrupt, None);
    }

    pub fn exception(&mut self, cause: Exception, baddr: Option<u32>) {
        let mode = self.regs.sr & 0x3f;
        self.regs.sr &= !0x3f;
        self.regs.sr |= (mode << 2) & 0x3f;

        self.regs.cause &= !0x7c;
        self.regs.cause |= (cause.code() as u32) << 2; // woot?

        // if self.regs.delay_slot { // this what happend?
        //     self.regs.epc = self.regs.current_pc.wrapping_sub(4);
        //     self.regs.cause |= 1 << 31;
        // } else {
        self.regs.epc = self.regs.pc;
        self.regs.cause &= !(1 << 31);
        // }

        // exception handler address depends on the BEV bit
        let handler: u32 = if self.regs.sr & (1 << 22) != 0 {
            0xbfc00180
        } else {
            0x80000080
        };

        if let Exception::LoadAddressError | Exception::StoreAddressError = cause {
            self.regs.baddr = baddr.unwrap();
        }

        self.regs.pc = handler;
    }

    pub fn debug_print(&mut self) {
        if !self.debug.hit_breakpoint {
            if self.regs.pc == 0xBFC02EA0 {
                self.debug.hit_breakpoint = true;
            }
        }
        if self.debug.hit_breakpoint {
            if self.debug.i < 40 {
                println!("CPU pc: 0x{:X}", self.regs.pc);
                self.debug.i += 1;
            } else {
                println!("CPU pc: 0x{:X}", self.regs.pc);
                panic!("done");
            }
        }
        // println!("CPU pc: 0x{:X}", self.regs.pc);
        // for (i, x) in self.regs.regs.iter().enumerate() {
        //     println!("regs[{:02}]: 0x{:X}", i, x);
        // }
        // println!();
    }

    pub fn compile_block(&mut self, addr: u32) -> block_cache::Block {
        // woot?
        // if addr == 0x80000080 {
        //     panic!("at exception handler");
        // }
        let mut buff = [Instruction(0); 256];
        // I guess first parse the block
        let xs = self.parse_block(addr, &mut buff);

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

        // builder.import_function(ExtFuncData { name: "", signature: (), colocated: (), patchable: () })

        let mut bldr = BlockBuilder {
            bldr: builder,
            vars: HashMap::new(),
            module: &mut self.module,
            delayed_load: None,
            constants: &self.constants,
            store_word: None,
            store_half_word: None,
            store_byte: None,
            load_word: None,
            load_half_word: None,
            load_byte: None,
            print_: None,
            gte_command: None,
            gte_data: None,
            gte_control: None,
            gte_set_data: None,
            gte_set_control: None,
            i: 0,
            addr,
            next_instr: None,
            delay_slot: false,
            regs: HashSet::new(),
            // entry_block,
            // start_block,
        };


        for i in 0..xs.len() {
           bldr.get_registers(xs[i]);
        }

        bldr.compile_entry_block();

        let mut last_jmp = false;
        for i in 0..xs.len() {
            bldr.i = i;
            bldr.next_instr = (i < xs.len() - 1).then(|| xs[i + 1]);
            bldr.compile_instr(xs[i]);
            if xs[i].is_unconditional_jump() {
                last_jmp = true;
                break;
            }
        }

        bldr.push_delay_load_to_regs();
        if !last_jmp {
            println!("not last jump, start addr: {:X}, len: {}", addr, xs.len());
            bldr.save_context();
            bldr.set_pc_imm(addr + (xs.len() as u32 * 4) + 4);
            let cycles = bldr.get_cycles();
            let cycles = bldr.bldr.ins().iconst(types::I32, cycles);
            bldr.bldr.ins().return_(&[cycles]);
        }

        // translate... everything...
        bldr.bldr.finalize(isa::TargetFrontendConfig {
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
        if !last_jmp {
            // println!("{}", self.ctx.func.display());
            println!("last command is not jump");
        }
        //
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

        block_cache::Block {
            ptr: code,
            crc32: 0,
            length: xs.len() as u32,
        }
    }

    pub fn parse_block<'a, 'b>(
        &'a mut self,
        addr: u32,
        buff: &'b mut [Instruction],
    ) -> &'b [Instruction] {
        let mut len = 255;
        for i in 0..255 {
            buff[i] = Instruction(self.system.load::<u32>(addr + i as u32 * 4));
            if buff[i].is_unconditional_jump() {
                buff[i + 1] = Instruction(self.system.load::<u32>(addr + (i + 1) as u32 * 4));
                // what to do with syscall? break?
                len = i + 2;
                break;
            }
        }
        if len == 255 && buff[254].is_conditional_jump() {
            buff[255] = Instruction(self.system.load::<u32>(addr + (255) as u32 * 4));
            len = 256;
        }
        &buff[0..len]
    }

    // pub fn create_data(&mut self) {
    //     // self.module.declare_data(name, linkage, writable, tls)
    //     // self.data_description.define(contents);
    // }
}

struct BlockBuilder<'a> {
    bldr: FunctionBuilder<'a>,
    vars: HashMap<u32, Variable>,
    regs: HashSet<u32>,
    module: &'a mut JITModule,
    delayed_load: Option<(u32, Value)>,
    constants: &'a Constants,
    i: usize,
    addr: u32,
    next_instr: Option<Instruction>,
    delay_slot: bool,
    store_word: Option<FuncRef>,
    load_word: Option<FuncRef>,
    store_half_word: Option<FuncRef>,
    store_byte: Option<FuncRef>,
    load_half_word: Option<FuncRef>,
    load_byte: Option<FuncRef>,
    print_: Option<FuncRef>,
    gte_command: Option<FuncRef>,
    gte_data: Option<FuncRef>,
    gte_control: Option<FuncRef>,
    gte_set_data: Option<FuncRef>,
    gte_set_control: Option<FuncRef>,
    // entry_block: Block,
    // start_block: Block,
}

impl<'a> BlockBuilder<'a> {
    fn compile_instr(&mut self, instr: Instruction) {
        self.inject_print_tty_output();
        match instr.opcode() {
            0x00 => match instr.secondary_opcode() {
                0x00 => self.compile_sll(instr),
                0x02 => self.compile_srl(instr),
                0x03 => self.compile_sra(instr),
                0x04 => self.compile_sllv(instr),
                0x06 => self.compile_srlv(instr),
                0x07 => self.compile_srav(instr),
                0x08 => self.compile_jr(instr),
                0x09 => self.compile_jalr(instr),
                0x0C => self.compile_syscall(instr),
                0x0D => self.compile_break(instr),
                0x10 => self.compile_mfhi(instr),
                0x11 => self.compile_mthi(instr),
                0x12 => self.compile_mflo(instr),
                0x13 => self.compile_mtlo(instr),
                0x1A => self.compile_div(instr),
                0x1B => self.compile_divu(instr),
                0x18 => self.compile_mult(instr),
                0x19 => self.compile_multu(instr),
                0x20 => self.compile_add(instr),
                0x21 => self.compile_addu(instr),
                0x22 => self.compile_sub(instr),
                0x23 => self.compile_subu(instr),
                0x24 => self.compile_and(instr),
                0x25 => self.compile_or(instr),
                0x26 => self.compile_xor(instr),
                0x27 => self.compile_nor(instr),
                0x2A => self.compile_slt(instr),
                0x2B => self.compile_sltu(instr),
                _ => panic!(
                    "Unhandled secondary opcode: {:02X}, instr: {:X} addr: {:X}",
                    instr.secondary_opcode(),
                    instr.0,
                    self.get_current_pc()
                ),
            },
            0x01 => self.compile_bxx(instr),
            0x02 => self.compile_j(instr),
            0x03 => self.compile_jal(instr),
            0x04 => self.compile_beq(instr),
            0x05 => self.compile_bne(instr),
            0x06 => self.compile_blez(instr),
            0x07 => self.compile_bgtz(instr),
            0x08 => self.compile_addi(instr),
            0x09 => self.compile_addiu(instr),
            0x12 => self.compile_cop2(instr),
            0x0A => self.compile_slti(instr),
            0x0B => self.compile_sltiu(instr),
            0x0C => self.compile_andi(instr),
            0x0D => self.compile_ori(instr),
            0x0E => self.compile_xori(instr),
            0x0F => self.compile_lui(instr),
            0x10 => self.compile_cop0(instr),
            0x20 => self.compile_lb(instr),
            0x21 => self.compile_lh(instr),
            0x22 => self.compile_lwl(instr),
            0x23 => self.compile_lw(instr),
            0x24 => self.compile_lbu(instr),
            0x25 => self.compile_lhu(instr),
            0x26 => self.compile_lwr(instr),
            0x28 => self.compile_sb(instr),
            0x29 => self.compile_sh(instr),
            0x2a => self.compile_swl(instr),
            0x2B => self.compile_sw(instr),
            0x2E => self.compile_swr(instr),
            0x32 => self.compile_lwc2(instr),
            0x3A => self.compile_swc2(instr),
            _ => panic!(
                "Unhandled instruction: {:08X}, opcode: {:02X}, address: {:X}",
                instr.0,
                instr.opcode(),
                self.get_current_pc()
            ),
        }
    }

    fn get_registers(&mut self, instr: Instruction) {
        // self.inject_print_tty_output();
        let pc = self.get_current_pc() & 0x1FFFFFFF;
        if pc == 0xA0 || pc == 0xB0 {
            self.regs.insert(0x9);
            self.regs.insert(0x4);
        }
        match instr.opcode() {
            0x00 => match instr.secondary_opcode() {
                0x00 => {
                    // sll
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rd());
                }
                0x02 => {
                    //self.compile_srl(instr),
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rd());
                }
                0x03 => {
                    //self.compile_sra(instr),
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rd());
                }
                0x04 => {
                    //self.compile_sllv(instr),
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rs());
                    self.regs.insert(instr.rd());
                }
                0x06 => {
                    // self.compile_srlv(instr),
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rs());
                    self.regs.insert(instr.rd());
                }
                0x07 => {
                    //self.compile_srav(instr),
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rs());
                    self.regs.insert(instr.rd());
                }
                0x08 => {
                    //self.compile_jr(instr),
                    self.regs.insert(instr.rs());
                }
                0x09 => {
                    //self.compile_jalr(instr),
                    self.regs.insert(instr.rs());
                    self.regs.insert(instr.rd());
                }
                0x0C => {
                    // self.compile_syscall(instr),
                    self.get_exception_regs();
                }
                0x0D => {
                    //self.compile_break(instr),
                    self.get_exception_regs();
                }
                0x10 => {
                    // self.compile_mfhi(instr),
                    self.regs.insert(REG_HI);
                    self.regs.insert(instr.rd());
                }
                0x11 => {
                    //self.compile_mthi(instr),
                    self.regs.insert(REG_HI);
                    self.regs.insert(instr.rs());
                }
                0x12 => {
                    //self.compile_mflo(instr),
                    self.regs.insert(REG_LO);
                    self.regs.insert(instr.rd());
                }
                0x13 => {
                    //self.compile_mtlo(instr),
                    self.regs.insert(REG_LO);
                    self.regs.insert(instr.rs());
                }
                0x1A => {
                    //self.compile_div(instr),
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rs());
                    self.regs.insert(REG_LO);
                    self.regs.insert(REG_HI);
                }
                0x1B => {
                    //self.compile_divu(instr),
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rs());
                    self.regs.insert(REG_LO);
                    self.regs.insert(REG_HI);
                }
                0x18 => {
                    //self.compile_mult(instr),
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rs());
                    self.regs.insert(REG_LO);
                    self.regs.insert(REG_HI);
                }
                0x19 => {
                    //self.compile_multu(instr),
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rs());
                    self.regs.insert(REG_LO);
                    self.regs.insert(REG_HI);
                }
                0x20 => {
                    //self.compile_add(instr),
                    self.get_exception_regs();
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rs());
                    self.regs.insert(instr.rd());
                }
                0x21 => {
                    //self.compile_addu(instr),
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rs());
                    self.regs.insert(instr.rd());
                }
                0x22 => {
                    //self.compile_sub(instr),
                    self.get_exception_regs();
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rs());
                    self.regs.insert(instr.rd());
                }
                0x23 => {
                    // self.compile_subu(instr),
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rs());
                    self.regs.insert(instr.rd());
                }
                0x24 => {
                    //self.compile_and(instr),
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rs());
                    self.regs.insert(instr.rd());
                }
                0x25 => {
                    //self.compile_or(instr),
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rs());
                    self.regs.insert(instr.rd());
                }
                0x26 => {
                    //self.compile_xor(instr),
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rs());
                    self.regs.insert(instr.rd());
                }
                0x27 => {
                    //self.compile_nor(instr),
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rs());
                    self.regs.insert(instr.rd());
                }
                0x2A => {
                    //self.compile_slt(instr),
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rs());
                    self.regs.insert(instr.rd());
                }
                0x2B => {
                    //self.compile_sltu(instr),
                    self.regs.insert(instr.rt());
                    self.regs.insert(instr.rs());
                    self.regs.insert(instr.rd());
                }
                _ => panic!(
                    "Unhandled secondary opcode: {:02X}, instr: {:X} addr: {:X}",
                    instr.secondary_opcode(),
                    instr.0,
                    self.get_current_pc()
                ),
            },
            0x01 => {
                //self.compile_bxx(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(31);
            }
            0x02 => { //self.compile_j(instr),
            }
            0x03 => {
                //self.compile_jal(instr),
                self.regs.insert(31);
            }
            0x04 => {
                //self.compile_beq(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
            }
            0x05 => {
                //self.compile_bne(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
            }
            0x06 => {
                //self.compile_blez(instr),
                self.regs.insert(instr.rs());
            }
            0x07 => {
                //self.compile_bgtz(instr),
                self.regs.insert(instr.rs());
            }
            0x08 => {
                //self.compile_addi(instr),
                self.get_exception_regs();
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
            }
            0x09 => {
                //self.compile_addiu(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
            }
            0x12 => {
                self.get_regs_cop2(instr);
            }
            0x0A => {
                //self.compile_slti(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
            }
            0x0B => {
                //self.compile_sltiu(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
            }
            0x0C => {
                // self.compile_andi(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
            }
            0x0D => {
                //self.compile_ori(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
            }
            0x0E => {
                //self.compile_xori(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
            }
            0x0F => {
                //self.compile_lui(instr),
                self.regs.insert(instr.rt());
            }
            0x10 => {
                self.get_regs_cop0(instr);
            }
            0x20 => {
                //self.compile_lb(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
            }
            0x21 => {
                //self.compile_lh(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
                self.get_exception_regs();
            }
            0x22 => {
                //self.compile_lwl(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
            }
            0x23 => {
                //self.compile_lw(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
                self.get_exception_regs();
            }
            0x24 => {
                //self.compile_lbu(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
            }
            0x25 => {
                //self.compile_lhu(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
                self.get_exception_regs();
            }
            0x26 => {
                //self.compile_lwr(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
            }
            0x28 => {
                //self.compile_sb(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
                self.regs.insert(REG_SR);
            }

            0x29 => {
                //self.compile_sh(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
                self.regs.insert(REG_SR);
                self.get_exception_regs();
            }
            0x2a => {
                //self.compile_swl(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
                self.regs.insert(REG_SR);
            }
            0x2B => {
                //self.compile_sw(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
                self.regs.insert(REG_SR);
                self.get_exception_regs();
            }
            0x2E => {
                //self.compile_swr(instr),
                self.regs.insert(instr.rs());
                self.regs.insert(instr.rt());
                self.regs.insert(REG_SR);
            }
            0x32 => {
                // self.compile_lwc2(instr),
                self.regs.insert(instr.rs());
                self.get_exception_regs();
            }
            0x3A => {
                //self.compile_swc2(instr),
                self.regs.insert(instr.rs());
                self.get_exception_regs();
                self.regs.insert(REG_SR);
            }
            _ => panic!(
                "Unhandled instruction: {:08X}, opcode: {:02X}, address: {:X}",
                instr.0,
                instr.opcode(),
                self.get_current_pc()
            ),
        }
    }

    fn get_exception_regs(&mut self) {
        self.regs.insert(REG_SR);
        self.regs.insert(REG_CAUSE);
        self.regs.insert(REG_EPC);
        self.regs.insert(REG_BADDR);
    }

    fn get_regs_cop0(&mut self, instr: Instruction) {
        match instr.cop_opcode() {
            0b00000 => {
                //self.compile_mfc0(instr),
                match instr.rd() {
                    6 => {} // jumpdest..
                    7 => {} // not used (0)
                    8 => {
                        self.regs.insert(REG_BADDR);
                    } // bad virtual address (R),
                    12 => {
                        self.regs.insert(REG_SR);
                    }
                    13 => {
                        self.regs.insert(REG_CAUSE);
                    }
                    14 => {
                        self.regs.insert(REG_EPC);
                    }
                    15 => {} // Processor ID
                    x => panic!("unhandled read from the cop0r{} register", x),
                }
                self.regs.insert(instr.rt());
            }
            0b00100 => {
                match instr.rd() {
                    3 | 5 | 6 | 7 | 9 | 11 => {
                        // breakpoint registers
                    }
                    8 => {
                        self.regs.insert(REG_BADDR);
                    }
                    12 => {
                        self.regs.insert(REG_SR);
                    } //self.sr = v,
                    13 => {
                        self.regs.insert(REG_CAUSE);
                    }
                    n => panic!("Unhandled cop0 register {:08X}", n),
                }

                self.regs.insert(instr.rt());
            }
            0b10000 => {
                self.regs.insert(REG_SR);
            }
            _ => panic!(
                "Unhandled cop0 instruction:  {:02X} ({:b})",
                instr.cop_opcode(),
                instr.cop_opcode()
            ),
        }
    }

    fn get_regs_cop2(&mut self, instr: Instruction) {
        let cop_opcode = instr.cop_opcode();

        if cop_opcode & 0x10 != 0 {
            // GTE command
        } else {
            match cop_opcode {
                0b00000 => {
                    //self.compile_mfc2(instr),
                    self.regs.insert(instr.rt());
                }

                0b00010 => {
                    //self.compile_cfc2(instr),
                    self.regs.insert(instr.rt());
                }
                0b00100 => {
                    //self.compile_mtc2(instr),
                    self.regs.insert(instr.rt());
                }
                0b00110 => {
                    //self.compile_ctc2(instr),
                    self.regs.insert(instr.rt());
                }
                _ => panic!(
                    "Unhandled cop2 instruction:  {:02X} ({:b})",
                    instr.cop_opcode(),
                    instr.cop_opcode()
                ),
            }
        }
    }

    fn compile_instr_in_delay_slot(&mut self, instr: Instruction) {
        self.delay_slot = true;
        self.compile_instr(instr);
        self.push_delay_load_to_regs();
        // if self.delayed_load.is_some() {
        // println!("missing 6 hot loads!!!");
        // self.compile_delayed_load();
        // }
        self.delay_slot = false;
    }

    fn inject_print_tty_output(&mut self) {
        let pc = self.get_current_pc() & 0x1FFFFFFF;
        let print_ = self.get_print_();
        if pc == 0xA0 {
            let reg = self.get_reg_read(9);
            let reg = self.bldr.use_var(reg);
            let is_equal = self.bldr.ins().icmp_imm_u(IntCC::Equal, reg, 0x3C);

            let then_block = self.bldr.create_block();
            let else_block = self.bldr.create_block();

            self.bldr
                .ins()
                .brif(is_equal, then_block, &[], else_block, &[]);

            self.bldr.switch_to_block(then_block);
            self.bldr.seal_block(then_block);

            let reg4 = self.get_reg_read(4);
            let reg4 = self.bldr.use_var(reg4);
            let reg4 = self.bldr.ins().ireduce(types::I8, reg4);
            self.bldr.ins().call(print_, &[reg4]);

            self.bldr.ins().jump(else_block, &[]);
            self.bldr.switch_to_block(else_block);
            self.bldr.seal_block(else_block);
        } else if pc == 0xB0 {
            let reg = self.get_reg_read(9);
            let reg = self.bldr.use_var(reg);
            let is_equal = self.bldr.ins().icmp_imm_u(IntCC::Equal, reg, 0x3D);

            let then_block = self.bldr.create_block();
            let else_block = self.bldr.create_block();

            self.bldr
                .ins()
                .brif(is_equal, then_block, &[], else_block, &[]);

            self.bldr.switch_to_block(then_block);
            self.bldr.seal_block(then_block);

            let reg4 = self.get_reg_read(4);
            let reg4 = self.bldr.use_var(reg4);
            let reg4 = self.bldr.ins().ireduce(types::I8, reg4);
            self.bldr.ins().call(print_, &[reg4]);

            self.bldr.ins().jump(else_block, &[]);
            self.bldr.switch_to_block(else_block);
            self.bldr.seal_block(else_block);
        }
        //     if (pc == 0xA0 && self.regs[9] ==  0x3C) |  (pc == 0xB0 && self.regs[9] ==  0x3D) {
        //         let ch = self.regs[4] as u8 as char;
        //         print!("{ch}");
        //     }
    }

    fn compile_entry_block(&mut self) {
        self.load_context();
    }

    fn load_context(&mut self) {
        let p = self
            .bldr
            .ins()
            .iconst(types::I64, self.constants.regs as usize as i64);
        for reg in self.regs.iter() {
            let var = self.bldr.declare_var(types::I32);
            self.vars.insert(*reg, var);
            let val = self
                .bldr
                .ins()
                .load(types::I32, MachMemFlags::trusted(), p, *reg as i32 * 4);
            self.bldr.def_var(var, val);
        }
        // for (reg, var) in self.vars.iter() {
        //     let val = self
        //         .bldr
        //         .ins()
        //         .load(types::I32, MachMemFlags::trusted(), p, *reg as i32 * 4);
        //     self.bldr.def_var(*var, val);
        // }
    }

    fn save_context(&mut self) {
        let p = self
            .bldr
            .ins()
            .iconst(types::I64, self.constants.regs as usize as i64);
        for reg in self.regs.iter() {
            let var = self.vars.get(reg).unwrap();
            let x = self.bldr.use_var(*var);
            self.bldr
                .ins()
                .store(MachMemFlags::trusted(), x, p, *reg as i32 * 4);
        }
        // for (reg, var) in self.vars.iter() {
        //     let x = self.bldr.use_var(*var);
        //     self.bldr
        //         .ins()
        //         .store(MachMemFlags::trusted(), x, p, *reg as i32 * 4);
        // }
    }

    fn get_current_pc(&self) -> u32 {
        self.addr + (self.i as u32 * 4)
    }

    fn get_cycles(&self) -> i64 {
        (self.i + 1) as i64
    }

    fn set_pc_imm(&mut self, pc: u32) {
        let x = self.bldr.ins().iconst(types::I32, pc as i64);
        self.set_pc(x);
    }

    fn set_pc(&mut self, pc: Value) {
        let p = self
            .bldr
            .ins()
            .iconst(types::I64, self.constants.regs as usize as i64);
        self.bldr
            .ins()
            .store(MachMemFlags::trusted(), pc, p, REG_PC as i32 * 4);
    }

    fn push_delay_load_to_regs(&mut self) {
        if let Some((reg, val)) = self.delayed_load {
            if reg != 0 {
                let delayed_load_reg = self.get_reg_write(REG_LOAD_REG);
                let delayed_load_val = self.get_reg_write(REG_LOAD_VAL);
                // let val = self.bldr.use_var(var);
                let reg = self.bldr.ins().iconst(types::I32, reg as i64);
                self.bldr.def_var(delayed_load_reg, reg);
                self.bldr.def_var(delayed_load_val, val);
                self.regs.insert(REG_LOAD_REG);
                self.regs.insert(REG_LOAD_VAL);
            }
            self.delayed_load = None;
        }
    }

    fn check_for_load_in_regs(&mut self, current_delayed_reg: Option<u32>) {
        if self.i == 0 {
            let regs_address = self
                .bldr
                .ins()
                .iconst(types::I64, self.constants.regs as usize as i64);
            let delayed_load_reg = self.get_reg_read(REG_LOAD_REG);
            let reg = self
                .bldr
                .ins()
                .load(types::I32, MachMemFlags::trusted(), regs_address, REG_LOAD_REG as i32 * 4);
            self.bldr.def_var(delayed_load_reg, reg);
            // let reg = self.bldr.use_var(delayed_load_reg);
            let delayed_load_val = self.get_reg_read(REG_LOAD_VAL);
            let delayed_load = self
                .bldr
                .ins()
                .load(types::I32, MachMemFlags::trusted(), regs_address, REG_LOAD_VAL as i32 * 4);
            self.bldr.def_var(delayed_load_val, delayed_load);
            // let delayed_load = self.bldr.use_var(delayed_load_val);
            let zero = self.bldr.ins().iconst(types::I32, 0);

            // if reg != 0 ...
            let then_block = self.bldr.create_block();
            let else_block = self.bldr.create_block();

            self.bldr.ins().brif(reg, then_block, &[], else_block, &[]);

            self.bldr.switch_to_block(then_block);
            self.bldr.seal_block(then_block);

            if let Some(current_reg) = current_delayed_reg {
                let val = self.bldr.ins().iconst(types::I32, current_reg as i64);
                let res = self.bldr.ins().icmp(IntCC::Equal, reg, val);

                let then_block = self.bldr.create_block();
                self.bldr.ins().brif(res, else_block, &[], then_block, &[]);
                self.bldr.switch_to_block(then_block);
                self.bldr.seal_block(then_block);
            }
            // ok now update the reg...

            // let mut pool = ValueListPool::with_capacity(32);
            // let mut blocks = vec![];
            let mut jtable = vec![];

            let raise_exception_block = self.bldr.create_block();
            let raise_exception = BlockCall::new(
                raise_exception_block,
                [],
                &mut self.bldr.func.dfg.value_lists,
            );
            jtable.push(raise_exception);

            for _ in 1..32 {
                let b = self.bldr.create_block();
                let bc = BlockCall::new(b, [], &mut self.bldr.func.dfg.value_lists);
                // blocks.push(b);
                jtable.push(bc);
            }

            let jump_table_data = JumpTableData::new(raise_exception, &jtable);
            let jump_table = self.bldr.create_jump_table(jump_table_data);
            self.bldr.ins().br_table(reg, jump_table);
            // self.bldr.ins().jump(else_block, &[]);
            // ---------------------------
            self.bldr.switch_to_block(raise_exception_block);
            self.bldr.seal_block(raise_exception_block);
            self.bldr.ins().trap(TrapCode::user(14).unwrap());
            for i in 1..32 {
                let b = jtable[i].block(&mut self.bldr.func.dfg.value_lists);
                self.bldr.switch_to_block(b);
                self.bldr.seal_block(b);
                // self.bldr.ins().trap(TrapCode::user(14).unwrap());
                let r = self.get_reg_write(i as u32);
                self.bldr.def_var(r, delayed_load);
                self.bldr.def_var(delayed_load_reg, zero);
                let target_reg_address = self.bldr.ins().iconst(types::I64, i as i64);
                let target_reg_address = self.bldr.ins().ishl_imm_u(target_reg_address, 2);
                let target_reg_address = self.bldr.ins().iadd(regs_address, target_reg_address);
                self.bldr
                    .ins()
                    .store(MachMemFlags::trusted(), delayed_load, target_reg_address, 0);
                self.bldr.ins().jump(else_block, &[]);
            }
            // ---------------------------
            self.regs.insert(REG_LOAD_REG);

            self.bldr.switch_to_block(else_block);
            self.bldr.seal_block(else_block);
        }
    }

    fn compile_delayed_load(&mut self, clear: bool) {
        self.check_for_load_in_regs(None);
        if let Some((reg, val)) = self.delayed_load {
            if reg != 0 {
                let variable = self
                    .vars
                    .entry(reg)
                    .or_insert_with(|| self.bldr.declare_var(types::I32));
                // let val = self.bldr.use_var(var);
                self.bldr.def_var(*variable, val);
            }
            if clear {
                self.delayed_load = None;
            }
        }
    }
    fn get_reg_from_delay_load(&mut self, r: u32) -> Value {
        // what to do with runtime loads?
        if let Some((reg, val)) = self.delayed_load {
            if reg == r {
                return val;
            }
        }
        let r = self.get_reg_read(r);
        self.bldr.use_var(r)
    }

    fn compile_delayed_load_chain(&mut self, reg: u32, val: Value) {
        self.check_for_load_in_regs(Some(reg));
        if let Some((pending_reg, pending_val)) = self.delayed_load {
            if pending_reg != reg && pending_reg != 0 {
                let variable = self
                    .vars
                    .entry(pending_reg)
                    .or_insert_with(|| self.bldr.declare_var(types::I32));
                // let val = self.bldr.use_var(pending_var);
                self.bldr.def_var(*variable, pending_val);
            }
        }
        self.delayed_load = Some((reg, val))
    }

    fn compile_exception(&mut self, reason: Exception, baddr: Option<Value>) {
        // let mode = self.sr & 0x3f;
        // self.sr &= !0x3f;
        // self.sr |= (mode << 2) & 0x3f;
        let sr_var = self.get_reg_read(REG_SR);
        let sr_val = self.bldr.use_var(sr_var);
        let mode = self.bldr.ins().band_imm_u(sr_val, 0x3f);
        let sr_val = self.bldr.ins().band_imm_u(sr_val, !0x3f);
        let mode = self.bldr.ins().ishl_imm_u(mode, 2);
        let mode = self.bldr.ins().band_imm_u(mode, 0x3f);
        let sr_val = self.bldr.ins().bor(sr_val, mode);
        self.bldr.def_var(sr_var, sr_val);

        // self.cause &= !0x7c;
        // self.cause |= (cause.code() as u32) << 2;
        let cause_var = self.get_reg_read(REG_CAUSE);
        let cause = self.bldr.use_var(cause_var);
        let cause = self.bldr.ins().band_imm_u(cause, !0x7C);
        let cause = self
            .bldr
            .ins()
            .bor_imm_u(cause, (reason.code() << 2) as i64);

        // if self.delay_slot {
        //     self.epc = self.current_pc.wrapping_sub(4);
        //     self.cause |= 1 << 31;
        // } else {
        //     self.epc = self.current_pc;
        //     self.cause &= !(1 << 31);
        // }
        let epc_var = self.get_reg_write(REG_EPC);
        let cause = if self.delay_slot {
            let pc = self.get_current_pc().wrapping_sub(4) as i64;
            let epc = self.bldr.ins().iconst(types::I32, pc);
            self.bldr.def_var(epc_var, epc);
            self.bldr.ins().bor_imm_u(cause, 1 << 31)
        } else {
            let pc = self.get_current_pc() as i64;
            let epc = self.bldr.ins().iconst(types::I32, pc);
            self.bldr.def_var(epc_var, epc);
            self.bldr.ins().band_imm_u(cause, !(1 << 31))
        };
        self.bldr.def_var(cause_var, cause);

        // exception handler address depends on the BEV bit
        // let handler: u32 = if self.sr & (1 << 22) != 0 {
        //     0xbfc00180
        //     } else {
        //     0x80000080
        // };
        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();
        let merge_block = self.bldr.create_block();
        self.bldr.append_block_param(merge_block, types::I32);

        let sr_bit = self.bldr.ins().band_imm_u(sr_val, 1 << 22);
        self.bldr
            .ins()
            .brif(sr_bit, then_block, &[], else_block, &[]);

        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        let handler = self.bldr.ins().iconst(types::I32, 0xfbc00180);
        self.bldr
            .ins()
            .jump(merge_block, &[BlockArg::Value(handler)]);

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);
        let handler = self.bldr.ins().iconst(types::I32, 0x80000080);
        self.bldr
            .ins()
            .jump(merge_block, &[BlockArg::Value(handler)]);

        self.bldr.switch_to_block(merge_block);
        self.bldr.seal_block(merge_block);
        let handler = self.bldr.block_params(merge_block)[0];

        if let Some(v) = baddr {
            let baddr = self.get_reg_write(REG_BADDR);
            self.bldr.def_var(baddr, v);
        }
        self.save_context();
        self.set_pc(handler);
        let cycles = self.get_cycles();
        let cycles = self.bldr.ins().iconst(types::I32, cycles);
        self.bldr.ins().return_(&[cycles]);
    }

    fn compile_j(&mut self, instr: Instruction) {
        let next_instr = self.next_instr.unwrap();
        // FIXME: panic if next instruction is any kind of jump..
        self.compile_delayed_load(true);
        self.compile_instr_in_delay_slot(next_instr);
        let next_pc = (self.addr & 0xf000_0000) | (instr.imm26() << 2);
        self.save_context();
        self.set_pc_imm(next_pc);
        let cycles = self.get_cycles() + 1;
        let cycles = self.bldr.ins().iconst(types::I32, cycles);
        self.bldr.ins().return_(&[cycles]);
        // self.next_pc = (self.current_pc & 0xf000_0000) | (instr.imm26() << 2);
        // self.branch = true;
        // self.delayed_load();
    }

    // jump register
    fn compile_jr(&mut self, instr: Instruction) {
        let reg = self.get_reg_read(instr.rs());
        let val = self.bldr.use_var(reg);

        let next_instr = self.next_instr.unwrap();
        // FIXME: panic if next instruction is any kind of jump..
        self.compile_delayed_load(true);
        self.compile_instr_in_delay_slot(next_instr);

        self.save_context();
        self.set_pc(val);
        let cycles = self.get_cycles() + 1;
        let cycles = self.bldr.ins().iconst(types::I32, cycles);
        self.bldr.ins().return_(&[cycles]);
        // self.next_pc = (self.current_pc & 0xf000_0000) | (instr.imm26() << 2);
        // self.branch = true;
        // self.delayed_load();
    }

    // jump and link register
    fn compile_jalr(&mut self, instr: Instruction) {
        let reg = self.get_reg_read(instr.rs());
        let val = self.bldr.use_var(reg);

        let next_instr = self.next_instr.unwrap();
        // FIXME: panic if next instruction is any kind of jump..
        self.compile_delayed_load(true);

        let ra = self.get_current_pc().wrapping_add(8);
        if instr.rd() != 0 {
            let target_reg = self.get_reg_write(instr.rd());
            let ra = self.bldr.ins().iconst(types::I32, ra as i64);
            self.bldr.def_var(target_reg, ra);
        }
        self.compile_instr_in_delay_slot(next_instr);

        self.save_context();
        self.set_pc(val);
        let cycles = self.get_cycles() + 1;
        let cycles = self.bldr.ins().iconst(types::I32, cycles);
        self.bldr.ins().return_(&[cycles]);
    }

    fn compile_jal(&mut self, instr: Instruction) {
        let next_instr = self.next_instr.unwrap();
        // FIXME: panic if next instruction is any kind of jump..
        self.compile_delayed_load(true);
        self.compile_instr_in_delay_slot(next_instr);
        let ra = self.get_current_pc().wrapping_add(8);
        let next_pc = (self.addr & 0xf000_0000) | (instr.imm26() << 2);
        let r31 = self.get_reg_write(31);
        let ra = self.bldr.ins().iconst(types::I32, ra as i64);
        self.bldr.def_var(r31, ra);
        self.save_context();
        self.set_pc_imm(next_pc);
        let cycles = self.get_cycles() + 1;
        let cycles = self.bldr.ins().iconst(types::I32, cycles);
        self.bldr.ins().return_(&[cycles]);
        // let ra = self.next_pc;
        // self.next_pc = (self.pc & 0xf0000000) | (instr.imm26() << 2);
        // self.delayed_load();
        // self.branch = true;
        // self.set_reg(31, ra);
    }

    fn compile_branch(&mut self, offset: u32) {
        let offset = offset << 2;

        let next_instr = self.next_instr.unwrap();
        // FIXME: panic if next instruction is any kind of jump..
        self.compile_instr_in_delay_slot(next_instr);

        // why wrapping add 4?????
        let next_pc = self.get_current_pc().wrapping_add(offset).wrapping_add(4);

        self.save_context();
        self.set_pc_imm(next_pc);
        let cycles = self.get_cycles() + 1;
        let cycles = self.bldr.ins().iconst(types::I32, cycles);
        self.bldr.ins().return_(&[cycles]);

        // self.next_pc = self.pc.wrapping_add(offset);
    }

    // branch not equal
    fn compile_bne(&mut self, instr: Instruction) {
        let rs_reg = self.get_reg_read(instr.rs());
        let rt_reg = self.get_reg_read(instr.rt());
        let rs_val = self.bldr.use_var(rs_reg);
        let rt_val = self.bldr.use_var(rt_reg);

        self.compile_delayed_load(true);

        let res = self.bldr.ins().icmp(IntCC::NotEqual, rs_val, rt_val);

        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();

        self.bldr.ins().brif(res, then_block, &[], else_block, &[]);

        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        self.compile_branch(instr.imm_se());

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);
        // if self.reg(instr.rs()) != self.reg(instr.rt()) {
        //     self.branch(instr.imm_se());
        // }
    }

    // branch if less or equal to zero
    fn compile_blez(&mut self, instr: Instruction) {
        let rs_reg = self.get_reg_read(instr.rs());
        let rs_val = self.bldr.use_var(rs_reg);

        self.compile_delayed_load(true);

        let zero = self.bldr.ins().iconst(types::I32, 0);
        let res = self
            .bldr
            .ins()
            .icmp(IntCC::SignedLessThanOrEqual, rs_val, zero);

        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();

        self.bldr.ins().brif(res, then_block, &[], else_block, &[]);

        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        self.compile_branch(instr.imm_se());

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);
    }

    // branch greater then zero
    fn compile_bgtz(&mut self, instr: Instruction) {
        let rs_reg = self.get_reg_read(instr.rs());
        let rs_val = self.bldr.use_var(rs_reg);

        self.compile_delayed_load(true);

        let zero = self.bldr.ins().iconst(types::I32, 0);
        let res = self.bldr.ins().icmp(IntCC::SignedGreaterThan, rs_val, zero);

        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();

        self.bldr.ins().brif(res, then_block, &[], else_block, &[]);

        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        self.compile_branch(instr.imm_se());

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);
    }

    // BGEZ branch if greater or equal to 0
    // BLTZ branch if less then 0
    // BGEZAL and link
    // BLTZAL and link
    fn compile_bxx(&mut self, instr: Instruction) {
        let instruction = instr.0;

        let is_bgez = ((instruction >> 16) & 1) == 1;
        let is_link = (instruction >> 17) & 0xf == 8;

        let rs_reg = self.get_reg_read(instr.rs());
        let rs_val = self.bldr.use_var(rs_reg);

        self.compile_delayed_load(true);

        let zero = self.bldr.ins().iconst(types::I32, 0);
        let cond = if is_bgez {
            IntCC::SignedGreaterThanOrEqual
        } else {
            IntCC::SignedLessThan
        };
        let res = self.bldr.ins().icmp(cond, rs_val, zero);

        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();

        if is_link {
            let ra = self.get_current_pc().wrapping_add(8);
            let r31 = self.get_reg_write(31);
            let ra = self.bldr.ins().iconst(types::I32, ra as i64);
            self.bldr.def_var(r31, ra);
        }

        self.bldr.ins().brif(res, then_block, &[], else_block, &[]);

        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        self.compile_branch(instr.imm_se());

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);
    }

    // branch equal
    fn compile_beq(&mut self, instr: Instruction) {
        let rs_reg = self.get_reg_read(instr.rs());
        let rt_reg = self.get_reg_read(instr.rt());
        let rs_val = self.bldr.use_var(rs_reg);
        let rt_val = self.bldr.use_var(rt_reg);

        self.compile_delayed_load(true);

        let res = self.bldr.ins().icmp(IntCC::Equal, rs_val, rt_val);

        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();

        self.bldr.ins().brif(res, then_block, &[], else_block, &[]);

        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        self.compile_branch(instr.imm_se());

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);
    }
    fn compile_add(&mut self, instr: Instruction) {
        let reg_a = self.get_reg_read(instr.rs());
        let reg_b = self.get_reg_read(instr.rt());
        let a = self.bldr.use_var(reg_a);
        let b = self.bldr.use_var(reg_b);
        let (res, overflow) = self.bldr.ins().sadd_overflow(a, b);
        self.compile_delayed_load(true);

        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();
        self.bldr
            .ins()
            .brif(overflow, then_block, &[], else_block, &[]);
        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        self.compile_exception(Exception::Overflow, None);

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);
        if instr.rd() != 0 {
            let dest = self.get_reg_write(instr.rd());
            self.bldr.def_var(dest, res);
        }
    }

    // add unsigned
    fn compile_addu(&mut self, instr: Instruction) {
        let reg_a = self.get_reg_read(instr.rs());
        let reg_b = self.get_reg_read(instr.rt());
        let a = self.bldr.use_var(reg_a);
        let b = self.bldr.use_var(reg_b);
        let res = self.bldr.ins().iadd(a, b);
        self.compile_delayed_load(true);

        if instr.rd() != 0 {
            let dest = self.get_reg_write(instr.rd());
            self.bldr.def_var(dest, res);
        }
    }

    fn compile_sub(&mut self, instr: Instruction) {
        let reg_a = self.get_reg_read(instr.rs());
        let reg_b = self.get_reg_read(instr.rt());
        let a = self.bldr.use_var(reg_a);
        let b = self.bldr.use_var(reg_b);
        let (res, overflow) = self.bldr.ins().ssub_overflow(a, b);
        self.compile_delayed_load(true);

        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();
        self.bldr
            .ins()
            .brif(overflow, then_block, &[], else_block, &[]);
        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        self.compile_exception(Exception::Overflow, None);

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);
        if instr.rd() != 0 {
            let dest = self.get_reg_write(instr.rd());
            self.bldr.def_var(dest, res);
        }
    }
    // substract unsigned
    fn compile_subu(&mut self, instr: Instruction) {
        let reg_a = self.get_reg_read(instr.rs());
        let reg_b = self.get_reg_read(instr.rt());
        let a = self.bldr.use_var(reg_a);
        let b = self.bldr.use_var(reg_b);
        let res = self.bldr.ins().isub(a, b);
        self.compile_delayed_load(true);

        if instr.rd() != 0 {
            let dest = self.get_reg_write(instr.rd());
            self.bldr.def_var(dest, res);
        }
    }

    fn compile_addi(&mut self, instr: Instruction) {
        let i = instr.imm_se();
        let src = self.get_reg_read(instr.rs());
        let v = self.bldr.use_var(src);
        let i = self.bldr.ins().iconst(types::I32, i as i64);
        let (res, overflow) = self.bldr.ins().sadd_overflow(v, i);
        self.compile_delayed_load(true);

        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();
        self.bldr
            .ins()
            .brif(overflow, then_block, &[], else_block, &[]);
        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        self.compile_exception(Exception::Overflow, None);

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);
        if instr.rt() != 0 {
            let dest = self.get_reg_write(instr.rt());
            self.bldr.def_var(dest, res);
        }
    }

    // add immediate unsigned
    fn compile_addiu(&mut self, instr: Instruction) {
        let i = instr.imm_se();
        let src = self.get_reg_read(instr.rs());
        let v = self.bldr.use_var(src);
        let res = self.bldr.ins().iadd_imm_s(v, i as i64);
        self.compile_delayed_load(true);
        if instr.rt() != 0 {
            let dest = self.get_reg_write(instr.rt());
            self.bldr.def_var(dest, res);
        }
        // let v = self.reg(instr.rs()).wrapping_add(i);
        // self.delayed_load();
        // self.set_reg(instr.rt(), v)
    }

    fn compile_divu(&mut self, instr: Instruction) {
        let reg_a = self.get_reg_read(instr.rs());
        let reg_b = self.get_reg_read(instr.rt());
        let n = self.bldr.use_var(reg_a);
        let d = self.bldr.use_var(reg_b);

        self.compile_delayed_load(true);

        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();
        let merge_block = self.bldr.create_block();

        let is_zero = self.bldr.ins().icmp_imm_u(IntCC::Equal, d, 0);
        self.bldr
            .ins()
            .brif(is_zero, then_block, &[], else_block, &[]);
        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);

        let hi_reg = self.get_reg_write(REG_HI);
        self.bldr.def_var(hi_reg, n);
        let lo_reg = self.get_reg_write(REG_LO);
        let foo = self.bldr.ins().iconst(types::I32, 0xffffffff);
        self.bldr.def_var(lo_reg, foo);

        self.bldr.ins().jump(merge_block, &[]);

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);
        let hi_reg = self.get_reg_write(REG_HI);
        let lo_reg = self.get_reg_write(REG_LO);

        let res = self.bldr.ins().udiv(n, d);
        self.bldr.def_var(lo_reg, res);

        let res = self.bldr.ins().urem(n, d);
        self.bldr.def_var(hi_reg, res);

        self.bldr.ins().jump(merge_block, &[]);

        self.bldr.switch_to_block(merge_block);
        self.bldr.seal_block(merge_block);
    }

    fn compile_multu(&mut self, instr: Instruction) {
        let reg_a = self.get_reg_read(instr.rs());
        let reg_b = self.get_reg_read(instr.rt());
        let a = self.bldr.use_var(reg_a);
        let b = self.bldr.use_var(reg_b);
        let a = self.bldr.ins().uextend(types::I64, a);
        let b = self.bldr.ins().uextend(types::I64, b);

        self.compile_delayed_load(true);

        let result = self.bldr.ins().imul(a, b);

        let hi_reg = self.get_reg_write(REG_HI);
        let lo_reg = self.get_reg_write(REG_LO);

        let lo_res = self.bldr.ins().ireduce(types::I32, result);
        let hi_res = self.bldr.ins().ushr_imm_u(result, 32);
        let hi_res = self.bldr.ins().ireduce(types::I32, hi_res);
        self.bldr.def_var(hi_reg, hi_res);
        self.bldr.def_var(lo_reg, lo_res);
    }
    fn compile_mult(&mut self, instr: Instruction) {
        let reg_a = self.get_reg_read(instr.rs());
        let reg_b = self.get_reg_read(instr.rt());
        let a = self.bldr.use_var(reg_a);
        let b = self.bldr.use_var(reg_b);
        let a = self.bldr.ins().sextend(types::I64, a);
        let b = self.bldr.ins().sextend(types::I64, b);

        self.compile_delayed_load(true);

        let result = self.bldr.ins().imul(a, b);

        let hi_reg = self.get_reg_write(REG_HI);
        let lo_reg = self.get_reg_write(REG_LO);

        let lo_res = self.bldr.ins().ireduce(types::I32, result);
        let hi_res = self.bldr.ins().ushr_imm_u(result, 32);
        let hi_res = self.bldr.ins().ireduce(types::I32, hi_res);
        self.bldr.def_var(hi_reg, hi_res);
        self.bldr.def_var(lo_reg, lo_res);
    }
    fn compile_div(&mut self, instr: Instruction) {
        let reg_a = self.get_reg_read(instr.rs());
        let reg_b = self.get_reg_read(instr.rt());
        let n = self.bldr.use_var(reg_a);
        let d = self.bldr.use_var(reg_b);

        self.compile_delayed_load(true);

        let is_zero_block = self.bldr.create_block();
        let not_zero_block = self.bldr.create_block();
        let merge_block = self.bldr.create_block();

        let is_zero = self.bldr.ins().icmp_imm_u(IntCC::Equal, d, 0);
        self.bldr
            .ins()
            .brif(is_zero, is_zero_block, &[], not_zero_block, &[]);
        self.bldr.switch_to_block(is_zero_block);
        self.bldr.seal_block(is_zero_block);

        let hi_reg = self.get_reg_write(REG_HI);
        self.bldr.def_var(hi_reg, n);
        let lo_reg = self.get_reg_write(REG_LO);
        // if n >= 0 {
        //     self.lo = 0xffffffff;
        // } else {
        //     self.lo = 1;
        // }
        // TODO: check that this is correct...
        let is_n_not_neg = self
            .bldr
            .ins()
            .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, n, 0);
        let is_n_not_neg = self.bldr.ins().uextend(types::I32, is_n_not_neg);
        let one = self.bldr.ins().iconst(types::I32, 0x1);
        let bar = self.bldr.ins().band_not(one, is_n_not_neg);
        let foo = self.bldr.ins().imul_imm_u(is_n_not_neg, 0xffffffff);
        let baz = self.bldr.ins().iadd(foo, bar);
        self.bldr.def_var(lo_reg, baz);

        self.bldr.ins().jump(merge_block, &[]);

        self.bldr.switch_to_block(not_zero_block);
        self.bldr.seal_block(not_zero_block);

        let n_is_large = self.bldr.ins().icmp_imm_u(IntCC::Equal, n, 0x80000000);
        let d_is_minus_one = self.bldr.ins().icmp_imm_s(IntCC::Equal, d, -1);
        let cond = self.bldr.ins().band(n_is_large, d_is_minus_one);
        let not_representable_block = self.bldr.create_block();
        let normal_div_block = self.bldr.create_block();
        self.bldr
            .ins()
            .brif(cond, not_representable_block, &[], normal_div_block, &[]);
        // else if n as u32 == 0x80000000 && d == -1 {
        //    self.hi = 0;
        //    self.lo = 0x80000000;
        self.bldr.switch_to_block(not_representable_block);
        self.bldr.seal_block(not_representable_block);
        let hi_reg = self.get_reg_write(REG_HI);
        let lo_reg = self.get_reg_write(REG_LO);

        let foo = self.bldr.ins().iconst(types::I32, 0x80000000);
        let zero = self.bldr.ins().iconst(types::I32, 0x00000000);

        self.bldr.def_var(lo_reg, foo);
        self.bldr.def_var(hi_reg, zero);

        self.bldr.ins().jump(merge_block, &[]);

        self.bldr.switch_to_block(normal_div_block);
        self.bldr.seal_block(normal_div_block);
        let hi_reg = self.get_reg_write(REG_HI);
        let lo_reg = self.get_reg_write(REG_LO);

        let res = self.bldr.ins().sdiv(n, d);
        self.bldr.def_var(lo_reg, res);

        let res = self.bldr.ins().srem(n, d);
        self.bldr.def_var(hi_reg, res);

        self.bldr.ins().jump(merge_block, &[]);

        self.bldr.switch_to_block(merge_block);
        self.bldr.seal_block(merge_block);
    }

    // mov from hi,
    fn compile_mfhi(&mut self, instr: Instruction) {
        self.compile_delayed_load(true);
        let hi = self.get_reg_read(REG_HI);
        let hi = self.bldr.use_var(hi);
        if instr.rd() != 0 {
            let reg = self.get_reg_write(instr.rd());
            self.bldr.def_var(reg, hi);
        }
    }

    fn compile_mflo(&mut self, instr: Instruction) {
        self.compile_delayed_load(true);
        let lo = self.get_reg_read(REG_LO);
        let lo = self.bldr.use_var(lo);
        if instr.rd() != 0 {
            let reg = self.get_reg_write(instr.rd());
            self.bldr.def_var(reg, lo);
        }
    }
    fn compile_mtlo(&mut self, instr: Instruction) {
        let reg = self.get_reg_read(instr.rs());
        let lo = self.get_reg_write(REG_LO);
        let reg = self.bldr.use_var(reg);
        self.compile_delayed_load(true);
        self.bldr.def_var(lo, reg);
    }
    fn compile_mthi(&mut self, instr: Instruction) {
        let reg = self.get_reg_read(instr.rs());
        let hi = self.get_reg_write(REG_HI);
        let reg = self.bldr.use_var(reg);
        self.compile_delayed_load(true);
        self.bldr.def_var(hi, reg);
    }
    /// set on less then signed
    fn compile_slt(&mut self, instr: Instruction) {
        let reg_a = self.get_reg_read(instr.rs());
        let reg_b = self.get_reg_read(instr.rt());

        let a = self.bldr.use_var(reg_a);
        let b = self.bldr.use_var(reg_b);

        let res = self.bldr.ins().icmp(IntCC::SignedLessThan, a, b);
        let res = self.bldr.ins().uextend(types::I32, res);
        self.compile_delayed_load(true);
        if instr.rd() != 0 {
            let dest = self.get_reg_write(instr.rd());
            self.bldr.def_var(dest, res);
        }
    }

    // set on less then unsigned
    fn compile_sltu(&mut self, instr: Instruction) {
        let reg_a = self.get_reg_read(instr.rs());
        let reg_b = self.get_reg_read(instr.rt());

        let a = self.bldr.use_var(reg_a);
        let b = self.bldr.use_var(reg_b);

        let res = self.bldr.ins().icmp(IntCC::UnsignedLessThan, a, b);
        let res = self.bldr.ins().uextend(types::I32, res);
        self.compile_delayed_load(true);
        if instr.rd() != 0 {
            let dest = self.get_reg_write(instr.rd());
            self.bldr.def_var(dest, res);
        }
    }
    // set on less then immediate
    fn compile_slti(&mut self, instr: Instruction) {
        let reg_a = self.get_reg_read(instr.rs());
        let i = instr.imm_se();

        let a = self.bldr.use_var(reg_a);
        let b = self.bldr.ins().iconst(types::I32, i as i64);

        let res = self.bldr.ins().icmp(IntCC::SignedLessThan, a, b);
        let res = self.bldr.ins().uextend(types::I32, res);
        self.compile_delayed_load(true);
        if instr.rt() != 0 {
            let dest = self.get_reg_write(instr.rt());
            self.bldr.def_var(dest, res);
        }
    }
    // set on less then immediate unsigned
    fn compile_sltiu(&mut self, instr: Instruction) {
        let reg_a = self.get_reg_read(instr.rs());
        let i = instr.imm_se();

        let a = self.bldr.use_var(reg_a);
        let b = self.bldr.ins().iconst(types::I32, i as i64);

        let res = self.bldr.ins().icmp(IntCC::UnsignedLessThan, a, b);
        let res = self.bldr.ins().uextend(types::I32, res);
        self.compile_delayed_load(true);
        if instr.rt() != 0 {
            let dest = self.get_reg_write(instr.rt());
            self.bldr.def_var(dest, res);
        }
    }

    // load upper immediate
    fn compile_lui(&mut self, instr: Instruction) {
        let v = instr.imm() << 16;
        self.compile_delayed_load(true);
        let reg = instr.rt();
        let ty = Type::int(32).unwrap();
        let variable = self.get_reg_read(reg);

        if reg != 0 {
            let val = self.bldr.ins().iconst(ty, v as i64);
            self.bldr.def_var(variable, val);
        }
    }

    fn compile_ori(&mut self, instr: Instruction) {
        let v = instr.imm();
        let src_reg = instr.rs();
        let target_reg = instr.rt();

        let src_reg_var = self.get_reg_read(src_reg);
        let src_reg_val = self.bldr.use_var(src_reg_var);
        let res_val = self.bldr.ins().bor_imm_u(src_reg_val, v as i64);

        self.compile_delayed_load(true);

        if target_reg != 0 {
            let res_var = self.get_reg_write(target_reg);
            self.bldr.def_var(res_var, res_val);
        }
    }

    fn compile_xori(&mut self, instr: Instruction) {
        let v = instr.imm();
        let src_reg = instr.rs();
        let target_reg = instr.rt();

        let src_reg_var = self.get_reg_read(src_reg);
        let src_reg_val = self.bldr.use_var(src_reg_var);
        let res_val = self.bldr.ins().bxor_imm_u(src_reg_val, v as i64);

        self.compile_delayed_load(true);

        if target_reg != 0 {
            let res_var = self.get_reg_write(target_reg);
            self.bldr.def_var(res_var, res_val);
        }
    }

    fn compile_and(&mut self, instr: Instruction) {
        let src0_reg = instr.rs();
        let src1_reg = instr.rt();

        let src0_reg_var = self.get_reg_read(src0_reg);
        let src1_reg_var = self.get_reg_read(src1_reg);
        let src0_reg_val = self.bldr.use_var(src0_reg_var);
        let src1_reg_val = self.bldr.use_var(src1_reg_var);
        let res_val = self.bldr.ins().band(src0_reg_val, src1_reg_val);

        self.compile_delayed_load(true);

        if instr.rd() != 0 {
            let res_var = self.get_reg_write(instr.rd());
            self.bldr.def_var(res_var, res_val);
        }
    }
    fn compile_andi(&mut self, instr: Instruction) {
        let i = instr.imm();

        let src_reg = self.get_reg_read(instr.rs());
        let src = self.bldr.use_var(src_reg);
        let val = self.bldr.ins().iconst(types::I32, i as i64);
        let res_val = self.bldr.ins().band(val, src);

        self.compile_delayed_load(true);

        if instr.rt() != 0 {
            let res_var = self.get_reg_write(instr.rt());
            self.bldr.def_var(res_var, res_val);
        }
    }
    fn compile_or(&mut self, instr: Instruction) {
        let src0_reg = instr.rs();
        let src1_reg = instr.rt();

        let src0_reg_var = self.get_reg_read(src0_reg);
        let src1_reg_var = self.get_reg_read(src1_reg);
        let src0_reg_val = self.bldr.use_var(src0_reg_var);
        let src1_reg_val = self.bldr.use_var(src1_reg_var);
        let res_val = self.bldr.ins().bor(src0_reg_val, src1_reg_val);

        self.compile_delayed_load(true);

        if instr.rd() != 0 {
            let res_var = self.get_reg_write(instr.rd());
            self.bldr.def_var(res_var, res_val);
        }
    }

    fn compile_nor(&mut self, instr: Instruction) {
        let src0_reg = instr.rs();
        let src1_reg = instr.rt();

        let src0_reg_var = self.get_reg_read(src0_reg);
        let src1_reg_var = self.get_reg_read(src1_reg);
        let src0_reg_val = self.bldr.use_var(src0_reg_var);
        let src1_reg_val = self.bldr.use_var(src1_reg_var);
        let res_val = self.bldr.ins().bor(src0_reg_val, src1_reg_val);
        let res_val = self.bldr.ins().bnot(res_val);

        self.compile_delayed_load(true);

        if instr.rd() != 0 {
            let res_var = self.get_reg_write(instr.rd());
            self.bldr.def_var(res_var, res_val);
        }
    }

    fn compile_xor(&mut self, instr: Instruction) {
        let src0_reg = instr.rs();
        let src1_reg = instr.rt();

        let src0_reg_var = self.get_reg_read(src0_reg);
        let src1_reg_var = self.get_reg_read(src1_reg);
        let src0_reg_val = self.bldr.use_var(src0_reg_var);
        let src1_reg_val = self.bldr.use_var(src1_reg_var);
        let res_val = self.bldr.ins().bxor(src0_reg_val, src1_reg_val);

        self.compile_delayed_load(true);

        if instr.rd() != 0 {
            let res_var = self.get_reg_write(instr.rd());
            self.bldr.def_var(res_var, res_val);
        }
    }

    fn compile_sll(&mut self, instr: Instruction) {
        let i = instr.imm5();

        let value_reg = self.get_reg_read(instr.rt());
        let value = self.bldr.use_var(value_reg);
        let result = self.bldr.ins().ishl_imm_u(value, i as i64);
        self.compile_delayed_load(true);
        if instr.rd() != 0 {
            let target_reg = self.get_reg_write(instr.rd());
            self.bldr.def_var(target_reg, result);
        }
    }

    fn compile_sllv(&mut self, instr: Instruction) {
        let value_reg = self.get_reg_read(instr.rt());
        let shift_reg = self.get_reg_read(instr.rs());
        let shift = self.bldr.use_var(shift_reg);
        let shift = self.bldr.ins().band_imm_u(shift, 0x1f);
        let value = self.bldr.use_var(value_reg);
        let result = self.bldr.ins().ishl(value, shift);
        self.compile_delayed_load(true);
        if instr.rd() != 0 {
            let target_reg = self.get_reg_write(instr.rd());
            self.bldr.def_var(target_reg, result);
        }
    }

    fn compile_srlv(&mut self, instr: Instruction) {
        let value_reg = self.get_reg_read(instr.rt());
        let shift_reg = self.get_reg_read(instr.rs());
        let shift = self.bldr.use_var(shift_reg);
        let shift = self.bldr.ins().band_imm_u(shift, 0x1f);
        let value = self.bldr.use_var(value_reg);
        let result = self.bldr.ins().ushr(value, shift);
        self.compile_delayed_load(true);
        if instr.rd() != 0 {
            let target_reg = self.get_reg_write(instr.rd());
            self.bldr.def_var(target_reg, result);
        }
    }

    // shift right arithmetic
    fn compile_sra(&mut self, instr: Instruction) {
        let i = instr.imm5();

        let value_reg = self.get_reg_read(instr.rt());
        let value = self.bldr.use_var(value_reg);
        let result = self.bldr.ins().sshr_imm_u(value, i as i64);
        self.compile_delayed_load(true);
        if instr.rd() != 0 {
            let target_reg = self.get_reg_write(instr.rd());
            self.bldr.def_var(target_reg, result);
        }
    }
    // shift right arythmetic value
    fn compile_srav(&mut self, instr: Instruction) {
        let value_reg = self.get_reg_read(instr.rt());
        let shift_reg = self.get_reg_read(instr.rs());
        let shift = self.bldr.use_var(shift_reg);
        let shift = self.bldr.ins().band_imm_u(shift, 0x1f);

        let value = self.bldr.use_var(value_reg);
        let result = self.bldr.ins().sshr(value, shift);
        self.compile_delayed_load(true);
        if instr.rd() != 0 {
            let target_reg = self.get_reg_write(instr.rd());
            self.bldr.def_var(target_reg, result);
        }
    }
    // shift right logical
    fn compile_srl(&mut self, instr: Instruction) {
        let i = instr.imm5();

        let value_reg = self.get_reg_read(instr.rt());
        let value = self.bldr.use_var(value_reg);
        let result = self.bldr.ins().ushr_imm_u(value, i as i64);
        self.compile_delayed_load(true);
        if instr.rd() != 0 {
            let target_reg = self.get_reg_write(instr.rd());
            self.bldr.def_var(target_reg, result);
        }
    }

    fn get_reg_write(&mut self, reg: u32) -> Variable {
        let result = self
            .vars
            .entry(reg)
            .or_insert_with(|| self.bldr.declare_var(types::I32));
        *result
    }

    fn get_reg_read(&mut self, reg: u32) -> Variable {
        let res = self.vars.entry(reg).or_insert_with(|| {
            let v = self.bldr.declare_var(types::I32);
            // let p = self
            //     .bldr
            //     .ins()
            //     .iconst(types::I64, self.constants.regs as usize as i64);
            // let val = self
            //     .bldr
            //     .ins()
            //     .load(types::I32, MachMemFlags::trusted(), p, reg as i32 * 4);
            // self.bldr.def_var(v, val);
            v
        });
        *res
    }

    fn compile_sw(&mut self, instr: Instruction) {
        let v = instr.imm_se();
        let addr_reg = instr.rs();
        let val_reg = instr.rt();

        let addr_reg_var = self.get_reg_read(addr_reg);

        let val_reg_var = self.get_reg_read(val_reg);
        let imm_val = self.bldr.ins().iconst(types::I32, v as i64);
        let addr_val = self.bldr.use_var(addr_reg_var);

        let address = self.bldr.ins().iadd(imm_val, addr_val);
        let val = self.bldr.use_var(val_reg_var);

        self.compile_delayed_load(true);

        let addr = self.bldr.ins().band_imm_u(address, 0x3);

        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();
        self.bldr.ins().brif(addr, then_block, &[], else_block, &[]);
        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        self.compile_exception(Exception::StoreAddressError, Some(address));

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);

        let sr = self.get_reg_read(REG_SR);
        let sr = self.bldr.use_var(sr);
        let sr = self.bldr.ins().band_imm_u(sr, 0x10000);
        // if self.sr & 0x10000 != 0 {
        //     // println!("Ignoring store while cache is isolated");
        //     return;
        // }
        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();
        self.bldr.ins().brif(sr, then_block, &[], else_block, &[]);
        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);

        let system = self
            .bldr
            .ins()
            .iconst(types::I64, self.constants.system as usize as i64);
        let store_word = self.get_store_word();
        self.bldr.ins().call(store_word, &[system, address, val]);
        self.bldr.ins().jump(then_block, &[]);

        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);

        // let star = self.bldr.ins().iconst(types::I8, '*' as i64);
        //     self.bldr
        //         .ins()
        //         .call(self.print_, &[star]);
        // self.bldr.ins().jump(merge_block, &[]);

        // do nothing....
    }

    // store half word
    fn compile_sh(&mut self, instr: Instruction) {
        let v = instr.imm_se();
        let addr_reg = instr.rs();
        let val_reg = instr.rt();

        let addr_reg_var = self.get_reg_read(addr_reg);

        let val_reg_var = self.get_reg_read(val_reg);
        let imm_val = self.bldr.ins().iconst(types::I32, v as i64);
        let addr_val = self.bldr.use_var(addr_reg_var);

        let address = self.bldr.ins().iadd(imm_val, addr_val);
        let val = self.bldr.use_var(val_reg_var);
        let val = self.bldr.ins().ireduce(types::I16, val);

        self.compile_delayed_load(true);

        let addr = self.bldr.ins().band_imm_u(address, 0x1);

        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();
        self.bldr.ins().brif(addr, then_block, &[], else_block, &[]);
        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        self.compile_exception(Exception::StoreAddressError, Some(address));

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);

        let sr = self.get_reg_read(REG_SR);
        let sr = self.bldr.use_var(sr);
        let sr = self.bldr.ins().band_imm_u(sr, 0x10000);
        // if self.sr & 0x10000 != 0 {
        //     // println!("Ignoring store while cache is isolated");
        //     return;
        // }
        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();
        self.bldr.ins().brif(sr, then_block, &[], else_block, &[]);
        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);

        let system = self
            .bldr
            .ins()
            .iconst(types::I64, self.constants.system as usize as i64);
        let store_half_word = self.get_store_half_word();
        self.bldr
            .ins()
            .call(store_half_word, &[system, address, val]);
        self.bldr.ins().jump(then_block, &[]);

        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        // do nothing....
    }

    // store byte
    fn compile_sb(&mut self, instr: Instruction) {
        let v = instr.imm_se();
        let addr_reg = instr.rs();
        let val_reg = instr.rt();

        let addr_reg_var = self.get_reg_read(addr_reg);

        let val_reg_var = self.get_reg_read(val_reg);
        let imm_val = self.bldr.ins().iconst(types::I32, v as i64);
        let addr_val = self.bldr.use_var(addr_reg_var);

        let address = self.bldr.ins().iadd(imm_val, addr_val);
        let val = self.bldr.use_var(val_reg_var);
        let val = self.bldr.ins().ireduce(types::I8, val);

        self.compile_delayed_load(true);

        let sr = self.get_reg_read(REG_SR);
        let sr = self.bldr.use_var(sr);
        let sr = self.bldr.ins().band_imm_u(sr, 0x10000);
        // if self.sr & 0x10000 != 0 {
        //     return;
        // }
        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();
        self.bldr.ins().brif(sr, then_block, &[], else_block, &[]);
        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);

        let system = self
            .bldr
            .ins()
            .iconst(types::I64, self.constants.system as usize as i64);
        let store_byte = self.get_store_byte();
        self.bldr.ins().call(store_byte, &[system, address, val]);
        self.bldr.ins().jump(then_block, &[]);

        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        // do nothing....
    }

    fn compile_lh(&mut self, instr: Instruction) {
        let v = instr.imm_se();

        let addr_reg = self.get_reg_read(instr.rs());

        // let result_reg = self.get_reg_read(instr.rt());

        let imm_val = self.bldr.ins().iconst(types::I32, v as i64);
        let addr = self.bldr.use_var(addr_reg);

        let address = self.bldr.ins().iadd(imm_val, addr);

        let addr = self.bldr.ins().band_imm_u(address, 0x1);

        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();
        self.bldr.ins().brif(addr, then_block, &[], else_block, &[]);
        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        self.compile_delayed_load(false);
        self.compile_exception(Exception::LoadAddressError, Some(address));

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);

        // let system = self
        //     .bldr
        //     .ins()
        //     .iconst(types::I64, self.constants.system as usize as i64);
        // let load_half_word = self.get_load_half_word();
        // let res = self.bldr.ins().call(load_half_word, &[system, address]);
        // let v = self.bldr.inst_results(res)[0];
        let v = self.compile_load(types::I16, address);
        let v = self.bldr.ins().sextend(types::I32, v);
        self.compile_delayed_load_chain(instr.rt(), v);
    }

    fn compile_lhu(&mut self, instr: Instruction) {
        let v = instr.imm_se();

        let addr_reg = self.get_reg_read(instr.rs());

        let imm_val = self.bldr.ins().iconst(types::I32, v as i64);
        let addr = self.bldr.use_var(addr_reg);

        let address = self.bldr.ins().iadd(imm_val, addr);

        let addr = self.bldr.ins().band_imm_u(address, 0x1);

        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();
        self.bldr.ins().brif(addr, then_block, &[], else_block, &[]);
        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        self.compile_delayed_load(false);
        self.compile_exception(Exception::LoadAddressError, Some(address));

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);

        // let system = self
        //     .bldr
        //     .ins()
        //     .iconst(types::I64, self.constants.system as usize as i64);
        // let load_half_word = self.get_load_half_word();
        // let res = self.bldr.ins().call(load_half_word, &[system, address]);
        // let v = self.bldr.inst_results(res)[0];
        let v = self.compile_load(types::I16, address);
        let v = self.bldr.ins().uextend(types::I32, v);
        self.compile_delayed_load_chain(instr.rt(), v);
    }

// const REGION_MASK: [u32; 8] = [
//     0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff, // KUSEG: 2048MB,
//     0x7fffffff, // KSEG0: 512MB,
//     0x1fffffff, // KSEG1: 512MB,
//     0xffffffff, 0xffffffff, // KSEG2: 1024MB, cache cotnrol hw registers
// ];
// pub fn mask_region(addr: u32) -> u32 {
//     let index = (addr >> 29) as usize;
//     addr & REGION_MASK[index]
// }
    // now make polymorphic...
    fn compile_load(&mut self, ty: Type, address: Value) -> Value {
        //self.bldr.func.dfg.value_type(my_value);
        // self.bldr.func.
        let index = self.bldr.ins().sshr_imm_u(address, 29);
        let kuseg_test = self.bldr.ins().iconst(types::I32, 0b100);
        let is_kuseg = self.bldr.ins().band_not(kuseg_test, index);
        let is_kseg2 = self.bldr.ins().band_imm_u(index, 0b010);
        let is_kseg1 = self.bldr.ins().band_imm_u(index, 0b001);



        let kseg0_mask = self.bldr.ins().iconst(types::I32, 0x7fffffff);
        let kseg1_mask = self.bldr.ins().iconst(types::I32, 0x1fffffff);
        let other_mask = self.bldr.ins().select(is_kseg1, kseg1_mask, kseg0_mask);

        let kuseg_mask = self.bldr.ins().iconst(types::I32, 0xffffffff);
        let not_kuseg_mask = self.bldr.ins().select(is_kseg2, kuseg_mask, other_mask);

        let mask = self.bldr.ins().select(is_kuseg, kuseg_mask, not_kuseg_mask);

        let masked_address = self.bldr.ins().band(address, mask);
        let masked_address = self.bldr.ins().uextend(types::I64, masked_address);

        // now check if we in ram:
        // pub const RAM: Range = Range(0x0000_0000, 2 * 1024 * 1024);
        let is_ram = self.bldr.ins().icmp_imm_u(IntCC::UnsignedLessThan, masked_address, 2 * 1024 * 1024);


        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();
        let merge_block = self.bldr.create_block();
        self.bldr.append_block_param(merge_block, ty);

        self.bldr.ins().brif(is_ram, then_block, &[], else_block, &[]);
        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        let p = self
            .bldr
            .ins()
            .iconst(types::I64, self.constants.ram as *mut u8 as usize as i64);
        let p = self.bldr.ins().iadd(p, masked_address);
            let val = self
                .bldr
                .ins()
                .load(ty, MachMemFlags::trusted(), p, 0);

        self.bldr.ins().jump(merge_block, &[BlockArg::Value(val)]);

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);

        let system = self
            .bldr
            .ins()
            .iconst(types::I64, self.constants.system as i64);
        match ty {
            types::I32 => {
                let load_word = self.get_load_word();
                let res = self.bldr.ins().call(load_word, &[system, address]);
                let res = self.bldr.inst_results(res)[0];
                self.bldr.ins().jump(merge_block, &[BlockArg::Value(res)]);
            },
            types::I16 => {
                let load_half_word = self.get_load_half_word();
                let res = self.bldr.ins().call(load_half_word, &[system, address]);
                let res = self.bldr.inst_results(res)[0];
                self.bldr.ins().jump(merge_block, &[BlockArg::Value(res)]);
            },
            types::I8 => {
                let load_byte = self.get_load_byte();
                let res = self.bldr.ins().call(load_byte, &[system, address]);
                let res = self.bldr.inst_results(res)[0];
                self.bldr.ins().jump(merge_block, &[BlockArg::Value(res)]);
            },
            _ => {
                panic!("unsupported type: {}", ty);
            }
        };

        self.bldr.switch_to_block(merge_block);
        self.bldr.seal_block(merge_block);
        self.bldr.block_params(merge_block)[0]
    }

    fn compile_lw(&mut self, instr: Instruction) {
        let v = instr.imm_se();

        let addr_reg = self.get_reg_read(instr.rs());

        // let result_reg = self.get_reg_read(instr.rt());

        let imm_val = self.bldr.ins().iconst(types::I32, v as i64);
        let addr = self.bldr.use_var(addr_reg);

        let address = self.bldr.ins().iadd(imm_val, addr);

        let addr = self.bldr.ins().band_imm_u(address, 0x3);

        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();
        self.bldr.ins().brif(addr, then_block, &[], else_block, &[]);
        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        self.compile_delayed_load(false);
        self.compile_exception(Exception::LoadAddressError, Some(address));

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);

        // let system = self
        //     .bldr
        //     .ins()
        //     .iconst(types::I64, self.constants.system as i64);
        // let load_word = self.get_load_word();
        // let res = self.bldr.ins().call(load_word, &[system, address]);
        // let v = self.bldr.inst_results(res)[0];
        let v = self.compile_load(types::I32, address);
        self.compile_delayed_load_chain(instr.rt(), v);
    }

    // load word left
    fn compile_lwl(&mut self, instr: Instruction) {
        let v = instr.imm_se();

        let addr_reg = self.get_reg_read(instr.rs());

        // let result_reg = self.get_reg_read(instr.rt());

        let imm_val = self.bldr.ins().iconst(types::I32, v as i64);
        let addr = self.bldr.use_var(addr_reg);

        let address = self.bldr.ins().iadd(imm_val, addr);
        // let address = self.bldr.ins().band_imm_u(address, 0x3);

        let aligned_address = self.bldr.ins().band_imm_u(address, !0x3);

        // This instruction bypasses the load delay restriction: this instruction will merge the new
        // contents with the value currently being loaded if need be.
        // self.compile_delayed_load(false);

        let curr_v = self.get_reg_from_delay_load(instr.rt());
        // let curr_v = self.bldr.use_var(curr_v);

        // let system = self
        //     .bldr
        //     .ins()
        //     .iconst(types::I64, self.constants.system as i64);
        // let load_word = self.get_load_word();
        // let res = self.bldr.ins().call(load_word, &[system, aligned_address]);
        // let v = self.bldr.inst_results(res)[0];
        let v = self.compile_load(types::I32, aligned_address);
        // now interesting part
        // TODO: verify me
        let curr_mask = self.bldr.ins().iconst(types::I32, 0x00ffffff);
        let bits = self.bldr.ins().band_imm_u(address, 0x3);
        let bits_x2 = self.bldr.ins().imul_imm_u(bits, 8);
        let curr_mask = self.bldr.ins().ushr(curr_mask, bits_x2);
        let curr_v = self.bldr.ins().band(curr_v, curr_mask);

        let three = self.bldr.ins().iconst(types::I32, 0x3);
        let inv_bits = self.bldr.ins().isub(three, bits);
        let aligned_shift = self.bldr.ins().imul_imm_u(inv_bits, 8);

        let v = self.bldr.ins().ishl(v, aligned_shift);

        let v = self.bldr.ins().bor(curr_v, v);

        // let v = match addr & 3 {
        //     0 => (cur_v & 0x00ffffff) | (aligned_word << 24),
        //     1 => (cur_v & 0x0000ffff) | (aligned_word << 16),
        //     2 => (cur_v & 0x000000ff) | (aligned_word << 8),
        //     3 => (cur_v & 0x00000000) | (aligned_word),
        //     _ => unreachable!()
        // };
        self.compile_delayed_load_chain(instr.rt(), v);
    }

    // load word right
    fn compile_lwr(&mut self, instr: Instruction) {
        let v = instr.imm_se();

        let addr_reg = self.get_reg_read(instr.rs());

        // let result_reg = self.get_reg_read(instr.rt());

        let imm_val = self.bldr.ins().iconst(types::I32, v as i64);
        let addr = self.bldr.use_var(addr_reg);

        let address = self.bldr.ins().iadd(imm_val, addr);
        // let address = self.bldr.ins().band_imm_u(address, 0x3);

        let aligned_address = self.bldr.ins().band_imm_u(address, !0x3);

        // This instruction bypasses the load delay restriction: this instruction will merge the new
        // contents with the value currently being loaded if need be.
        // self.compile_delayed_load(false);

        let curr_v = self.get_reg_from_delay_load(instr.rt());
        // let curr_v = self.get_reg_read(instr.rt());
        // let curr_v = self.bldr.use_var(curr_v);

        // let system = self
        //     .bldr
        //     .ins()
        //     .iconst(types::I64, self.constants.system as i64);
        // let load_word = self.get_load_word();
        // let res = self.bldr.ins().call(load_word, &[system, aligned_address]);
        // let v = self.bldr.inst_results(res)[0];
        let v = self.compile_load(types::I32, aligned_address);
        // now interesting part
        // TODO: verify me
        let curr_mask = self.bldr.ins().iconst(types::I32, 0xffffff00);
        let bits = self.bldr.ins().band_imm_u(address, 0x3);
        let three = self.bldr.ins().iconst(types::I32, 0x3);
        let inv_bits = self.bldr.ins().isub(three, bits);
        let bits_x2 = self.bldr.ins().imul_imm_u(inv_bits, 8);
        let curr_mask = self.bldr.ins().ishl(curr_mask, bits_x2);
        let curr_v = self.bldr.ins().band(curr_v, curr_mask);

        let aligned_shift = self.bldr.ins().imul_imm_u(bits, 8);

        let v = self.bldr.ins().ushr(v, aligned_shift);

        let v = self.bldr.ins().bor(curr_v, v);

        // let v = match addr & 3 {
        //     0 => (cur_v & 0x00000000) | (aligned_word),
        //     1 => (cur_v & 0xff000000) | (aligned_word >> 8),
        //     2 => (cur_v & 0xffff0000) | (aligned_word >> 16),
        //     3 => (cur_v & 0xffffff00) | (aligned_word >> 24),
        //     _ => unreachable!()
        // };
        self.compile_delayed_load_chain(instr.rt(), v);
    }

    // store word left
    fn compile_swl(&mut self, instr: Instruction) {
        let addr_reg = self.get_reg_read(instr.rs());

        let imm_val = self.bldr.ins().iconst(types::I32, instr.imm_se() as i64);
        let addr = self.bldr.use_var(addr_reg);

        let address = self.bldr.ins().iadd(imm_val, addr);

        let aligned_address = self.bldr.ins().band_imm_u(address, !0x3);

        let curr_v = self.get_reg_read(instr.rt());
        let curr_v = self.bldr.use_var(curr_v);

        let system = self
            .bldr
            .ins()
            .iconst(types::I64, self.constants.system as i64);
        // let load_word = self.get_load_word();
        // let res = self.bldr.ins().call(load_word, &[system, aligned_address]);
        // let curr_memory = self.bldr.inst_results(res)[0];
        let curr_memory = self.compile_load(types::I32, aligned_address);
        // now interesting part
        // TODO: verify me
        let curr_mask = self.bldr.ins().iconst(types::I32, 0xffffff00);
        let bits = self.bldr.ins().band_imm_u(address, 0x3);
        let bits_x2 = self.bldr.ins().imul_imm_u(bits, 8);
        let curr_mask = self.bldr.ins().ishl(curr_mask, bits_x2);
        let curr_memory = self.bldr.ins().band(curr_memory, curr_mask);

        let three = self.bldr.ins().iconst(types::I32, 0x3);
        let inv_bits = self.bldr.ins().isub(three, bits);
        let aligned_shift = self.bldr.ins().imul_imm_u(inv_bits, 8);

        let curr_v = self.bldr.ins().ushr(curr_v, aligned_shift);

        let v = self.bldr.ins().bor(curr_v, curr_memory);

        self.compile_delayed_load(true);

        // FIXME: where's SR check?

        let store_word = self.get_store_word();
        self.bldr
            .ins()
            .call(store_word, &[system, aligned_address, v]);

        // let mem = match addr & 3 {
        //     0 => (cur_mem & 0xffffff00) | (v >> 24),
        //     1 => (cur_mem & 0xffff0000) | (v >> 16),
        //     2 => (cur_mem & 0xff000000) | (v >> 8),
        //     3 => (cur_mem & 0x00000000) | (v),
        //     _ => unreachable!()
        // };
    }

    fn compile_swr(&mut self, instr: Instruction) {
        let addr_reg = self.get_reg_read(instr.rs());

        let imm_val = self.bldr.ins().iconst(types::I32, instr.imm_se() as i64);
        let addr = self.bldr.use_var(addr_reg);

        let address = self.bldr.ins().iadd(imm_val, addr);

        let aligned_address = self.bldr.ins().band_imm_u(address, !0x3);

        let curr_v = self.get_reg_read(instr.rt());
        let curr_v = self.bldr.use_var(curr_v);

        let system = self
            .bldr
            .ins()
            .iconst(types::I64, self.constants.system as i64);
        // let load_word = self.get_load_word();
        // let res = self.bldr.ins().call(load_word, &[system, aligned_address]);
        // let curr_memory = self.bldr.inst_results(res)[0];
        let curr_memory = self.compile_load(types::I32, aligned_address);
        // now interesting part
        // TODO: verify me
        let curr_mask = self.bldr.ins().iconst(types::I32, 0x00ffffff);
        let bits = self.bldr.ins().band_imm_u(address, 0x3);
        let three = self.bldr.ins().iconst(types::I32, 0x3);
        let inv_bits = self.bldr.ins().isub(three, bits);
        let bits_x2 = self.bldr.ins().imul_imm_u(inv_bits, 8);
        let curr_mask = self.bldr.ins().ushr(curr_mask, bits_x2);
        let curr_memory = self.bldr.ins().band(curr_memory, curr_mask);

        let aligned_shift = self.bldr.ins().imul_imm_u(bits, 8);

        let curr_v = self.bldr.ins().ishl(curr_v, aligned_shift);

        let v = self.bldr.ins().bor(curr_v, curr_memory);

        self.compile_delayed_load(true);

        let store_word = self.get_store_word();
        self.bldr
            .ins()
            .call(store_word, &[system, aligned_address, v]);

        // let mem = match addr & 3 {
        //     0 => (cur_mem & 0x00000000) | (v),
        //     1 => (cur_mem & 0x000000ff) | (v << 8),
        //     2 => (cur_mem & 0x0000ffff) | (v << 16),
        //     3 => (cur_mem & 0x00ffffff) | (v << 24),
        //     _ => unreachable!()
        // };
    }

    fn compile_lb(&mut self, instr: Instruction) {
        let v = instr.imm_se();

        let addr_reg = self.get_reg_read(instr.rs());

        let imm_val = self.bldr.ins().iconst(types::I32, v as i64);
        let addr = self.bldr.use_var(addr_reg);

        let address = self.bldr.ins().iadd(imm_val, addr);

        // let system = self
        //     .bldr
        //     .ins()
        //     .iconst(types::I64, self.constants.system as usize as i64);
        // let load_byte = self.get_load_byte();
        // let res = self.bldr.ins().call(load_byte, &[system, address]);
        // let v = self.bldr.inst_results(res)[0];
        let v = self.compile_load(types::I8, address);
        let v = self.bldr.ins().sextend(types::I32, v);
        // let v = self.bldr.ins().uextend(types::I32, v);

        // do we sign extend it??? (yes)

        self.compile_delayed_load_chain(instr.rt(), v);
    }

    fn compile_lbu(&mut self, instr: Instruction) {
        let v = instr.imm_se();

        let addr_reg = self.get_reg_read(instr.rs());

        let imm_val = self.bldr.ins().iconst(types::I32, v as i64);
        let addr = self.bldr.use_var(addr_reg);

        let address = self.bldr.ins().iadd(imm_val, addr);

        // let system = self
        //     .bldr
        //     .ins()
        //     .iconst(types::I64, self.constants.system as usize as i64);
        // let load_byte = self.get_load_byte();
        // let res = self.bldr.ins().call(load_byte, &[system, address]);
        // let v = self.bldr.inst_results(res)[0];
        let v = self.compile_load(types::I8, address);
        let v = self.bldr.ins().uextend(types::I32, v);
        // let v = self.bldr.ins().uextend(types::I32, v);

        // do we sign extend it??? (yes)

        self.compile_delayed_load_chain(instr.rt(), v);
    }

    fn compile_break(&mut self, instr: Instruction) {
        self.compile_delayed_load(true);
        self.compile_exception(Exception::Break, None);
    }

    fn compile_syscall(&mut self, instr: Instruction) {
        self.compile_delayed_load(true);
        self.compile_exception(Exception::SysCall, None);
    }

    fn compile_cop0(&mut self, instr: Instruction) {
        match instr.cop_opcode() {
            0b00000 => self.compile_mfc0(instr),
            0b00100 => self.compile_mtc0(instr),
            0b10000 => self.compile_rfe(instr),
            _ => panic!(
                "Unhandled cop0 instruction:  {:02X} ({:b})",
                instr.cop_opcode(),
                instr.cop_opcode()
            ),
        }
    }

    fn compile_mfc0(&mut self, instr: Instruction) {
        let v = match instr.rd() {
            6 => self.bldr.ins().iconst(types::I32, 0), // jumpdest..
            7 => self.bldr.ins().iconst(types::I32, 0), // not used (0)
            8 => {
                let reg = self.get_reg_read(REG_BADDR);
                self.bldr.use_var(reg)
            } // bad virtual address (R),
            12 => {
                let reg = self.get_reg_read(REG_SR);
                self.bldr.use_var(reg)
            }
            13 => {
                let reg = self.get_reg_read(REG_CAUSE);
                self.bldr.use_var(reg)
            }
            14 => {
                let reg = self.get_reg_read(REG_EPC);
                self.bldr.use_var(reg)
            }
            15 => self.bldr.ins().iconst(types::I32, 0x00000002), // Processor ID
            x => panic!("unhandled read from the cop0r{} register", x),
        };
        // let var = self.bldr.declare_var(types::I32);
        // self.bldr.def_var(var, v);
        self.compile_delayed_load_chain(instr.rt(), v);
    }

    fn compile_mtc0(&mut self, instr: Instruction) {
        let v = self.get_reg_read(instr.rt());
        let value = self.bldr.use_var(v);
        self.compile_delayed_load(true);
        match instr.rd() {
            3 | 5 | 6 | 7 | 9 | 11 => {
                // breakpoint registers
                // FIXME: put the guard here....
                // if v != 0 {
                //     panic!("unhandled cop0 breakpoint register write");
                // }
            }
            8 => {
                let baddr = self.get_reg_write(REG_BADDR);
                self.bldr.def_var(baddr, value);
            }
            12 => {
                let sr = self.get_reg_write(REG_SR);
                self.bldr.def_var(sr, value);
            } //self.sr = v,
            13 => {
                // cause register
                //self.cause =  (self.cause & !0x300) | (v & 0x300);
                let cause_reg = self.get_reg_read(REG_CAUSE);
                let cause_v = self.bldr.use_var(cause_reg);
                let cause_v = self.bldr.ins().band_imm_u(cause_v, !0x300);
                let v = self.bldr.ins().band_imm_u(value, 0x300);
                let res = self.bldr.ins().bor(cause_v, v);
                self.bldr.def_var(cause_reg, res);
            }
            n => panic!("Unhandled cop0 register {:08X}", n),
        }
    }

    fn compile_cop2(&mut self, instr: Instruction) {
        let cop_opcode = instr.cop_opcode();

        if cop_opcode & 0x10 != 0 {
            let gte_command = self.get_gte_command();
            // GTE command
            self.compile_delayed_load(true);
            let i = self.bldr.ins().iconst(types::I32, instr.0 as i64);
            let gte = self
                .bldr
                .ins()
                .iconst(types::I64, self.constants.gte as usize as i64);
            let res = self.bldr.ins().call(gte_command, &[gte, i]);
            // self.gte.command(instr.0);
        } else {
            match cop_opcode {
                0b00000 => self.compile_mfc2(instr),
                0b00010 => self.compile_cfc2(instr),
                0b00100 => self.compile_mtc2(instr),
                0b00110 => self.compile_ctc2(instr),
                _ => panic!(
                    "Unhandled cop2 instruction:  {:02X} ({:b})",
                    instr.cop_opcode(),
                    instr.cop_opcode()
                ),
            }
        }
    }

    fn compile_mfc2(&mut self, instr: Instruction) {
        let cop_r = instr.rd();
        let gte_data = self.get_gte_data();
        let gte = self.get_gte();

        let cop_r = self.bldr.ins().iconst(types::I8, cop_r as i64);

        let res = self.bldr.ins().call(gte_data, &[gte, cop_r]);
        let res = self.bldr.inst_results(res)[0];
        self.compile_delayed_load_chain(instr.rt(), res);
    }

    fn compile_cfc2(&mut self, instr: Instruction) {
        let cop_r = instr.rd();
        let gte_data = self.get_gte_control();
        let gte = self.get_gte();

        let cop_r = self.bldr.ins().iconst(types::I8, cop_r as i64);

        let res = self.bldr.ins().call(gte_data, &[gte, cop_r]);
        let res = self.bldr.inst_results(res)[0];
        self.compile_delayed_load_chain(instr.rt(), res);
    }
    fn compile_mtc2(&mut self, instr: Instruction) {
        let cpu_r = self.get_reg_read(instr.rt());
        let cop_r = instr.rd();
        let set_data = self.get_gte_set_data();
        let gte = self.get_gte();

        let cpu_v = self.bldr.use_var(cpu_r);

        let cop_r = self.bldr.ins().iconst(types::I8, cop_r as i64);

        self.compile_delayed_load(true);

        self.bldr.ins().call(set_data, &[gte, cop_r, cpu_v]);
    }
    fn compile_ctc2(&mut self, instr: Instruction) {
        let cpu_r = self.get_reg_read(instr.rt());
        let cop_r = instr.rd();
        let set_control = self.get_gte_set_control();
        let gte = self.get_gte();

        let cpu_v = self.bldr.use_var(cpu_r);

        let cop_r = self.bldr.ins().iconst(types::I8, cop_r as i64);

        self.compile_delayed_load(true);

        self.bldr.ins().call(set_control, &[gte, cop_r, cpu_v]);
    }

    fn compile_lwc2(&mut self, instr: Instruction) {
        let cop_r = instr.rt();

        let addr_reg = self.get_reg_read(instr.rs());

        let imm_val = self.bldr.ins().iconst(types::I32, instr.imm_se() as i64);
        let addr = self.bldr.use_var(addr_reg);

        let address = self.bldr.ins().iadd(imm_val, addr);

        let addr = self.bldr.ins().band_imm_u(address, 0x3);

        self.compile_delayed_load(true);

        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();
        self.bldr.ins().brif(addr, then_block, &[], else_block, &[]);
        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        self.compile_exception(Exception::LoadAddressError, Some(address));

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);

        // let system = self
        //     .bldr
        //     .ins()
        //     .iconst(types::I64, self.constants.system as i64);
        // let load_word = self.get_load_word();
        // let res = self.bldr.ins().call(load_word, &[system, address]);
        // let res = self.bldr.inst_results(res)[0];
        let res = self.compile_load(types::I32, address);

        let gte = self.get_gte();
        let cop_r = self.bldr.ins().iconst(types::I8, cop_r as i64);
        let set_data = self.get_gte_set_data();
        self.bldr.ins().call(set_data, &[gte, cop_r, res]);
    }

    fn compile_swc2(&mut self, instr: Instruction) {
        let v = instr.imm_se();
        let addr_reg = instr.rs();
        let cop_reg = instr.rt();

        let addr_reg_var = self.get_reg_read(addr_reg);

        let imm_val = self.bldr.ins().iconst(types::I32, v as i64);
        let addr_val = self.bldr.use_var(addr_reg_var);

        let address = self.bldr.ins().iadd(imm_val, addr_val);

        self.compile_delayed_load(true);

        let addr = self.bldr.ins().band_imm_u(address, 0x3);

        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();
        self.bldr.ins().brif(addr, then_block, &[], else_block, &[]);
        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        self.compile_exception(Exception::StoreAddressError, Some(address));

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);

        let sr = self.get_reg_read(REG_SR);
        let sr = self.bldr.use_var(sr);
        let sr = self.bldr.ins().band_imm_u(sr, 0x10000);
        // if self.sr & 0x10000 != 0 {
        //     // println!("Ignoring store while cache is isolated");
        //     return;
        // }
        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();
        self.bldr.ins().brif(sr, then_block, &[], else_block, &[]);
        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);

        let gte_data = self.get_gte_data();
        let gte = self.get_gte();

        let cop_r = self.bldr.ins().iconst(types::I8, cop_reg as i64);

        let res = self.bldr.ins().call(gte_data, &[gte, cop_r]);
        let res = self.bldr.inst_results(res)[0];
        let system = self
            .bldr
            .ins()
            .iconst(types::I64, self.constants.system as usize as i64);
        let store_word = self.get_store_word();
        self.bldr.ins().call(store_word, &[system, address, res]);
        self.bldr.ins().jump(then_block, &[]);

        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
    }

    fn compile_rfe(&mut self, instr: Instruction) {
        if instr.0 & 0x3f != 0b010000 {
            panic!("Invalid cop0 instruction {:x}", instr.0);
        }
        self.compile_delayed_load(true);

        let sr_var = self.get_reg_read(REG_SR);
        let sr = self.bldr.use_var(sr_var);
        let mode = self.bldr.ins().band_imm_u(sr, 0x3f);
        let sr = self.bldr.ins().band_imm_u(sr, !0xf);
        let mode = self.bldr.ins().sshr_imm_u(mode, 2);
        let sr = self.bldr.ins().bor(sr, mode);
        // let mode = self.sr & 0x3f;
        // self.sr &= !0xf;
        // self.sr |= mode >> 2;
        self.bldr.def_var(sr_var, sr);
    }

    pub fn get_store_word(&mut self) -> FuncRef {
        match self.store_word {
            Some(f) => f,
            None => {
                let store_word = self
                    .module
                    .declare_func_in_func(self.constants.store_word, &mut self.bldr.func);
                self.store_word = Some(store_word);
                store_word
            }
        }
    }

    pub fn get_store_half_word(&mut self) -> FuncRef {
        match self.store_half_word {
            Some(f) => f,
            None => {
                let f = self
                    .module
                    .declare_func_in_func(self.constants.store_half_word, &mut self.bldr.func);
                self.store_half_word = Some(f);
                f
            }
        }
    }

    pub fn get_store_byte(&mut self) -> FuncRef {
        match self.store_byte {
            Some(f) => f,
            None => {
                let f = self
                    .module
                    .declare_func_in_func(self.constants.store_byte, &mut self.bldr.func);
                self.store_byte = Some(f);
                f
            }
        }
    }

    pub fn get_load_word(&mut self) -> FuncRef {
        match self.load_word {
            Some(f) => f,
            None => {
                let f = self
                    .module
                    .declare_func_in_func(self.constants.load_word, &mut self.bldr.func);
                self.load_word = Some(f);
                f
            }
        }
    }

    pub fn get_load_half_word(&mut self) -> FuncRef {
        match self.load_half_word {
            Some(f) => f,
            None => {
                let f = self
                    .module
                    .declare_func_in_func(self.constants.load_half_word, &mut self.bldr.func);
                self.load_half_word = Some(f);
                f
            }
        }
    }
    pub fn get_load_byte(&mut self) -> FuncRef {
        match self.load_byte {
            Some(f) => f,
            None => {
                let f = self
                    .module
                    .declare_func_in_func(self.constants.load_byte, &mut self.bldr.func);
                self.load_byte = Some(f);
                f
            }
        }
    }

    pub fn get_print_(&mut self) -> FuncRef {
        match self.print_ {
            Some(f) => f,
            None => {
                let f = self
                    .module
                    .declare_func_in_func(self.constants.print_, &mut self.bldr.func);
                self.print_ = Some(f);
                f
            }
        }
    }

    pub fn get_gte(&mut self) -> Value {
        self.bldr
            .ins()
            .iconst(types::I64, self.constants.gte as usize as i64)
    }

    pub fn get_gte_command(&mut self) -> FuncRef {
        match self.gte_command {
            Some(f) => f,
            None => {
                let f = self
                    .module
                    .declare_func_in_func(self.constants.gte_command, &mut self.bldr.func);
                self.gte_command = Some(f);
                f
            }
        }
    }

    pub fn get_gte_data(&mut self) -> FuncRef {
        match self.gte_data {
            Some(f) => f,
            None => {
                let f = self
                    .module
                    .declare_func_in_func(self.constants.gte_data, &mut self.bldr.func);
                self.gte_data = Some(f);
                f
            }
        }
    }
    pub fn get_gte_control(&mut self) -> FuncRef {
        match self.gte_control {
            Some(f) => f,
            None => {
                let f = self
                    .module
                    .declare_func_in_func(self.constants.gte_control, &mut self.bldr.func);
                self.gte_control = Some(f);
                f
            }
        }
    }
    pub fn get_gte_set_data(&mut self) -> FuncRef {
        match self.gte_set_data {
            Some(f) => f,
            None => {
                let f = self
                    .module
                    .declare_func_in_func(self.constants.gte_set_data, &mut self.bldr.func);
                self.gte_set_data = Some(f);
                f
            }
        }
    }
    pub fn get_gte_set_control(&mut self) -> FuncRef {
        match self.gte_set_control {
            Some(f) => f,
            None => {
                let f = self
                    .module
                    .declare_func_in_func(self.constants.gte_set_control, &mut self.bldr.func);
                self.gte_set_control = Some(f);
                f
            }
        }
    }
    // let print_ = self
    //     .module
    //     .declare_func_in_func(self.constants.print_, &mut builder.func);
}

/*
    pub fn decode_and_execute(&mut self, instr: Instruction) {
        match instr.opcode() {
            0x00 => match instr.secondary_opcode() {
                _    => compile_illegal(instr), // TODO: bltz/bgez undocumented dupes
            },
            0x11 => compile_cop1(instr),
            0x13 => compile_cop3(instr),
            0x30 => compile_lwc0(instr),
            0x31 => compile_lwc1(instr),
            0x33 => compile_lwc3(instr),
            0x38 => compile_swc0(instr),
            0x39 => compile_swc1(instr),
            0x3B => compile_swc3(instr),
            // _ => panic!("Unhandled instruction: {:08X}, opcode: {:02X}", instr, instr.opcode())
            _ => compile_illegal(instr),
        }
    }
*/

pub enum Exception {
    ExternalInterrupt,
    LoadAddressError,  // baddr
    StoreAddressError, // baddr
    BusErrorOnFetch,
    SysCall,
    Break,
    IllegalInstruction,
    CoprocessorError,
    Overflow,
}

impl Exception {
    pub fn code(&self) -> u32 {
        match self {
            Exception::ExternalInterrupt => 0x0,
            Exception::LoadAddressError => 0x4,
            Exception::StoreAddressError => 0x5,
            Exception::BusErrorOnFetch => 0x6,
            Exception::SysCall => 0x8,
            Exception::Break => 0x9,
            Exception::IllegalInstruction => 0xa,
            Exception::CoprocessorError => 0xb,
            Exception::Overflow => 0xc,
        }
    }
}

pub extern "C" fn print_(ch: u8) {
    print!("{}", ch as char);
}

pub extern "C" fn gte_command(gte_ptr: *mut Gte, command: u32) {
    let gte = unsafe { gte_ptr.as_mut().expect("ok") };
    gte.command(command);
}

pub extern "C" fn gte_data(gte_ptr: *mut Gte, reg: u8) -> u32 {
    let gte = unsafe { gte_ptr.as_mut().expect("ok") };
    gte.data(reg)
}

pub extern "C" fn gte_control(gte_ptr: *mut Gte, reg: u8) -> u32 {
    let gte = unsafe { gte_ptr.as_mut().expect("ok") };
    gte.control(reg)
}

pub extern "C" fn gte_set_data(gte_ptr: *mut Gte, reg: u8, val: u32) {
    let gte = unsafe { gte_ptr.as_mut().expect("ok") };
    gte.set_data(reg, val)
}

pub extern "C" fn gte_set_control(gte_ptr: *mut Gte, reg: u8, val: u32) {
    let gte = unsafe { gte_ptr.as_mut().expect("ok") };
    gte.set_control(reg, val)
}
