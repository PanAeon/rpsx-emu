use crate::cpu::Exception;
use crate::{
    cpu::Instruction,
    dynarec::Registers,
    gte::Gte,
    system::{Addressable, System},
};

pub struct Interpreter {
    pub next_pc: u32,
    pub current_pc: u32,

    branch: bool,     // set by the current instruction if the branch occurred
    delay_slot: bool, // set if the current instruction executes in the delay slot
    interrupt: bool,
}

impl Interpreter {
    pub fn new() -> Interpreter {
        Interpreter {
            next_pc: 0xBFC00004,
            current_pc: 0xBFC00000,
            branch: false,
            delay_slot: false,
            interrupt: false,
        }
    }
    pub fn run_block(&mut self, regs: &mut Registers, system: &mut System, gte: &mut Gte) -> u64 {
        // self.next_pc = regs.pc.wrapping_add(4);
        // self.branch = false;
        // self.delay_slot = false;
        self.interrupt = false;
        let mut num_instr = 0;
        while !self.branch && !self.interrupt {
            self.run_next_instruction(regs, system, gte);
            num_instr += 1;
        }
        self.run_next_instruction(regs, system, gte);
        num_instr += 1;
        num_instr
    }
    pub fn run_next_instruction(&mut self, regs: &mut Registers, system: &mut System, gte: &mut Gte) {
        if regs.pc & 3 != 0 {
            return self.exception(Exception::LoadAddressError(regs.pc), regs);
        }
        let instr = Instruction(self.load::<u32>(regs.pc, system));
        self.current_pc = regs.pc;
        regs.pc = self.next_pc;
        self.next_pc = self.next_pc.wrapping_add(4);
        // let (reg, val) = self.load;
        // self.set_reg(reg, val, regs);
        // self.load = (0, 0);
        self.delay_slot = self.branch;
        self.branch = false;

        let is_gte = (instr.0 & 0xFE00_0000) == 0x4A00_0000;
        let pending_interrupt = self.check_for_pending_interrupts(regs, system);

        if !pending_interrupt || (is_gte && !self.delay_slot) {
            self.decode_and_execute(instr, regs, system, gte);
        }

        if pending_interrupt {
            self.exception(Exception::ExternalInterrupt, regs);
        }
        // self.regs = self.out_regs;
    }

    pub fn check_for_pending_interrupts(
        &mut self,
        regs: &mut Registers,
        system: &mut System,
    ) -> bool {
        if system.irqctl.pending() {
            regs.cause |= 1 << 10;
        } else {
            regs.cause &= !(1 << 10);
        }
        // mask bits 8..15
        let pending = (regs.cause & regs.sr) & 0x700; //0xFF00;
        pending != 0 && (regs.sr & 1 != 0)
    }

    pub fn check_for_tty_output(&self, regs: &mut Registers) {
        let pc = regs.pc & 0x1FFFFFFF;
        if (pc == 0xA0 && regs.regs[9] == 0x3C) | (pc == 0xB0 && regs.regs[9] == 0x3D) {
            let ch = regs.regs[4] as u8 as char;
            print!("{ch}");
        }
    }

    pub fn load<T: Addressable>(&mut self, address: u32, system: &mut System) -> T {
        system.load(address)
    }

    pub fn store<T: Addressable>(
        &mut self,
        address: u32,
        value: T,
        regs: &mut Registers,
        system: &mut System,
    ) {
        if regs.sr & 0x10000 != 0 {
            // println!("Ignoring store while cache is isolated");
            return;
        }
        system.store(address, value)
    }

    pub fn set_reg(&mut self, index: u32, value: u32, regs: &mut Registers) {
        regs.regs[index as usize] = value;
        regs.regs[0] = 0;
    }
    pub fn reg(&mut self, index: u32, regs: &mut Registers) -> u32 {
        regs.regs[index as usize]
    }

    pub fn decode_and_execute(
        &mut self,
        instr: Instruction,
        regs: &mut Registers,
        system: &mut System,
        gte: &mut Gte,
    ) {
        match instr.opcode() {
            0x00 => match instr.secondary_opcode() {
                0x00 => self.op_sll(instr, regs),
                0x02 => self.op_srl(instr, regs),
                0x03 => self.op_sra(instr, regs),
                0x04 => self.op_sllv(instr, regs),
                0x06 => self.op_srlv(instr, regs),
                0x07 => self.op_srav(instr, regs),
                0x08 => self.op_jr(instr, regs),
                0x09 => self.op_jalr(instr, regs),
                0x0C => self.op_syscall(instr, regs),
                0x0D => self.op_break(instr, regs),
                0x10 => self.op_mfhi(instr, regs),
                0x11 => self.op_mthi(instr, regs),
                0x12 => self.op_mflo(instr, regs),
                0x13 => self.op_mtlo(instr, regs),
                0x18 => self.op_mult(instr, regs),
                0x19 => self.op_multu(instr, regs),
                0x1A => self.op_div(instr, regs),
                0x1B => self.op_divu(instr, regs),
                0x20 => self.op_add(instr, regs),
                0x21 => self.op_addu(instr, regs),
                0x22 => self.op_sub(instr, regs),
                0x23 => self.op_subu(instr, regs),
                0x24 => self.op_and(instr, regs),
                0x25 => self.op_or(instr, regs),
                0x26 => self.op_xor(instr, regs),
                0x27 => self.op_nor(instr, regs),
                0x2A => self.op_slt(instr, regs),
                0x2B => self.op_sltu(instr, regs),
                _ => self.op_illegal(instr, regs), // TODO: bltz/bgez undocumented dupes
            },
            0x01 => self.op_bxx(instr, regs),
            0x02 => self.op_j(instr, regs),
            0x03 => self.op_jal(instr, regs),
            0x04 => self.op_beq(instr, regs),
            0x05 => self.op_bne(instr, regs),
            0x06 => self.op_blez(instr, regs),
            0x07 => self.op_bgtz(instr, regs),
            0x08 => self.op_addi(instr, regs),
            0x09 => self.op_addiu(instr, regs),
            0x0A => self.op_slti(instr, regs),
            0x0B => self.op_sltiu(instr, regs),
            0x0C => self.op_andi(instr, regs),
            0x0D => self.op_ori(instr, regs),
            0x0E => self.op_xori(instr, regs),
            0x0F => self.op_lui(instr, regs),
            0x10 => self.op_cop0(instr, regs),
            0x11 => self.op_cop1(instr, regs),
            0x12 => self.op_cop2(instr, regs, gte),
            0x13 => self.op_cop3(instr, regs),
            0x20 => self.op_lb(instr, regs, system),
            0x21 => self.op_lh(instr, regs, system),
            0x22 => self.op_lwl(instr, regs, system),
            0x23 => self.op_lw(instr, regs, system),
            0x24 => self.op_lbu(instr, regs, system),
            0x25 => self.op_lhu(instr, regs, system),
            0x26 => self.op_lwr(instr, regs, system),
            0x28 => self.op_sb(instr, regs, system),
            0x29 => self.op_sh(instr, regs, system),
            0x2a => self.op_swl(instr, regs, system),
            0x2b => self.op_sw(instr, regs, system),
            0x2e => self.op_swr(instr, regs, system),
            0x30 => self.op_lwc0(instr, regs),
            0x31 => self.op_lwc1(instr, regs),
            0x32 => self.op_lwc2(instr, regs, system, gte),
            0x33 => self.op_lwc3(instr, regs),
            0x38 => self.op_swc0(instr, regs),
            0x39 => self.op_swc1(instr, regs),
            0x3A => self.op_swc2(instr, regs, system, gte),
            0x3B => self.op_swc3(instr, regs),
            // _ => panic!("Unhandled instruction: {:08X}, opcode: {:02X}", instr, instr.opcode())
            _ => self.op_illegal(instr, regs),
        }
    }

    pub fn op_illegal(&mut self, instr: Instruction, regs: &mut Registers) {
        self.delayed_load(regs);
        println!(
            "Illegal instruction: {:X}, opcode: {:02X}",
            instr.0,
            instr.opcode()
        );
        self.exception(Exception::IllegalInstruction, regs);
    }

    pub fn delayed_load(&mut self, regs: &mut Registers) {
        let (reg, val) = regs.load;
        self.set_reg(reg, val, regs);
        regs.load = (0, 0);
    }
    pub fn delayed_load_chain(&mut self, reg: u32, val: u32, regs: &mut Registers) {
        let (pending_reg, pending_val) = regs.load;
        if pending_reg != reg {
            self.set_reg(pending_reg, pending_val, regs);
        }
        regs.load = (reg, val);
    }

    pub fn op_lui(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = instr.imm() << 16;
        self.delayed_load(regs);
        self.set_reg(instr.rt(), v, regs);
    }
    pub fn op_ori(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = instr.imm() | self.reg(instr.rs(), regs);
        self.delayed_load(regs);
        self.set_reg(instr.rt(), v, regs);
    }
    pub fn op_andi(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = instr.imm() & self.reg(instr.rs(), regs);
        self.delayed_load(regs);
        self.set_reg(instr.rt(), v, regs);
    }
    pub fn op_or(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = self.reg(instr.rs(), regs) | self.reg(instr.rt(), regs);
        self.delayed_load(regs);
        self.set_reg(instr.rd(), v, regs);
    }
    pub fn op_xor(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = self.reg(instr.rs(), regs) ^ self.reg(instr.rt(), regs);
        self.delayed_load(regs);
        self.set_reg(instr.rd(), v, regs);
    }
    pub fn op_xori(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = self.reg(instr.rs(), regs) ^ instr.imm();
        self.delayed_load(regs);
        self.set_reg(instr.rt(), v, regs);
    }
    pub fn op_nor(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = !(self.reg(instr.rs(), regs) | self.reg(instr.rt(), regs));
        self.delayed_load(regs);
        self.set_reg(instr.rd(), v, regs);
    }
    pub fn op_and(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = self.reg(instr.rs(), regs) & self.reg(instr.rt(), regs);
        self.delayed_load(regs);
        self.set_reg(instr.rd(), v, regs);
    }
    pub fn op_sw(&mut self, instr: Instruction, regs: &mut Registers, system: &mut System) {
        let addr = self.reg(instr.rs(), regs).wrapping_add(instr.imm_se());
        let v = self.reg(instr.rt(), regs);
        if addr % 4 != 0 {
            self.delayed_load(regs);
            return self.exception(Exception::StoreAddressError(addr), regs);
        }
        self.delayed_load(regs);
        self.store::<u32>(addr, v, regs, system);
    }
    pub fn op_sh(&mut self, instr: Instruction, regs: &mut Registers, system: &mut System) {
        let addr = self.reg(instr.rs(), regs).wrapping_add(instr.imm_se());
        let v = self.reg(instr.rt(), regs);
        self.delayed_load(regs);
        if addr % 2 != 0 {
            return self.exception(Exception::StoreAddressError(addr), regs);
        }
        self.store::<u16>(addr, v as u16, regs, system);
    }
    pub fn op_sb(&mut self, instr: Instruction, regs: &mut Registers, system: &mut System) {
        let addr = self.reg(instr.rs(), regs).wrapping_add(instr.imm_se());
        let v = self.reg(instr.rt(), regs);
        self.delayed_load(regs);
        self.store::<u8>(addr, v as u8, regs, system);
    }
    pub fn op_lh(&mut self, instr: Instruction, regs: &mut Registers, system: &mut System) {
        let addr = self.reg(instr.rs(), regs).wrapping_add(instr.imm_se());
        if addr % 2 != 0 {
            self.delayed_load(regs);
            return self.exception(Exception::LoadAddressError(addr), regs);
        }
        let v = (self.load::<u16>(addr, system) as u16) as i16;
        self.delayed_load_chain(instr.rt(), v as u32, regs);
    }
    pub fn op_lhu(&mut self, instr: Instruction, regs: &mut Registers, system: &mut System) {
        let addr = self.reg(instr.rs(), regs).wrapping_add(instr.imm_se());
        if addr % 2 != 0 {
            self.delayed_load(regs);
            return self.exception(Exception::LoadAddressError(addr), regs);
        }
        let v = self.load::<u16>(addr, system);
        self.delayed_load_chain(instr.rt(), v as u32, regs);
    }
    pub fn op_lw(&mut self, instr: Instruction, regs: &mut Registers, system: &mut System) {
        let addr = self.reg(instr.rs(), regs).wrapping_add(instr.imm_se());
        if addr % 4 != 0 {
            self.delayed_load(regs);
            return self.exception(Exception::LoadAddressError(addr), regs);
        }
        let v = self.load::<u32>(addr, system);
        self.delayed_load_chain(instr.rt(), v, regs);
    }
    // load word left
    pub fn op_lwl(&mut self, instr: Instruction, regs: &mut Registers, system: &mut System) {
        let addr = self.reg(instr.rs(), regs).wrapping_add(instr.imm_se());
        let mut cur_v = regs.regs[instr.rt() as usize];
        let aligned_addr = addr & !3;
        let aligned_word = self.load::<u32>(aligned_addr, system);

        // This instruction bypasses the load delay restriction: this instruction will merge the new
        // contents with the value currently being loaded if need be.
        let (pending_reg, pending_value) = regs.load;
        if pending_reg == instr.rt() {
            cur_v = pending_value;
        }

        let v = match addr & 3 {
            0 => (cur_v & 0x00ffffff) | (aligned_word << 24),
            1 => (cur_v & 0x0000ffff) | (aligned_word << 16),
            2 => (cur_v & 0x000000ff) | (aligned_word << 8),
            3 => (cur_v & 0x00000000) | (aligned_word),
            _ => unreachable!(),
        };
        self.delayed_load_chain(instr.rt(), v, regs);
    }
    pub fn op_swl(&mut self, instr: Instruction, regs: &mut Registers, system: &mut System) {
        let addr = self.reg(instr.rs(), regs).wrapping_add(instr.imm_se());
        let v = self.reg(instr.rt(), regs);
        let aligned_addr = addr & !3;
        let cur_mem = self.load::<u32>(aligned_addr, system);

        let mem = match addr & 3 {
            0 => (cur_mem & 0xffffff00) | (v >> 24),
            1 => (cur_mem & 0xffff0000) | (v >> 16),
            2 => (cur_mem & 0xff000000) | (v >> 8),
            3 => (cur_mem & 0x00000000) | (v),
            _ => unreachable!(),
        };
        self.delayed_load(regs);
        self.store::<u32>(aligned_addr, mem, regs, system);
    }
    pub fn op_swr(&mut self, instr: Instruction, regs: &mut Registers, system: &mut System) {
        let addr = self.reg(instr.rs(), regs).wrapping_add(instr.imm_se());
        // if self.sr & 0x10000 != 0 {
        //     println!("Cache is isolated, ignoring read to {:08x}", addr);
        //     return;
        // }
        let v = self.reg(instr.rt(), regs);
        let aligned_addr = addr & !3;
        let cur_mem = self.load::<u32>(aligned_addr, system);

        let mem = match addr & 3 {
            0 => (cur_mem & 0x00000000) | (v),
            1 => (cur_mem & 0x000000ff) | (v << 8),
            2 => (cur_mem & 0x0000ffff) | (v << 16),
            3 => (cur_mem & 0x00ffffff) | (v << 24),
            _ => unreachable!(),
        };
        self.delayed_load(regs);
        self.store::<u32>(aligned_addr, mem, regs, system);
    }
    pub fn op_lwr(&mut self, instr: Instruction, regs: &mut Registers, system: &mut System) {
        let addr = self.reg(instr.rs(), regs).wrapping_add(instr.imm_se());
        let mut cur_v = regs.regs[instr.rt() as usize];
        let aligned_addr = addr & !3;
        let aligned_word = self.load::<u32>(aligned_addr, system);

        let (pending_reg, pending_value) = regs.load;
        if pending_reg == instr.rt() {
            cur_v = pending_value;
        }

        let v = match addr & 3 {
            0 => (cur_v & 0x00000000) | (aligned_word),
            1 => (cur_v & 0xff000000) | (aligned_word >> 8),
            2 => (cur_v & 0xffff0000) | (aligned_word >> 16),
            3 => (cur_v & 0xffffff00) | (aligned_word >> 24),
            _ => unreachable!(),
        };
        self.delayed_load_chain(instr.rt(), v, regs);
    }
    pub fn op_lb(&mut self, instr: Instruction, regs: &mut Registers, system: &mut System) {
        let addr = self.reg(instr.rs(), regs).wrapping_add(instr.imm_se());
        let v = (self.load::<u8>(addr, system) as u8) as i8;
        // self.load = (instr.rt(), v as u32);
        // self.delayed_load(regs);
        self.delayed_load_chain(instr.rt(), v as u32, regs);
    }
    pub fn op_lbu(&mut self, instr: Instruction, regs: &mut Registers, system: &mut System) {
        let addr = self.reg(instr.rs(), regs).wrapping_add(instr.imm_se());
        let v = self.load::<u8>(addr, system);
        self.delayed_load_chain(instr.rt(), v as u32, regs);
    }
    pub fn op_sll(&mut self, instr: Instruction, regs: &mut Registers) {
        let i = instr.imm5();
        let v = self.reg(instr.rt(), regs) << i;
        self.delayed_load(regs);
        self.set_reg(instr.rd(), v, regs);
    }
    pub fn op_sllv(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = self.reg(instr.rt(), regs) << (self.reg(instr.rs(), regs) & 0x1f);
        self.delayed_load(regs);
        self.set_reg(instr.rd(), v, regs);
    }
    pub fn op_sra(&mut self, instr: Instruction, regs: &mut Registers) {
        let i = instr.imm5();
        let v = (self.reg(instr.rt(), regs) as i32) >> i;
        self.delayed_load(regs);
        self.set_reg(instr.rd(), v as u32, regs);
    }
    // shift right arithmetic variable
    pub fn op_srav(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = (self.reg(instr.rt(), regs) as i32) >> (self.reg(instr.rs(), regs) & 0x1f);
        self.delayed_load(regs);
        self.set_reg(instr.rd(), v as u32, regs);
    }
    pub fn op_srlv(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = (self.reg(instr.rt(), regs)) >> (self.reg(instr.rs(), regs) & 0x1f);
        self.delayed_load(regs);
        self.set_reg(instr.rd(), v, regs);
    }
    pub fn op_srl(&mut self, instr: Instruction, regs: &mut Registers) {
        let i = instr.imm5();
        let v = (self.reg(instr.rt(), regs)) >> i;
        self.delayed_load(regs);
        self.set_reg(instr.rd(), v, regs);
    }
    pub fn op_add(&mut self, instr: Instruction, regs: &mut Registers) {
        let s = self.reg(instr.rs(), regs) as i32;
        let t = self.reg(instr.rt(), regs) as i32;
        let v = match s.checked_add(t) {
            Some(v) => v as u32,
            None => return self.exception(Exception::Overflow, regs),
        };
        self.delayed_load(regs);
        self.set_reg(instr.rd(), v, regs)
    }
    pub fn op_addi(&mut self, instr: Instruction, regs: &mut Registers) {
        let i = instr.imm_se() as i32;
        let s = self.reg(instr.rs(), regs) as i32;
        let v = match s.checked_add(i) {
            Some(v) => v as u32,
            None => return self.exception(Exception::Overflow, regs),
        };
        self.delayed_load(regs);
        self.set_reg(instr.rt(), v, regs)
    }
    pub fn op_addu(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = self.reg(instr.rs(), regs).wrapping_add(self.reg(instr.rt(), regs));
        self.delayed_load(regs);
        self.set_reg(instr.rd(), v, regs)
    }
    pub fn op_sub(&mut self, instr: Instruction, regs: &mut Registers) {
        let s = self.reg(instr.rs(), regs) as i32;
        let t = self.reg(instr.rt(), regs) as i32;
        let d = instr.rd();
        self.delayed_load(regs);
        match s.checked_sub(t) {
            Some(v) => self.set_reg(d, v as u32, regs),
            None => self.exception(Exception::Overflow, regs),
        }
    }
    pub fn op_subu(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = self.reg(instr.rs(), regs).wrapping_sub(self.reg(instr.rt(), regs));
        self.delayed_load(regs);
        self.set_reg(instr.rd(), v, regs)
    }
    pub fn op_addiu(&mut self, instr: Instruction, regs: &mut Registers) {
        let i = instr.imm_se();
        let v = self.reg(instr.rs(), regs).wrapping_add(i);
        self.delayed_load(regs);
        self.set_reg(instr.rt(), v, regs)
    }
    // TODO: stalls if division is not yet complete
    pub fn op_mflo(&mut self, instr: Instruction, regs: &mut Registers) {
        self.delayed_load(regs);
        self.set_reg(instr.rd(), regs.lo, regs);
    }
    pub fn op_mtlo(&mut self, instr: Instruction, regs: &mut Registers) {
        self.delayed_load(regs);
        regs.lo = self.reg(instr.rs(), regs);
    }
    pub fn op_mthi(&mut self, instr: Instruction, regs: &mut Registers) {
        self.delayed_load(regs);
        regs.hi = self.reg(instr.rs(), regs);
    }
    pub fn op_mfhi(&mut self, instr: Instruction, regs: &mut Registers) {
        self.delayed_load(regs);
        self.set_reg(instr.rd(), regs.hi, regs);
    }
    pub fn op_mult(&mut self, instr: Instruction, regs: &mut Registers) {
        let a = (self.reg(instr.rs(), regs) as i32) as i64;
        let b = (self.reg(instr.rt(), regs) as i32) as i64;

        let v = (a * b) as u64;
        self.delayed_load(regs);

        regs.hi = (v >> 32) as u32;
        regs.lo = v as u32;
    }
    pub fn op_multu(&mut self, instr: Instruction, regs: &mut Registers) {
        let a = self.reg(instr.rs(), regs) as u64;
        let b = self.reg(instr.rt(), regs) as u64;

        let v = a * b;

        self.delayed_load(regs);
        regs.hi = (v >> 32) as u32;
        regs.lo = v as u32;
    }
    pub fn op_div(&mut self, instr: Instruction, regs: &mut Registers) {
        let s = instr.rs();
        let t = instr.rt();

        let n = self.reg(s, regs) as i32;
        let d = self.reg(t, regs) as i32;
        self.delayed_load(regs);

        if d == 0 {
            regs.hi = n as u32;
            if n >= 0 {
                regs.lo = 0xffffffff;
            } else {
                regs.lo = 1;
            }
        } else if n as u32 == 0x80000000 && d == -1 {
            regs.hi = 0;
            regs.lo = 0x80000000;
        } else {
            regs.hi = (n % d) as u32;
            regs.lo = (n / d) as u32;
        }
    }
    pub fn op_divu(&mut self, instr: Instruction, regs: &mut Registers) {
        let s = instr.rs();
        let t = instr.rt();

        let n = self.reg(s, regs);
        let d = self.reg(t, regs);
        self.delayed_load(regs);

        if d == 0 {
            regs.hi = n as u32;
            regs.lo = 0xffffffff;
        } else {
            regs.hi = (n % d) as u32;
            regs.lo = (n / d) as u32;
        }
    }
    pub fn op_j(&mut self, instr: Instruction, regs: &mut Registers) {
        self.next_pc = (self.current_pc & 0xf000_0000) | (instr.imm26() << 2);
        self.branch = true;
        self.delayed_load(regs);
    }
    pub fn op_jal(&mut self, instr: Instruction, regs: &mut Registers) {
        let ra = self.next_pc;
        self.next_pc = (regs.pc & 0xf0000000) | (instr.imm26() << 2);
        self.delayed_load(regs);
        self.branch = true;
        self.set_reg(31, ra, regs);
    }
    pub fn op_jr(&mut self, instr: Instruction, regs: &mut Registers) {
        self.next_pc = self.reg(instr.rs(), regs);
        self.delayed_load(regs);
        self.branch = true;
    }

    pub fn op_jalr(&mut self, instr: Instruction, regs: &mut Registers) {
        let ra = self.next_pc;
        self.next_pc = self.reg(instr.rs(), regs);
        self.delayed_load(regs);
        self.branch = true;
        self.set_reg(instr.rd(), ra, regs);
    }

    pub fn branch(&mut self, offset: u32, regs: &mut Registers) {
        let offset = offset << 2;
        self.next_pc = regs.pc.wrapping_add(offset);
        self.delayed_load(regs);
        self.branch = true;
    }
    pub fn op_bne(&mut self, instr: Instruction, regs: &mut Registers) {
        if self.reg(instr.rs(), regs) != self.reg(instr.rt(), regs) {
            self.branch(instr.imm_se(), regs);
        }
    }
    pub fn op_bgtz(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = self.reg(instr.rs(), regs) as i32;
        if v > 0 {
            self.branch(instr.imm_se(), regs);
        }
    }
    pub fn op_blez(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = self.reg(instr.rs(), regs) as i32;
        if v <= 0 {
            self.branch(instr.imm_se(), regs);
        }
    }
    pub fn op_beq(&mut self, instr: Instruction, regs: &mut Registers) {
        if self.reg(instr.rs(), regs) == self.reg(instr.rt(), regs) {
            self.branch(instr.imm_se(), regs);
        }
    }
    // BGEZ, BLTZ, BGEZAL, BLTZAL
    pub fn op_bxx(&mut self, instr: Instruction, regs: &mut Registers) {
        let i = instr.imm_se();
        let s = instr.rs();

        let instruction = instr.0;

        let is_bgez = (instruction >> 16) & 1;
        let is_link = (instruction >> 17) & 0xf == 8;

        let v = self.reg(s, regs) as i32;

        let test = (v < 0) as u32;
        let test = test ^ is_bgez;

        self.delayed_load(regs);

        if is_link {
            let ra = regs.pc.wrapping_add(4); // self.pc??
            self.set_reg(31, ra, regs);
        }
        if test != 0 {
            self.branch(i, regs);
        }
    }
    pub fn op_slt(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = (self.reg(instr.rs(), regs) as i32) < (self.reg(instr.rt(), regs) as i32);
        self.delayed_load(regs);
        self.set_reg(instr.rd(), v as u32, regs);
    }
    pub fn op_sltu(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = self.reg(instr.rs(), regs) < self.reg(instr.rt(), regs);
        self.delayed_load(regs);
        self.set_reg(instr.rd(), v as u32, regs);
    }
    pub fn op_slti(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = (self.reg(instr.rs(), regs) as i32) < (instr.imm_se() as i32);
        self.delayed_load(regs);
        self.set_reg(instr.rt(), v as u32, regs);
    }
    pub fn op_sltiu(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = (self.reg(instr.rs(), regs)) < (instr.imm_se());
        self.delayed_load(regs);
        self.set_reg(instr.rt(), v as u32, regs);
    }

    fn exception(&mut self, cause: Exception, regs: &mut Registers) {
        // TODO:: branch delay slot
        // TODO: bad address exception...
        let mode = regs.sr & 0x3f;
        regs.sr &= !0x3f;
        regs.sr |= (mode << 2) & 0x3f;

        regs.cause &= !0x7c;
        regs.cause |= (cause.code() as u32) << 2; // woot?

        if self.delay_slot {
            // this what happend?
            regs.epc = self.current_pc.wrapping_sub(4);
            regs.cause |= 1 << 31;
        } else {
            regs.epc = self.current_pc;
            regs.cause &= !(1 << 31);
        }

        // exception handler address depends on the BEV bit
        let handler: u32 = if regs.sr & (1 << 22) != 0 {
            0xbfc00180
        } else {
            0x80000080
        };

        if let Exception::LoadAddressError(x) | Exception::StoreAddressError(x) = cause {
            regs.baddr = x;
        }

        self.interrupt = true;
        regs.pc = handler;
        self.next_pc = handler.wrapping_add(4);
    }

    pub fn op_syscall(&mut self, _: Instruction, regs: &mut Registers) {
        self.delayed_load(regs);
        self.exception(Exception::SysCall, regs);
    }
    pub fn op_break(&mut self, _: Instruction, regs: &mut Registers) {
        self.delayed_load(regs);
        self.exception(Exception::Break, regs);
    }

    pub fn op_lwc0(&mut self, _: Instruction, regs: &mut Registers) {
        self.delayed_load(regs);
        self.exception(Exception::CoprocessorError, regs);
    }
    pub fn op_lwc1(&mut self, _: Instruction, regs: &mut Registers) {
        self.delayed_load(regs);
        self.exception(Exception::CoprocessorError, regs);
    }
    // load word to coprocessor 2
    pub fn op_lwc2(&mut self, instr: Instruction, regs: &mut Registers, system: &mut System, gte: &mut Gte) {
        let i = instr.imm_se();
        let cop_r = instr.rt() as u8;
        let s = instr.rs();

        let addr = self.reg(s, regs).wrapping_add(i);

        self.delayed_load(regs);

        if addr.is_multiple_of(4) {
            let v = self.load::<u32>(addr, system);
            gte.set_data(cop_r, v);
        } else {
            self.exception(Exception::LoadAddressError(addr), regs);
        }
    }
    pub fn op_lwc3(&mut self, _: Instruction, regs: &mut Registers) {
        self.delayed_load(regs);
        self.exception(Exception::CoprocessorError, regs);
    }

    pub fn op_swc0(&mut self, _: Instruction, regs: &mut Registers) {
        self.delayed_load(regs);
        self.exception(Exception::CoprocessorError, regs);
    }
    pub fn op_swc1(&mut self, _: Instruction, regs: &mut Registers) {
        self.delayed_load(regs);
        self.exception(Exception::CoprocessorError, regs);
    }
    // store word from coprocessor 2
    pub fn op_swc2(&mut self, instr: Instruction, regs: &mut Registers, system: &mut System, gte: &mut Gte) {
        let i = instr.imm_se();
        let cop_r = instr.rt() as u8;
        let s = instr.rs();

        let addr = self.reg(s, regs).wrapping_add(i);
        let v = gte.data(cop_r);
        self.delayed_load(regs);

        if addr.is_multiple_of(4) {
            self.store::<u32>(addr, v, regs, system);
        } else {
            self.exception(Exception::LoadAddressError(addr), regs);
        }
    }
    pub fn op_swc3(&mut self, _: Instruction, regs: &mut Registers) {
        self.delayed_load(regs);
        self.exception(Exception::CoprocessorError, regs);
    }

    pub fn op_cop0(&mut self, instr: Instruction, regs: &mut Registers) {
        match instr.cop_opcode() {
            0b00000 => self.op_mfc0(instr, regs),
            0b00100 => self.op_mtc0(instr, regs),
            0b10000 => self.op_rfe(instr, regs),
            _ => panic!(
                "Unhandled cop0 instruction:  {:02X} ({:b})",
                instr.cop_opcode(),
                instr.cop_opcode()
            ),
        }
    }
    pub fn op_cop1(&mut self, _: Instruction, regs: &mut Registers) {
        self.delayed_load(regs);
        self.exception(Exception::CoprocessorError, regs);
    }
    pub fn op_cop2(&mut self, instr: Instruction, regs: &mut Registers, gte: &mut Gte) {
        let cop_opcode = instr.cop_opcode();

        if cop_opcode & 0x10 != 0 {
            // GTE command
            self.delayed_load(regs);
            gte.command(instr.0);
        } else {
            match cop_opcode {
                0b00000 => self.op_mfc2(instr, regs, gte),
                0b00010 => self.op_cfc2(instr, regs, gte),
                0b00100 => self.op_mtc2(instr, regs, gte),
                0b00110 => self.op_ctc2(instr, regs, gte),
                _ => panic!(
                    "Unhandled cop2 instruction:  {:02X} ({:b})",
                    instr.cop_opcode(),
                    instr.cop_opcode()
                ),
            }
        }
    }
    pub fn op_cop3(&mut self, _: Instruction, regs: &mut Registers) {
        self.exception(Exception::CoprocessorError, regs);
    }
    // move from coprocessor 2 data register
    pub fn op_mfc2(&mut self, instr: Instruction, regs: &mut Registers, gte: &mut Gte) {
        let cpu_r = instr.rt();
        let cop_r = instr.rd() as u8;

        let v = gte.data(cop_r);

        self.delayed_load_chain(cpu_r, v, regs);
    }
    // move from coprocessor 2 control register
    pub fn op_cfc2(&mut self, instr: Instruction, regs: &mut Registers, gte: &mut Gte) {
        let v = gte.control(instr.rd() as u8);
        self.delayed_load_chain(instr.rt(), v, regs);
    }
    // move to coprocessor 2 data register
    pub fn op_mtc2(&mut self, instr: Instruction, regs: &mut Registers, gte: &mut Gte) {
        let cpu_r = instr.rt();
        let cop_r = instr.rd();

        let v = self.reg(cpu_r, regs);

        self.delayed_load(regs);

        gte.set_data(cop_r as u8, v);
    }
    // move to coprocessor 2 control register
    pub fn op_ctc2(&mut self, instr: Instruction, regs: &mut Registers, gte: &mut Gte) {
        let cpu_r = instr.rt();
        let cop_r = instr.rd();

        let v = self.reg(cpu_r, regs);

        self.delayed_load(regs);

        gte.set_control(cop_r as u8, v);
    }
    pub fn op_mfc0(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = match instr.rd() {
            6 => 0,          // jumpdest..
            7 => 0,          // not used (0)
            8 => regs.baddr, // bad virtual address (R),
            12 => regs.sr,
            13 => regs.cause,
            14 => regs.epc,
            15 => 0x00000002, // Processor ID
            x => panic!("unhandled read from the cop0r{} register", x),
        };
        self.delayed_load_chain(instr.rt(), v, regs);
    }
    pub fn op_mtc0(&mut self, instr: Instruction, regs: &mut Registers) {
        let v = self.reg(instr.rt(), regs);
        self.delayed_load(regs);
        match instr.rd() {
            3 | 5 | 6 | 7 | 9 | 11 => {
                // breakpoint registers
                if v != 0 {
                    panic!("unhandled cop0 breakpoint register write");
                }
            }
            8 => regs.baddr = v,
            12 => regs.sr = v,
            13 => {
                // cause register
                regs.cause = (regs.cause & !0x300) | (v & 0x300);
                // if v != 0 {
                //     panic!("unhandled cop0 cause register write");
                // }
            }
            n => panic!("Unhandled cop0 register {:08X}", n),
        }
    }
    // Return from exception
    pub fn op_rfe(&mut self, instr: Instruction, regs: &mut Registers) {
        if instr.0 & 0x3f != 0b010000 {
            panic!("Invalid cop0 instruction {:x}", instr.0);
        }
        // self.delayed_load(regs);
        let mode = regs.sr & 0x3f;
        regs.sr &= !0xf;
        regs.sr |= mode >> 2;
    }

}
