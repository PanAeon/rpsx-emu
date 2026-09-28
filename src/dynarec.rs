use std::collections::HashMap;
use std::i8;

use crate::system::{load_byte, load_half_word, load_word, store_byte, store_half_word, store_word};
use crate::{block_cache::Block, cpu::Instruction, gte::Gte, system::System};

use cranelift::codegen::{ir::BlockArg, isa::CallConv};
use cranelift::prelude::*;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{DataDescription, FuncId, Linkage, Module};
// use cranelift::codegen::
// use cranelift_codegen::ir::types::*;
use cranelift::codegen::ir::{FuncRef, MachMemFlags};
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
    regs: [u32; 32],
    hi: u32,
    lo: u32,
    pub pc: u32, // reset value 0xBFC00000
    // pub next_pc: u32,
    // pub current_pc: u32,
    sr: u32,
    cause: u32,
    epc: u32,
    baddr: u32,
    load: (u32, u32), // load initiated by the current instruction
                      //branch: bool, // set by the current instruction if the branch occurred
                      //delay_slot:bool, // set if the current instruction executes in the delay slot
}
pub struct Constants {
    regs: *mut u8,
    system: *mut System,
    store_word: FuncId,
    load_word: FuncId,
    store_half_word: FuncId,
    store_byte: FuncId,
    load_half_word: FuncId,
    load_byte: FuncId,
    print_: FuncId,
}

pub struct Dynarec {
    pub system: System,
    pub regs: Box<Registers>,
    pub gte: Gte,
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

        let mut regs = Box::new(regs);
        let regs_ptr = unsafe { std::mem::transmute(regs.as_mut()) };
        let system_ptr = unsafe { std::mem::transmute(&system) };
        Dynarec {
            system,
            regs,
            gte: Gte::new(),
            builder_context: FunctionBuilderContext::new(),
            ctx: module.make_context(),
            data_description: DataDescription::new(),
            module,
            constants: Constants {
                regs: regs_ptr,
                system: system_ptr,
                store_word,
                store_half_word,
                store_byte,
                load_word,
                load_half_word,
                load_byte,
                print_
            },
        }
    }

    pub fn pc(&self) -> u32 {
        self.regs.pc
    }

    pub fn run_block(&mut self, idx: u16) -> u64 {
        // self.debug_print();
        let block = self.system.block_cache.get_block(idx);
        let jit_fn: extern "C" fn() -> i32 = unsafe { std::mem::transmute(block.ptr) };
        jit_fn() as u64
    }

    pub fn debug_print(&self) {
        println!("CPU pc: 0x{:X}", self.regs.pc);
        for (i, x) in self.regs.regs.iter().enumerate() {
            println!("regs[{:02}]: 0x{:X}", i, x);
        }
        println!();
    }

    pub fn compile_block(&mut self, addr: u32) -> Block {
        let mut buff = [Instruction(0); 256];
        // I guess first parse the block
        let xs = self.parse_block(addr, &mut buff);

        self.ctx.func.signature.returns.push(AbiParam::new(types::I32));

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

        let store_word = self
            .module
            .declare_func_in_func(self.constants.store_word, &mut builder.func);

        let store_half_word = self
            .module
            .declare_func_in_func(self.constants.store_half_word, &mut builder.func);

        let store_byte = self
            .module
            .declare_func_in_func(self.constants.store_byte, &mut builder.func);

        let load_word = self
            .module
            .declare_func_in_func(self.constants.load_word, &mut builder.func);

        let load_half_word = self
            .module
            .declare_func_in_func(self.constants.load_half_word, &mut builder.func);

        let load_byte = self
            .module
            .declare_func_in_func(self.constants.load_byte, &mut builder.func);

        let print_ = self
            .module
            .declare_func_in_func(self.constants.print_, &mut builder.func);

        let mut bldr = BlockBuilder {
            bldr: builder,
            vars: HashMap::new(),
            module: &mut self.module,
            delayed_load: None,
            constants: &self.constants,
            store_word,
            store_half_word,
            store_byte,
            load_word,
            load_half_word,
            load_byte,
            print_,
            i: 0,
            addr,
            next_instr: None,
            delay_slot: false,
        };

        // TODO: don't put load on the last instr in block

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

        if !last_jmp {
            println!("not last jump");
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
        verify_function(&self.ctx.func, &flags).expect("ok");
        // println!("{}", self.ctx.func.display());

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

        Block {
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
    module: &'a mut JITModule,
    delayed_load: Option<(u32, Value)>,
    constants: &'a Constants,
    store_word: FuncRef,
    load_word: FuncRef,
    i: usize,
    addr: u32,
    next_instr: Option<Instruction>,
    delay_slot: bool,
    store_half_word: FuncRef,
    store_byte: FuncRef,
    load_half_word: FuncRef,
    load_byte: FuncRef,
    print_: FuncRef,
}

impl<'a> BlockBuilder<'a> {
    fn compile_instr(&mut self, instr: Instruction) {
        self.inject_print_tty_output();
        match instr.opcode() {
            0x00 => match instr.secondary_opcode() {
                0x00 => self.compile_sll(instr),
                0x08 => self.compile_jr(instr),
                0x20 => self.compile_add(instr),
                0x21 => self.compile_addu(instr),
                0x24 => self.compile_and(instr),
                0x25 => self.compile_or(instr),
                0x26 => self.compile_xor(instr),
                0x27 => self.compile_nor(instr),
                0x2B => self.compile_sltu(instr),
                _ => panic!(
                    "Unhandled secondary opcode: {:02X}",
                    instr.secondary_opcode()
                ),
            },
            0x02 => self.compile_j(instr),
            0x04 => self.compile_beq(instr),
            0x03 => self.compile_jal(instr),
            0x05 => self.compile_bne(instr),
            0x08 => self.compile_addi(instr),
            0x09 => self.compile_addiu(instr),
            0x0C => self.compile_andi(instr),
            0x0D => self.compile_ori(instr),
            0x0F => self.compile_lui(instr),
            0x10 => self.compile_cop0(instr),
            0x20 => self.compile_lb(instr),
            0x23 => self.compile_lw(instr),
            0x28 => self.compile_sb(instr),
            0x29 => self.compile_sh(instr),
            0x2B => self.compile_sw(instr),
            _ => panic!(
                "Unhandled instruction: {:08X}, opcode: {:02X}",
                instr.0,
                instr.opcode()
            ),
        }
    }

    fn compile_instr_in_delay_slot(&mut self, instr: Instruction) {
        self.delay_slot = true;
        self.compile_instr(instr);
        self.delay_slot = false;
    }

    fn inject_print_tty_output(&mut self) {
        let pc = self.get_current_pc() & 0x1FFFFFFF;
        if pc == 0xA0 {
            let reg = self.get_reg_read(9);
            let reg = self.bldr.use_var(reg);
            let is_equal = self.bldr.ins().icmp_imm_u(IntCC::Equal, reg, 0x3C);

            let then_block = self.bldr.create_block();
            let else_block = self.bldr.create_block();

            self.bldr.ins().brif(is_equal, then_block, &[], else_block, &[]);

            self.bldr.switch_to_block(then_block);
            self.bldr.seal_block(then_block);
        
            let reg4 = self.get_reg_read(4);
            let reg4 = self.bldr.use_var(reg4);
            let reg4 = self.bldr.ins().ireduce(types::I8, reg4);
            self.bldr
                .ins()
                .call(self.print_, &[reg4]);

            self.bldr.ins().jump(else_block, &[]);
            self.bldr.switch_to_block(else_block);
            self.bldr.seal_block(else_block);
        } else if pc == 0xB0 {
            let reg = self.get_reg_read(9);
            let reg = self.bldr.use_var(reg);
            let is_equal = self.bldr.ins().icmp_imm_u(IntCC::Equal, reg, 0x3D);

            let then_block = self.bldr.create_block();
            let else_block = self.bldr.create_block();

            self.bldr.ins().brif(is_equal, then_block, &[], else_block, &[]);

            self.bldr.switch_to_block(then_block);
            self.bldr.seal_block(then_block);
        
            let reg4 = self.get_reg_read(4);
            let reg4 = self.bldr.use_var(reg4);
            let reg4 = self.bldr.ins().ireduce(types::I8, reg4);
            self.bldr
                .ins()
                .call(self.print_, &[reg4]);

            self.bldr.ins().jump(else_block, &[]);
            self.bldr.switch_to_block(else_block);
            self.bldr.seal_block(else_block);
        }
    //     if (pc == 0xA0 && self.regs[9] ==  0x3C) |  (pc == 0xB0 && self.regs[9] ==  0x3D) {
    //         let ch = self.regs[4] as u8 as char;
    //         print!("{ch}");
    //     }
    }

    fn save_context(&mut self) {
        let p = self
            .bldr
            .ins()
            .iconst(types::I64, self.constants.regs as usize as i64);
        for (reg, var) in self.vars.iter() {
            let x = self.bldr.use_var(*var);
            self.bldr
                .ins()
                .store(MachMemFlags::trusted(), x, p, *reg as i32 * 4);
        }
    }

    fn get_current_pc(&self) -> u32 {
        self.addr + (self.i as u32 * 4)
    }

    fn get_cycles(&self) -> i64 {
        ((self.i + 1) * 2) as i64
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

    fn compile_delayed_load(&mut self) {
        if let Some((reg, val)) = self.delayed_load {
            if reg != 0 {
                let variable = self
                    .vars
                    .entry(reg)
                    .or_insert_with(|| self.bldr.declare_var(types::I32));
                // let val = self.bldr.use_var(var);
                self.bldr.def_var(*variable, val);
            }
            self.delayed_load = None;
        }
    }

    fn compile_delayed_load_chain(&mut self, reg: u32, val: Value) {
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
        let cause = self.bldr.ins().bor_imm_u(cause, (reason.code() << 2) as i64);

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
        self.bldr.ins().brif(sr_bit, then_block, &[], else_block, &[]);

        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        let handler = self.bldr.ins().iconst(types::I32, 0xfbc00180);
        self.bldr.ins().jump(merge_block, &[BlockArg::Value(handler)]);

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);
        let handler = self.bldr.ins().iconst(types::I32, 0x80000080);
        self.bldr.ins().jump(merge_block, &[BlockArg::Value(handler)]);


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
        self.compile_instr_in_delay_slot(next_instr);
        self.compile_delayed_load();
        let next_pc = (self.addr & 0xf000_0000) | (instr.imm26() << 2);
        self.save_context();
        self.set_pc_imm(next_pc);
        let cycles = self.get_cycles();
        let cycles = self.bldr.ins().iconst(types::I32, cycles);
        self.bldr.ins().return_(&[cycles]);
        // self.next_pc = (self.current_pc & 0xf000_0000) | (instr.imm26() << 2);
        // self.branch = true;
        // self.delayed_load();
    }

    // jump register
    fn compile_jr(&mut self, instr: Instruction) {
        let next_instr = self.next_instr.unwrap();
        // FIXME: panic if next instruction is any kind of jump..
        self.compile_instr_in_delay_slot(next_instr);
        self.compile_delayed_load();

        let reg = self.get_reg_read(instr.rs());
        let val = self.bldr.use_var(reg);

        self.save_context();
        self.set_pc(val);
        let cycles = self.get_cycles();
        let cycles = self.bldr.ins().iconst(types::I32, cycles);
        self.bldr.ins().return_(&[cycles]);
        // self.next_pc = (self.current_pc & 0xf000_0000) | (instr.imm26() << 2);
        // self.branch = true;
        // self.delayed_load();
    }

    fn compile_jal(&mut self, instr: Instruction) {
        let next_instr = self.next_instr.unwrap();
        // FIXME: panic if next instruction is any kind of jump..
        self.compile_instr_in_delay_slot(next_instr);
        self.compile_delayed_load();
        let ra = self.get_current_pc().wrapping_add(4);
        let next_pc = (self.addr & 0xf000_0000) | (instr.imm26() << 2);
        let r31 = self.get_reg_write(31);
        let ra = self.bldr.ins().iconst(types::I32, ra as i64);
        self.bldr.def_var(r31, ra);
        self.save_context();
        self.set_pc_imm(next_pc);
        let cycles = self.get_cycles();
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
        let cycles = self.get_cycles();
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

        self.compile_delayed_load();

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


    // branch equal
    fn compile_beq(&mut self, instr: Instruction) {
        let rs_reg = self.get_reg_read(instr.rs());
        let rt_reg = self.get_reg_read(instr.rt());
        let rs_val = self.bldr.use_var(rs_reg);
        let rt_val = self.bldr.use_var(rt_reg);

        self.compile_delayed_load();

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
        self.compile_delayed_load();

        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();
        self.bldr.ins().brif(overflow, then_block, &[], else_block, &[]);
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
        self.compile_delayed_load();


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
        self.compile_delayed_load();

        let then_block = self.bldr.create_block();
        let else_block = self.bldr.create_block();
        self.bldr.ins().brif(overflow, then_block, &[], else_block, &[]);
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
        self.compile_delayed_load();
        if instr.rt() != 0 {
            let dest = self.get_reg_write(instr.rt());
            self.bldr.def_var(dest, res);
        }
        // let v = self.reg(instr.rs()).wrapping_add(i);
        // self.delayed_load();
        // self.set_reg(instr.rt(), v)
    }
    // set on less then unsigned
    fn compile_sltu(&mut self, instr: Instruction) {
        let reg_a = self.get_reg_read(instr.rs());
        let reg_b = self.get_reg_read(instr.rt());

        let a = self.bldr.use_var(reg_a);
        let b = self.bldr.use_var(reg_b);

        let res = self.bldr.ins().icmp(IntCC::UnsignedLessThan, a, b);
        let res = self.bldr.ins().uextend(types::I32, res);
        self.compile_delayed_load();
        if instr.rd() != 0 {
            let dest = self.get_reg_write(instr.rd());
            self.bldr.def_var(dest, res);
        }
    }

    // load upper immediate
    fn compile_lui(&mut self, instr: Instruction) {
        let v = instr.imm() << 16;
        self.compile_delayed_load();
        let reg = instr.rt();
        let ty = Type::int(32).unwrap();
        let variable = self.get_reg_read(reg);

        if reg != 0 {
            let val = self.bldr.ins().iconst(ty, v as i64);
            self.bldr.def_var(variable, val);
        }
    }

    fn compile_ori(&mut self, instr: Instruction) {
        // let ty = Type::int(32).unwrap();
        let v = instr.imm();
        let src_reg = instr.rs();
        let target_reg = instr.rt();

        let src_reg_var = self.get_reg_read(src_reg);
        let src_reg_val = self.bldr.use_var(src_reg_var);
        let res_val = self.bldr.ins().bor_imm_u(src_reg_val, v as i64);

        self.compile_delayed_load();

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

        self.compile_delayed_load();

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

        self.compile_delayed_load();

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

        self.compile_delayed_load();

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

        self.compile_delayed_load();

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

        self.compile_delayed_load();

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
        self.compile_delayed_load();
        if instr.rd() != 0 {
            let target_reg = self.get_reg_write(instr.rd());
            self.bldr.def_var(target_reg, result);
        }

        // let v = self.reg(instr.rt()) << i;
        // self.set_reg(instr.rd(), v);
    }

    fn get_reg_write(&mut self, reg: u32) -> Variable {
        let result = self
            .vars
            .entry(reg)
            .or_insert_with(|| self.bldr.declare_var(types::I32));
        *result
    }

    fn get_reg_read(&mut self, reg: u32) -> Variable {
        let res = (self.vars.entry(reg).or_insert_with(|| {
            let v = self.bldr.declare_var(types::I32);
            let p = self
                .bldr
                .ins()
                .iconst(types::I64, self.constants.regs as usize as i64);
            let val = self
                .bldr
                .ins()
                .load(types::I32, MachMemFlags::trusted(), p, reg as i32 * 4);
            self.bldr.def_var(v, val);
            v
        }));
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

        self.compile_delayed_load();

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
        self.bldr
            .ins()
            .call(self.store_word, &[system, address, val]);
        self.bldr.ins().jump(then_block, &[]);

        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
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

        self.compile_delayed_load();

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
        self.bldr
            .ins()
            .call(self.store_half_word, &[system, address, val]);
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

        self.compile_delayed_load();


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
        self.bldr
            .ins()
            .call(self.store_byte, &[system, address, val]);
        self.bldr.ins().jump(then_block, &[]);

        self.bldr.switch_to_block(then_block);
        self.bldr.seal_block(then_block);
        // do nothing....

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
        self.compile_delayed_load();
        self.compile_exception(Exception::StoreAddressError, Some(address));

        self.bldr.switch_to_block(else_block);
        self.bldr.seal_block(else_block);

        let system = self
            .bldr
            .ins()
            .iconst(types::I64, self.constants.system as usize as i64);
        let res = self.bldr
            .ins()
            .call(self.load_word, &[system, address]);
        let v = self.bldr.inst_results(res)[0];
        self.compile_delayed_load_chain(instr.rt(), v);
    }

    fn compile_lb(&mut self, instr: Instruction) {
        let v = instr.imm_se();

        let addr_reg = self.get_reg_read(instr.rs());


        let imm_val = self.bldr.ins().iconst(types::I32, v as i64);
        let addr = self.bldr.use_var(addr_reg);

        let address = self.bldr.ins().iadd(imm_val, addr);


        let system = self
            .bldr
            .ins()
            .iconst(types::I64, self.constants.system as usize as i64);
        let res = self.bldr
            .ins()
            .call(self.load_byte, &[system, address]);
        let v = self.bldr.inst_results(res)[0];
        let v = self.bldr.ins().sextend(types::I32, v);
        // let v = self.bldr.ins().uextend(types::I32, v);

        // do we sign extend it??? (yes)

        self.compile_delayed_load_chain(instr.rt(), v);
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
            6  => { self.bldr.ins().iconst(types::I32, 0)}, // jumpdest..
            7  => { self.bldr.ins().iconst(types::I32, 0)}, // not used (0)
            8  => {
                let reg = self.get_reg_read(REG_BADDR);
                self.bldr.use_var(reg)
            },// bad virtual address (R),
            12 => {
                let reg = self.get_reg_read(REG_SR);
                self.bldr.use_var(reg)
            },
            13 => {
                let reg = self.get_reg_read(REG_CAUSE);
                self.bldr.use_var(reg)
            },
            14 => {
                let reg = self.get_reg_read(REG_EPC);
                self.bldr.use_var(reg)
            },
            15 => {self.bldr.ins().iconst(types::I32, 0x00000002)} ,// Processor ID
            x => panic!("unhandled read from the cop0r{} register", x),
        };
        // let var = self.bldr.declare_var(types::I32);
        // self.bldr.def_var(var, v);
        self.compile_delayed_load_chain(instr.rt(), v);
    }

    fn compile_mtc0(&mut self, instr: Instruction) {
        let v = self.get_reg_read(instr.rt());
        self.compile_delayed_load();
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
                let value = self.bldr.use_var(v);
                self.bldr.def_var(baddr, value);
            },
            12 => {
                let sr = self.get_reg_write(REG_SR);
                let value = self.bldr.use_var(v);
                self.bldr.def_var(sr, value);
            },//self.sr = v,
            13 => {
                // cause register
                //self.cause =  (self.cause & !0x300) | (v & 0x300);
                let cause_reg = self.get_reg_read(REG_CAUSE);
                let cause_v = self.bldr.use_var(cause_reg);
                let cause_v = self.bldr.ins().band_imm_u(cause_v, !0x300);
                let v = self.bldr.use_var(v);
                let v = self.bldr.ins().band_imm_u(v, 0x300);
                let res = self.bldr.ins().bor(cause_v, v);
                self.bldr.def_var(cause_reg, res);
            }
            n => panic!("Unhandled cop0 register {:08X}", n),
        }
    }

    fn compile_rfe(&mut self, instr: Instruction) {
        if instr.0 & 0x3f != 0b010000 {
            panic!("Invalid cop0 instruction {:x}", instr.0);
        }
        self.compile_delayed_load();


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
}

/*
    pub fn decode_and_execute(&mut self, instr: Instruction) {
        match instr.opcode() {
            0x00 => match instr.secondary_opcode() {
                0x02 => compile_srl(instr),
                0x03 => compile_sra(instr),
                0x04 => compile_sllv(instr),
                0x06 => compile_srlv(instr),
                0x07 => compile_srav(instr),
                0x09 => compile_jalr(instr),
                0x0C => compile_syscall(instr),
                0x0D => compile_break(instr),
                0x10 => compile_mfhi(instr),
                0x11 => compile_mthi(instr),
                0x12 => compile_mflo(instr),
                0x13 => compile_mtlo(instr),
                0x18 => compile_mult(instr),
                0x19 => compile_multu(instr),
                0x1A => compile_div(instr),
                0x1B => compile_divu(instr),
                0x22 => compile_sub(instr),
                0x23 => compile_subu(instr),
                0x2A => compile_slt(instr),
                0x2B => compile_sltu(instr),
                _    => compile_illegal(instr), // TODO: bltz/bgez undocumented dupes
            },
            0x01 => compile_bxx(instr),
            0x06 => compile_blez(instr),
            0x07 => compile_bgtz(instr),
            0x0A => compile_slti(instr),
            0x0B => compile_sltiu(instr),
            0x0D => compile_ori(instr),
            0x0E => compile_xori(instr),
            0x11 => compile_cop1(instr),
            0x12 => compile_cop2(instr),
            0x13 => compile_cop3(instr),
            0x21 => compile_lh(instr),
            0x22 => compile_lwl(instr),
            0x24 => compile_lbu(instr),
            0x25 => compile_lhu(instr),
            0x26 => compile_lwr(instr),
            0x2a => compile_swl(instr),
            0x2b => compile_sw(instr),
            0x2e => compile_swr(instr),
            0x30 => compile_lwc0(instr),
            0x31 => compile_lwc1(instr),
            0x32 => compile_lwc2(instr),
            0x33 => compile_lwc3(instr),
            0x38 => compile_swc0(instr),
            0x39 => compile_swc1(instr),
            0x3A => compile_swc2(instr),
            0x3B => compile_swc3(instr),
            // _ => panic!("Unhandled instruction: {:08X}, opcode: {:02X}", instr, instr.opcode())
            _ => compile_illegal(instr),
        }
    }
*/


enum Exception {
    ExternalInterrupt,
    LoadAddressError, // baddr
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
