use crate::{gte::Gte, memory_bus::{Addressable, MemoryBus}};

#[derive(Clone, Copy, Debug)]
pub struct Instruction(u32);

impl Instruction {
    /// Return bits 31..26
    pub fn opcode(&self) -> u32 {
        let Instruction(code) = self;
        code >> 26
    }
    // bits 0..5
    pub fn secondary_opcode(&self) -> u32 {
        let Instruction(code) = self;
        code & 0x3f
    }
    pub fn cop_opcode(&self) -> u32 {
        let Instruction(code) = self;
        code >> 21 & 0x1f
    }
    /// Return bits 20..16
    pub fn rt(&self) -> u32 {
        let Instruction(code) = self;
        code >> 16 & 0x1f
    }
    pub fn rs(&self) -> u32 {
        let Instruction(code) = self;
        code >> 21 & 0x1f
    }
    /// Return bits 15..11
    pub fn rd(&self) -> u32 {
        let Instruction(code) = self;
        code >> 11 & 0x1f
    }
    pub fn imm(&self) -> u32 {
        let Instruction(code) = self;
        code & 0xffff
    }

    pub fn imm_se(&self) -> u32 {
        let Instruction(code) = self;
        let v = (code & 0xffff) as i16;
        v as u32
    }
    // in bits [10..6]
    pub fn imm5(&self) -> u32 {
        let Instruction(code) = self;
        (code >> 6) & 0x1f
    }
    // in bits [25..0]
    pub fn imm26(&self) -> u32 {
        let Instruction(code) = self;
        code & 0x3ff_ffff
    }
}

enum Exception {
    ExternalInterrupt = 0x0,
    LoadAddressError = 0x4,
    StoreAddressError = 0x5,
    BusErrorOnFetch = 0x6,
    SysCall = 0x8,
    Break = 0x9,
    IllegalInstruction = 0xa,
    CoprocessorError = 0xb,
    Overflow = 0xc,
}

pub struct Cpu {
    regs: [u32; 32],
    hi: u32,
    lo: u32,
    pub pc: u32, // reset value 0xBFC00000
    pub next_pc: u32,
    pub current_pc: u32,
    pub memory_bus: MemoryBus,

    sr: u32,
    cause: u32,
    epc: u32,
    baddr: u32,
    // out_regs: [u32; 32],
    load: (u32, u32), // load initiated by the current instruction
    branch: bool, // set by the current instruction if the branch occurred
    delay_slot:bool, // set if the current instruction executes in the delay slot
    gte: Gte,
}

impl Cpu {
    pub fn new(memory_bus: MemoryBus) -> Cpu {
        let mut regs = [0xdeadbeef; 32];
        regs[0] = 0;
        Cpu {
            regs,
            hi: 0xdeadbeef,
            lo: 0xdeadbeef,
            pc: 0xBFC00000,
            next_pc: 0xBFC00004,
            current_pc: 0xBFC00000,
            memory_bus,
            sr: 0,
            cause: 0,
            epc: 0,
            baddr: 0,
            // out_regs: regs,
            load: (0, 0),
            branch: false,
            delay_slot: false,
            gte: Gte::default(),
        }
    }
    pub fn run_next_instruction(&mut self) {
        if self.pc & 3 != 0 {
            return self.exception(Exception::LoadAddressError);
        }
        let instr = Instruction(self.load::<u32>(self.pc));
        self.current_pc = self.pc;
        self.pc = self.next_pc;
        self.next_pc = self.next_pc.wrapping_add(4);
        let (reg, val) = self.load;
        self.set_reg(reg, val);
        self.load = (0, 0);
        self.delay_slot = self.branch;
        self.branch = false;


        // TODO: check if below handling is correct
        let is_gte = (instr.0 & 0xFE00_0000) == 0x4A00_0000;
        let pending_interrupt = self.check_for_pending_interrupts();

        if !pending_interrupt || (is_gte && !self.delay_slot) {
            self.decode_and_execute(instr);
        }

        if pending_interrupt {
            self.exception(Exception::ExternalInterrupt);
        }
        // self.regs = self.out_regs;
    }

    pub fn check_for_pending_interrupts(&mut self) -> bool {
        if self.memory_bus.irqctl.pending() {
            self.cause |= 1 << 10;
        } else {
            self.cause &= !(1 << 10);
        }
        // mask bits 8..15
        let pending = (self.cause & self.sr) & 0x700;//0xFF00;
        pending != 0 && (self.sr & 1 != 0)
    }

    pub fn check_for_tty_output(&self) {
        let pc = self.pc & 0x1FFFFFFF;
        if (pc == 0xA0 && self.regs[9] ==  0x3C) |  (pc == 0xB0 && self.regs[9] ==  0x3D) {
            let ch = self.regs[4] as u8 as char;
            print!("{ch}");
        }
    }

    pub fn load<T:Addressable>(&mut self, address: u32) -> T {
        self.memory_bus.load(address)
    }

    pub fn store<T:Addressable>(&mut self, address: u32, value: T) {
        if self.sr & 0x10000 != 0 {
            // println!("Ignoring store while cache is isolated");
            return;
        }
        self.memory_bus.store(address, value)
    }
    //
    // pub fn load32(&self, address: u32) -> u32 {
    //     self.memory_bus.load32(address)
    // }
    // pub fn load16(&self, address: u32) -> u16 {
    //     self.memory_bus.load16(address)
    // }
    // pub fn load8(&self, address: u32) -> u8 {
    //     self.memory_bus.load8(address)
    // }
    //
    // pub fn store32(&mut self, address: u32, value: u32) {
    //     self.memory_bus.store32(address, value);
    // }
    // pub fn store16(&mut self, address: u32, value: u16) {
    //     self.memory_bus.store16(address, value);
    // }
    // pub fn store8(&mut self, address: u32, value: u8) {
    //     self.memory_bus.store8(address, value);
    // }

    pub fn set_reg(&mut self, index: u32, value: u32) {
        self.regs[index as usize] = value;
        self.regs[0] = 0;
    }
    pub fn reg(&mut self, index: u32) -> u32 {
        self.regs[index as usize]
    }

    pub fn decode_and_execute(&mut self, instr: Instruction) {
        match instr.opcode() {
            0x00 => match instr.secondary_opcode() {
                0x00 => self.op_sll(instr),
                0x02 => self.op_srl(instr),
                0x03 => self.op_sra(instr),
                0x04 => self.op_sllv(instr),
                0x06 => self.op_srlv(instr),
                0x07 => self.op_srav(instr),
                0x08 => self.op_jr(instr),
                0x09 => self.op_jalr(instr),
                0x0C => self.op_syscall(instr),
                0x0D => self.op_break(instr),
                0x10 => self.op_mfhi(instr),
                0x11 => self.op_mthi(instr),
                0x12 => self.op_mflo(instr),
                0x13 => self.op_mtlo(instr),
                0x18 => self.op_mult(instr),
                0x19 => self.op_multu(instr),
                0x1A => self.op_div(instr),
                0x1B => self.op_divu(instr),
                0x20 => self.op_add(instr),
                0x21 => self.op_addu(instr),
                0x22 => self.op_sub(instr),
                0x23 => self.op_subu(instr),
                0x24 => self.op_and(instr),
                0x25 => self.op_or(instr),
                0x26 => self.op_xor(instr),
                0x27 => self.op_nor(instr),
                0x2A => self.op_slt(instr),
                0x2B => self.op_sltu(instr),
                _    => self.op_illegal(instr), // TODO: bltz/bgez undocumented dupes
            },
            0x01 => self.op_bxx(instr),
            0x02 => self.op_j(instr),
            0x03 => self.op_jal(instr),
            0x04 => self.op_beq(instr),
            0x05 => self.op_bne(instr),
            0x06 => self.op_blez(instr),
            0x07 => self.op_bgtz(instr),
            0x08 => self.op_addi(instr),
            0x09 => self.op_addiu(instr),
            0x0A => self.op_slti(instr),
            0x0B => self.op_sltiu(instr),
            0x0C => self.op_andi(instr),
            0x0D => self.op_ori(instr),
            0x0E => self.op_xori(instr),
            0x0F => self.op_lui(instr),
            0x10 => self.op_cop0(instr),
            0x11 => self.op_cop1(instr),
            0x12 => self.op_cop2(instr),
            0x13 => self.op_cop3(instr),
            0x20 => self.op_lb(instr),
            0x21 => self.op_lh(instr),
            0x22 => self.op_lwl(instr),
            0x23 => self.op_lw(instr),
            0x24 => self.op_lbu(instr),
            0x25 => self.op_lhu(instr),
            0x26 => self.op_lwr(instr),
            0x28 => self.op_sb(instr),
            0x29 => self.op_sh(instr),
            0x2a => self.op_swl(instr),
            0x2b => self.op_sw(instr),
            0x2e => self.op_swr(instr),
            0x30 => self.op_lwc0(instr),
            0x31 => self.op_lwc1(instr),
            0x32 => self.op_lwc2(instr),
            0x33 => self.op_lwc3(instr),
            0x38 => self.op_swc0(instr),
            0x39 => self.op_swc1(instr),
            0x3A => self.op_swc2(instr),
            0x3B => self.op_swc3(instr),
            // _ => panic!("Unhandled instruction: {:08X}, opcode: {:02X}", instr, instr.opcode())
            _ => self.op_illegal(instr),
        }
    }

    pub fn op_illegal(&mut self, instr: Instruction) {
        self.delayed_load();
             println!(
                "Illegal instruction: {:X}, opcode: {:02X}",
                instr.0,
                instr.opcode());
        self.exception(Exception::IllegalInstruction);
    }

    pub fn delayed_load(&mut self) {
        let (reg, val) = self.load;
        self.set_reg(reg, val);
        self.load = (0, 0);
    }
    pub fn delayed_load_chain(&mut self, reg:u32, val:u32) {
        let (pending_reg, pending_val) = self.load;
        if pending_reg != reg {
            self.set_reg(pending_reg, pending_val);
        }
        self.load = (reg, val);
    }

    pub fn op_lui(&mut self, instr: Instruction) {
        let v = instr.imm() << 16;
        self.delayed_load();
        self.set_reg(instr.rt(), v);
    }
    pub fn op_ori(&mut self, instr: Instruction) {
        let v = instr.imm() | self.reg(instr.rs());
        self.delayed_load();
        self.set_reg(instr.rt(), v);
    }
    pub fn op_andi(&mut self, instr: Instruction) {
        let v = instr.imm() & self.reg(instr.rs());
        self.delayed_load();
        self.set_reg(instr.rt(), v);
    }
    pub fn op_or(&mut self, instr: Instruction) {
        let v = self.reg(instr.rs()) | self.reg(instr.rt());
        self.delayed_load();
        self.set_reg(instr.rd(), v);
    }
    pub fn op_xor(&mut self, instr: Instruction) {
        let v = self.reg(instr.rs()) ^ self.reg(instr.rt());
        self.delayed_load();
        self.set_reg(instr.rd(), v);
    }
    pub fn op_xori(&mut self, instr: Instruction) {
        let v = self.reg(instr.rs()) ^ instr.imm();
        self.delayed_load();
        self.set_reg(instr.rt(), v);
    }
    pub fn op_nor(&mut self, instr: Instruction) {
        let v = !(self.reg(instr.rs()) | self.reg(instr.rt()));
        self.delayed_load();
        self.set_reg(instr.rd(), v);
    }
    pub fn op_and(&mut self, instr: Instruction) {
        let v = self.reg(instr.rs()) & self.reg(instr.rt());
        self.delayed_load();
        self.set_reg(instr.rd(), v);
    }
    pub fn op_sw(&mut self, instr: Instruction) {
        let addr = self.reg(instr.rs()).wrapping_add(instr.imm_se());
        let v = self.reg(instr.rt());
        if addr % 4 != 0 {
            self.delayed_load();
            return self.exception(Exception::StoreAddressError);
        }
        self.delayed_load();
        self.store::<u32>(addr, v);
    }
    pub fn op_sh(&mut self, instr: Instruction) {
        let addr = self.reg(instr.rs()).wrapping_add(instr.imm_se());
        let v = self.reg(instr.rt());
        self.delayed_load();
        if addr % 2 != 0 {
            return self.exception(Exception::StoreAddressError);
        }
        self.store::<u16>(addr, v as u16);
    }
    pub fn op_sb(&mut self, instr: Instruction) {
        let addr = self.reg(instr.rs()).wrapping_add(instr.imm_se());
        let v = self.reg(instr.rt());
        self.delayed_load();
        self.store::<u8>(addr, v as u8);
    }
    pub fn op_lh(&mut self, instr: Instruction) {
        let addr = self.reg(instr.rs()).wrapping_add(instr.imm_se());
        if addr % 2 != 0 {
            self.delayed_load();
            return self.exception(Exception::LoadAddressError);
        }
        let v = (self.load::<u16>(addr) as u16) as i16;
        self.delayed_load_chain(instr.rt(), v as u32);
    }
    pub fn op_lhu(&mut self, instr: Instruction) {
        let addr = self.reg(instr.rs()).wrapping_add(instr.imm_se());
        if addr % 2 != 0 {
            self.delayed_load();
            return self.exception(Exception::LoadAddressError);
        }
        let v = self.load::<u16>(addr);
        self.delayed_load_chain(instr.rt(), v as u32);
    }
    pub fn op_lw(&mut self, instr: Instruction) {
        let addr = self.reg(instr.rs()).wrapping_add(instr.imm_se());
        if addr % 4 != 0 {
            self.delayed_load();
            return self.exception(Exception::LoadAddressError);
        }
        // if self.sr & 0x10000 != 0 {
        //     println!("Cache is isolated, ignoring read to {:08x}", addr);
        //     return;
        // }
        let v = self.load::<u32>(addr);
        self.delayed_load_chain(instr.rt(), v);
    }
    pub fn op_lwl(&mut self, instr: Instruction) {
        let addr = self.reg(instr.rs()).wrapping_add(instr.imm_se());
        let mut cur_v = self.regs[instr.rt() as usize];
        let aligned_addr = addr & !3;
        let aligned_word = self.load::<u32>(aligned_addr);

        
        // This instruction bypasses the load delay restriction: this instruction will merge the new
        // contents with the value currently being loaded if need be.
        let (pending_reg, pending_value) = self.load;
        if pending_reg == instr.rt()
        {
            cur_v = pending_value;
        }

        let v = match addr & 3 {
            0 => (cur_v & 0x00ffffff) | (aligned_word << 24),
            1 => (cur_v & 0x0000ffff) | (aligned_word << 16),
            2 => (cur_v & 0x000000ff) | (aligned_word << 8),
            3 => (cur_v & 0x00000000) | (aligned_word),
            _ => unreachable!()
        };
        self.delayed_load_chain(instr.rt(), v);
    }
    pub fn op_swl(&mut self, instr: Instruction) {
        let addr = self.reg(instr.rs()).wrapping_add(instr.imm_se());
        let v = self.reg(instr.rt());
        let aligned_addr = addr & !3;
        let cur_mem = self.load::<u32>(aligned_addr);

        let mem = match addr & 3 {
            0 => (cur_mem & 0xffffff00) | (v >> 24),
            1 => (cur_mem & 0xffff0000) | (v >> 16),
            2 => (cur_mem & 0xff000000) | (v >> 8),
            3 => (cur_mem & 0x00000000) | (v),
            _ => unreachable!()
        };
        self.delayed_load();
        self.store::<u32>(aligned_addr, mem);
    }
    pub fn op_swr(&mut self, instr: Instruction) {
        let addr = self.reg(instr.rs()).wrapping_add(instr.imm_se());
        // if self.sr & 0x10000 != 0 {
        //     println!("Cache is isolated, ignoring read to {:08x}", addr);
        //     return;
        // }
        let v = self.reg(instr.rt());
        let aligned_addr = addr & !3;
        let cur_mem = self.load::<u32>(aligned_addr);

        let mem = match addr & 3 {
            0 => (cur_mem & 0x00000000) | (v),
            1 => (cur_mem & 0x000000ff) | (v << 8),
            2 => (cur_mem & 0x0000ffff) | (v << 16),
            3 => (cur_mem & 0x00ffffff) | (v << 24),
            _ => unreachable!()
        };
        self.delayed_load();
        self.store::<u32>(aligned_addr, mem);
    }
    pub fn op_lwr(&mut self, instr: Instruction) {
        let addr = self.reg(instr.rs()).wrapping_add(instr.imm_se());
        let mut cur_v = self.regs[instr.rt() as usize];
        let aligned_addr = addr & !3;
        let aligned_word = self.load::<u32>(aligned_addr);

        let (pending_reg, pending_value) = self.load;
        if pending_reg == instr.rt()
        {
            cur_v = pending_value;
        }

        let v = match addr & 3 {
            0 => (cur_v & 0x00000000) | (aligned_word),
            1 => (cur_v & 0xff000000) | (aligned_word >> 8),
            2 => (cur_v & 0xffff0000) | (aligned_word >> 16),
            3 => (cur_v & 0xffffff00) | (aligned_word >> 24),
            _ => unreachable!()
        };
        self.delayed_load_chain(instr.rt(), v);
    }
    pub fn op_lb(&mut self, instr: Instruction) {
        let addr = self.reg(instr.rs()).wrapping_add(instr.imm_se());
        if addr == 0xfffffcac {
            println!("gotcha!!!");
            println!("next_pc {:X}", self.next_pc);
            self.debug_print();
        }
        let v = (self.load::<u8>(addr) as u8) as i8;
        self.delayed_load_chain(instr.rt(), v as u32);
    }
    pub fn op_lbu(&mut self, instr: Instruction) {
        let addr = self.reg(instr.rs()).wrapping_add(instr.imm_se());
        let v = self.load::<u8>(addr);
        self.delayed_load_chain(instr.rt(), v as u32);
    }
    pub fn op_sll(&mut self, instr: Instruction) {
        let i = instr.imm5();
        let v = self.reg(instr.rt()) << i;
        self.delayed_load();
        self.set_reg(instr.rd(), v);
    }
    pub fn op_sllv(&mut self, instr: Instruction) {
        let v = self.reg(instr.rt()) << (self.reg(instr.rs()) & 0x1f);
        self.delayed_load();
        self.set_reg(instr.rd(), v);
    }
    pub fn op_sra(&mut self, instr: Instruction) {
        let i = instr.imm5();
        let v = (self.reg(instr.rt()) as i32) >> i;
        self.delayed_load();
        self.set_reg(instr.rd(), v as u32);
    }
    // shift right arithmetic variable
    pub fn op_srav(&mut self, instr: Instruction) {
        let v = (self.reg(instr.rt()) as i32) >> (self.reg(instr.rs()) & 0x1f);
        self.delayed_load();
        self.set_reg(instr.rd(), v as u32);
    }
    pub fn op_srlv(&mut self, instr: Instruction) {
        let v = (self.reg(instr.rt())) >> (self.reg(instr.rs()) & 0x1f);
        self.delayed_load();
        self.set_reg(instr.rd(), v);
    }
    pub fn op_srl(&mut self, instr: Instruction) {
        let i = instr.imm5();
        let v = (self.reg(instr.rt())) >> i;
        self.delayed_load();
        self.set_reg(instr.rd(), v);
    }
    pub fn op_add(&mut self, instr: Instruction) {
        let s = self.reg(instr.rs()) as i32;
        let t = self.reg(instr.rt()) as i32;
        let v = match s.checked_add(t) {
            Some(v) => v as u32,
            None => return self.exception(Exception::Overflow),
        };
        self.delayed_load();
        self.set_reg(instr.rd(), v)
    }
    pub fn op_addi(&mut self, instr: Instruction) {
        let i = instr.imm_se() as i32;
        let s = self.reg(instr.rs()) as i32;
        let v = match s.checked_add(i) {
            Some(v) => v as u32,
            None => return self.exception(Exception::Overflow),
        };
        self.delayed_load();
        self.set_reg(instr.rt(), v)
    }
    pub fn op_addu(&mut self, instr: Instruction) {
        let v = self.reg(instr.rs()).wrapping_add(self.reg(instr.rt()));
        self.delayed_load();
        self.set_reg(instr.rd(), v)
    }
    pub fn op_sub(&mut self, instr: Instruction) {
        let s = self.reg(instr.rs()) as i32;
        let t = self.reg(instr.rt()) as i32;
        let d = instr.rd();
        self.delayed_load();
        match s.checked_sub(t) {
            Some(v) => self.set_reg(d, v as u32),
            None => self.exception(Exception::Overflow),
        }
    }
    pub fn op_subu(&mut self, instr: Instruction) {
        let v = self.reg(instr.rs()).wrapping_sub(self.reg(instr.rt()));
        self.delayed_load();
        self.set_reg(instr.rd(), v)
    }
    pub fn op_addiu(&mut self, instr: Instruction) {
        let i = instr.imm_se();
        let v = self.reg(instr.rs()).wrapping_add(i);
        self.delayed_load();
        self.set_reg(instr.rt(), v)
    }
    // TODO: stalls if division is not yet complete
    pub fn op_mflo(&mut self, instr: Instruction) {
        self.delayed_load();
        self.set_reg(instr.rd(), self.lo);
    }
    pub fn op_mtlo(&mut self, instr: Instruction) {
        self.delayed_load();
        self.lo = self.reg(instr.rs());
    }
    pub fn op_mthi(&mut self, instr: Instruction) {
        self.delayed_load();
        self.hi = self.reg(instr.rs());
    }
    pub fn op_mfhi(&mut self, instr: Instruction) {
        self.delayed_load();
        self.set_reg(instr.rd(), self.hi);
    }
    pub fn op_mult(&mut self, instr: Instruction) {
        let a = (self.reg(instr.rs()) as i32) as i64;
        let b = (self.reg(instr.rt()) as i32) as i64;

        let v = (a * b) as u64;
        self.delayed_load();

        self.hi = (v >> 32) as u32;
        self.lo = v as u32;
    }
    pub fn op_multu(&mut self, instr: Instruction) {
        let a = self.reg(instr.rs()) as u64;
        let b = self.reg(instr.rt()) as u64;

        let v = a * b;

        self.delayed_load();
        self.hi = (v >> 32) as u32;
        self.lo = v as u32;
    }
    pub fn op_div(&mut self, instr: Instruction) {
        let s = instr.rs();
        let t = instr.rt();

        let n = self.reg(s) as i32;
        let d = self.reg(t) as i32;
        self.delayed_load();

        if d == 0 {
            self.hi = n as u32;
            if n >= 0 {
                self.lo = 0xffffffff;
            } else {
                self.lo = 1;
            }
        } else if n as u32 == 0x80000000 && d == -1 {
            self.hi = 0;
            self.lo = 0x80000000;
        } else {
            self.hi = (n % d) as u32;
            self.lo = (n / d) as u32;
        }
    }
    pub fn op_divu(&mut self, instr: Instruction) {
        let s = instr.rs();
        let t = instr.rt();

        let n = self.reg(s);
        let d = self.reg(t);
        self.delayed_load();

        if d == 0 {
            self.hi = n as u32;
                self.lo = 0xffffffff;
        } else {
            self.hi = (n % d) as u32;
            self.lo = (n / d) as u32;
        }
    }
    pub fn op_j(&mut self, instr: Instruction) {
        self.next_pc = (self.current_pc & 0xf000_0000) | (instr.imm26() << 2);
        // self.next_pc = (self.next_pc & 0xf000_0000) | (instr.imm26() << 2) + 4;
        // self.next_pc = (self.next_pc & 0xf000_0000) | (instr.imm26() << 2);
        self.branch = true;
        self.delayed_load();
    }
    pub fn op_jal(&mut self, instr: Instruction) {
        let ra = self.next_pc;
        self.next_pc = (self.pc & 0xf0000000) | (instr.imm26() << 2);
        self.delayed_load();
        self.branch = true;
        self.set_reg(31, ra);
    }
    pub fn op_jr(&mut self, instr: Instruction) {
        self.next_pc = self.reg(instr.rs());
        self.delayed_load();
        self.branch = true;
    }

    pub fn op_jalr(&mut self, instr: Instruction) {
        let ra = self.next_pc;
        self.next_pc = self.reg(instr.rs());
        self.delayed_load();
        self.branch = true;
        self.set_reg(instr.rd(), ra);
    }

    pub fn branch(&mut self, offset: u32) {
        let offset = offset << 2;
        self.next_pc = self.pc.wrapping_add(offset);
        self.delayed_load();
        self.branch = true;
    }
    pub fn op_bne(&mut self, instr: Instruction) {
        if self.reg(instr.rs()) != self.reg(instr.rt()) {
            self.branch(instr.imm_se());
        }
    }
    pub fn op_bgtz(&mut self, instr: Instruction) {
        let v = self.reg(instr.rs()) as i32;
        if v > 0 {
            self.branch(instr.imm_se());
        }
    }
    pub fn op_blez(&mut self, instr: Instruction) {
        let v = self.reg(instr.rs()) as i32;
        if v <= 0 {
            self.branch(instr.imm_se());
        }
    }
    pub fn op_beq(&mut self, instr: Instruction) {
        if self.reg(instr.rs()) == self.reg(instr.rt()) {
            self.branch(instr.imm_se());
        }
    }
    // BGEZ, BLTZ, BGEZAL, BLTZAL
    pub fn op_bxx(&mut self, instr: Instruction) {
        let i = instr.imm_se();
        let s = instr.rs();

        let instruction = instr.0;

        let is_bgez = (instruction >> 16) & 1;
        let is_link = (instruction >> 17) & 0xf == 8;

        let v = self.reg(s) as i32;

        let test = (v < 0) as u32;
        let test = test ^ is_bgez;

        self.delayed_load();

        if is_link {
            let ra = self.pc;
            self.set_reg(31, ra);
        }
        if test != 0 {
            self.branch(i);
        }
    }
    pub fn op_slt(&mut self, instr: Instruction) {
        let v = (self.reg(instr.rs()) as i32) < (self.reg(instr.rt()) as i32);
        self.delayed_load();
        self.set_reg(instr.rd(), v as u32);
    }
    pub fn op_sltu(&mut self, instr: Instruction) {
        let v = self.reg(instr.rs()) < self.reg(instr.rt());
        self.delayed_load();
        self.set_reg(instr.rd(), v as u32);
    }
    pub fn op_slti(&mut self, instr: Instruction) {
        let v = (self.reg(instr.rs()) as i32) < (instr.imm_se() as i32);
        self.delayed_load();
        self.set_reg(instr.rt(), v as u32);
    }
    pub fn op_sltiu(&mut self, instr: Instruction) {
        let v = (self.reg(instr.rs())) < (instr.imm_se());
        self.delayed_load();
        self.set_reg(instr.rt(), v as u32);
    }

    fn exception(&mut self, cause: Exception) {
        // TODO:: branch delay slot
        // TODO: bad address exception...
        let mode = self.sr & 0x3f;
        self.sr &= !0x3f;
        self.sr |= (mode << 2) & 0x3f;

        self.cause &= !0x7c;
        self.cause = (cause as u32) << 2;

        if self.delay_slot { // this what happend?
            self.epc = self.current_pc.wrapping_sub(4);
            self.cause |= 1 << 31;
        } else {
            self.epc = self.current_pc;
            self.cause &= !(1 << 31);
        }

        // exception handler address depends on the BEV bit
        let handler: u32 = if self.sr & (1 << 22) != 0 {
            0xbfc00180
            } else {
            0x80000080
        };

        self.pc = handler;
        self.next_pc = handler.wrapping_add(4);
    }

    pub fn op_syscall(&mut self, _: Instruction) {
        self.delayed_load();
        self.exception(Exception::SysCall);
    }
    pub fn op_break(&mut self, _: Instruction) {
        self.delayed_load();
        self.exception(Exception::Break);
    }

    pub fn op_lwc0(&mut self, _: Instruction) {
        self.delayed_load();
        self.exception(Exception::CoprocessorError);
    }
    pub fn op_lwc1(&mut self, _: Instruction) {
        self.delayed_load();
        self.exception(Exception::CoprocessorError);
    }
    // load word in coprocessor 2
    pub fn op_lwc2(&mut self, instr: Instruction) {
        let i = instr.imm_se();
        let cop_r = instr.rt();
        let s = instr.rs();

        let addr = self.reg(s).wrapping_add(i);

        self.delayed_load();

        if addr.is_multiple_of(4) {
            let v = self.load::<u32>(addr);
            self.gte.set_data(cop_r, v);
        } else {
            self.exception(Exception::LoadAddressError);
        }
    }
    pub fn op_lwc3(&mut self, _: Instruction) {
        self.delayed_load();
        self.exception(Exception::CoprocessorError);
    }

    pub fn op_swc0(&mut self, _: Instruction) {
        self.delayed_load();
        self.exception(Exception::CoprocessorError);
    }
    pub fn op_swc1(&mut self, _: Instruction) {
        self.delayed_load();
        self.exception(Exception::CoprocessorError);
    }
    pub fn op_swc2(&mut self, instr: Instruction) {
        println!("unhandled GTE SWC: {:x}", instr.0);
    }
    pub fn op_swc3(&mut self, _: Instruction) {
        self.delayed_load();
        self.exception(Exception::CoprocessorError);
    }


    pub fn op_cop0(&mut self, instr: Instruction) {
        match instr.cop_opcode() {
            0b00000 => self.op_mfc0(instr),
            0b00100 => self.op_mtc0(instr),
            0b10000 => self.op_rfe(instr),
            _ => panic!(
                "Unhandled cop0 instruction:  {:02X} ({:b})",
                instr.cop_opcode(),
                instr.cop_opcode()
            ),
        }
    }
    pub fn op_cop1(&mut self, _: Instruction) {
        self.delayed_load();
        self.exception(Exception::CoprocessorError);
    }
    pub fn op_cop2(&mut self, instr: Instruction) {
        let cop_opcode = instr.cop_opcode();

        if cop_opcode & 0x10 != 0 {
            // GTE command
            self.gte.command(instr.0);
        } else {
            match cop_opcode {
                0b00000 => self.op_mfc2(instr),
                0b00010 => self.op_cfc2(instr),
                0b00100 => self.op_mtc2(instr),
                0b00110 => self.op_ctc2(instr),
                // 0b00100 => self.op_mtc2(instr),
                // 0b10000 => self.op_rfe(instr),
                _ => panic!(
                    "Unhandled cop2 instruction:  {:02X} ({:b})",
                    instr.cop_opcode(),
                    instr.cop_opcode()
                ),
            }
        }
    }
    pub fn op_cop3(&mut self, _: Instruction) {
        self.exception(Exception::CoprocessorError);
    }
    // move from coprocessor 2 data register
    pub fn op_mfc2(&mut self, instr: Instruction) {
        let v = match instr.rd() {
            x => {println!("op_mfc2: unhandled read from the cop2r{} register", x); 0_u32},
        };
        self.delayed_load_chain(instr.rt(), v);
    }
    // move from coprocessor 2 control register
    pub fn op_cfc2(&mut self, instr: Instruction) {
        let v = self.gte.control(instr.rd() as u8);
        self.delayed_load_chain(instr.rt(), v);
    }
    // move to coprocessor 2 data register
    pub fn op_mtc2(&mut self, instr: Instruction) {
        println!("unhandled op_mtc2")

    }
    // move to coprocessor 2 control register
    pub fn op_ctc2(&mut self, instr: Instruction) {
        let cpu_r = instr.rt();
        let cop_r = instr.rd();

        let v = self.reg(cpu_r);

        self.delayed_load();

        self.gte.set_control(cop_r, v);

    }
    pub fn op_mfc0(&mut self, instr: Instruction) {
        let v = match instr.rd() {
            6  => {println!(">>>>> jumpdest"); 0}, // jumpdest..
            7  => 0, // not used (0)
            8  => self.baddr,// bad virtual address (R),
            12 => self.sr,
            13 => self.cause,
            14 => self.epc,
            15 => 0x00000002,// Processor ID
            x => panic!("unhandled read from the cop0r{} register", x),
        };
        self.delayed_load_chain(instr.rt(), v);
    }
    pub fn op_mtc0(&mut self, instr: Instruction) {
        let v = self.reg(instr.rt());
        self.delayed_load();
        match instr.rd() {
            3 | 5 | 6 | 7 | 9 | 11 => {
                // breakpoint registers
                if v != 0 {
                    panic!("unhandled cop0 breakpoint register write");
                }
            }
            8 => self.baddr = v,
            12 => self.sr = v,
            13 => {
                // cause register
                self.cause =  (self.cause & !0x300) | (v & 0x300);
                // if v != 0 {
                //     panic!("unhandled cop0 cause register write");
                // }
            }
            n => panic!("Unhandled cop0 register {:08X}", n),
        }
    }
    // Return from exception
    pub fn op_rfe(&mut self, instr: Instruction) {
        if instr.0 & 0x3f != 0b010000 {
            panic!("Invalid cop0 instruction {:x}", instr.0);
        }
        // self.delayed_load();
        let mode = self.sr & 0x3f;
        self.sr &= !0xf;
        self.sr |= mode >> 2;
    }

    pub fn debug_print(&self) {
        println!("CPU pc: 0x{:X}", self.pc);
        for (i, x) in self.regs.iter().enumerate() {
            println!("regs[{:02}]: 0x{:X}", i, x);
        }
        println!();
    }
}
