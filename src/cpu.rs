use crate::memory_bus::MemoryBus;

#[derive(Clone, Copy)]
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
        code  & 0x3ffffff
    }
}

pub struct Cpu {
    regs: [u32; 32],
    hi: u32,
    lo: u32,
    pc: u32, // reset value 0xBFC00000
    memory_bus: MemoryBus,
    next_instruction: Instruction,

    sr: u32,
    out_regs: [u32;32],
    load: (u32, u32) // load initiated by the current instruction
}

impl Cpu {
    pub fn new(memory_bus: MemoryBus) -> Cpu {
        let mut regs = [0xdeadbeef; 32];
        regs[0] = 0;
        Cpu {
            regs,
            hi: 0,
            lo: 0,
            pc: 0xBFC00000,
            memory_bus,
            next_instruction: Instruction(0x0),
            sr: 0,
            out_regs: regs,
            load: (0, 0)
        }
    }
    pub fn run_next_instruction(&mut self) {
        let instr = self.next_instruction;
        self.next_instruction = Instruction(self.load32(self.pc));
        self.pc = self.pc.wrapping_add(4);
        let (reg, val) = self.load;
        self.set_reg(reg, val);
        self.load = (0, 0);
        self.decode_and_execute(instr);
        self.regs = self.out_regs;
    }

    pub fn load32(&self, address: u32) -> u32 {
        self.memory_bus.load32(address)
    }

    pub fn store32(&mut self, address: u32, value: u32) {
        self.memory_bus.store32(address, value);
    }

    pub fn set_reg(&mut self, index: u32, value: u32) {
        self.out_regs[index as usize] = value;
        self.out_regs[0] = 0;
    }
    pub fn reg(&mut self, index: u32) -> u32 {
        self.regs[index as usize]
    }

    pub fn decode_and_execute(&mut self, instr: Instruction) {
        match instr.opcode() {
            0x00 => match instr.secondary_opcode() {
                0x00 => self.op_sll(instr),
                0x21 => self.op_addu(instr),
                0x25 => self.op_or(instr),
                0x2B => self.op_sltu(instr),
                _ => panic!("Unhandled secondary instruction: opcode: 0x{:02X}" , instr.secondary_opcode()),
            },
            0x02 => self.op_j(instr),
            0x05 => self.op_bne(instr),
            0x08 => self.op_addi(instr),
            0x09 => self.op_addiu(instr),
            0x10 => self.op_cop0(instr),
            0x0f => self.op_lui(instr),
            0x0d => self.op_ori(instr),
            0x23 => self.op_lw(instr),
            0x2b => self.op_sw(instr),
            // _ => panic!("Unhandled instruction: {:08X}, opcode: {:02X}", instr, instr.opcode())
            _ => panic!("Unhandled instruction: opcode: {:02X} ({:b})", instr.opcode(), instr.opcode()),
        }
    }

    pub fn op_lui(&mut self, instr: Instruction) {
        let v = instr.imm() << 16;
        self.set_reg(instr.rt(), v);
    }
    pub fn op_ori(&mut self, instr: Instruction) {
        let v = instr.imm() | self.reg(instr.rs());
        self.set_reg(instr.rt(), v);
    }
    pub fn op_or(&mut self, instr: Instruction) {
        let v = self.reg(instr.rs()) | self.reg(instr.rt());
        self.set_reg(instr.rd(), v);
    }
    pub fn op_sw(&mut self, instr: Instruction) {
        let addr = self.reg(instr.rs()).wrapping_add(instr.imm_se());
        if self.sr & 0x10000 != 0 {
            println!("Cache is isolated, ignoring write to {:08x}", addr);
            return;
        }
        let v = self.reg(instr.rt());
        self.store32(addr, v);
    }
    pub fn op_lw(&mut self, instr: Instruction) {
        let addr = self.reg(instr.rs()).wrapping_add(instr.imm_se());
        if self.sr & 0x10000 != 0 {
            println!("Cache is isolated, ignoring read to {:08x}", addr);
            return;
        }
        let v = self.load32(addr);
        self.load = (instr.rt(), v);
        // self.set_reg(instr.rt(), v);
    }
    pub fn op_sll(&mut self, instr: Instruction) {
        let i = instr.imm5();
        let v = self.reg(instr.rt()) << i;
        self.set_reg(instr.rd(), v);
    }
    pub fn op_addi(&mut self, instr: Instruction) {
        let i = instr.imm_se() as i32;
        let s = self.reg(instr.rs()) as i32;
        let v = match s.checked_add(i) {
            Some(v) => v as u32,
            None => panic!("addi overflow!")
        };
        self.set_reg(instr.rt(), v)
    }
    pub fn op_addu(&mut self, instr: Instruction) {
        let v = self.reg(instr.rs()).wrapping_add(self.reg(instr.rt()));
        self.set_reg(instr.rd(), v)
    }
    pub fn op_addiu(&mut self, instr: Instruction) {
        let i = instr.imm_se();
        let v = self.reg(instr.rs()).wrapping_add(i);
        self.set_reg(instr.rt(), v)
    }
    pub fn op_j(&mut self, instr: Instruction) {
        self.pc = (self.pc & 0xf0000000) | (instr.imm26() << 2)
    }

    pub fn branch(&mut self, offset: u32) {
        let offset = offset << 2;
        let mut pc = self.pc;
        pc = pc.wrapping_add(offset);
        pc = pc.wrapping_sub(4);
        self.pc = pc;
    }
    pub fn op_bne(&mut self, instr: Instruction) {
        if self.reg(instr.rs()) != self.reg(instr.rt()) {
            self.branch(instr.imm_se());
        }
    }
    pub fn op_sltu(&mut self, instr: Instruction) {
        let v = self.reg(instr.rs()) < self.reg(instr.rt());
        self.set_reg(instr.rd(), v as u32);
    }


    pub fn op_cop0(&mut self, instr: Instruction) {
        match instr.cop_opcode() {
            0b00100 => self.op_mtc0(instr),
            _ => panic!("Unhandled cop0 instruction:  {:02X} ({:b})", instr.cop_opcode(), instr.cop_opcode()),
        }
    }
    pub fn op_mtc0(&mut self, instr: Instruction) {
        let v = self.reg(instr.rt());
        match instr.rd() {
            3 | 5 | 6 | 7 | 9 | 11 => { // breakpoint registers
                if v != 0 {
                    panic!("unhandled cop0 breakpoint register write");
                }
            },
            12 => self.sr = v,
            13 => { // cause register
                if v != 0 {
                    panic!("unhandled cop0 cause register write");
                }
            } 
            n => panic!("Unhandled cop0 register {:08X}", n),
        }
    }


    pub fn debug_print(&self) {
        println!("CPU pc: 0x{:X}", self.pc);
        for (i,x) in self.regs.iter().enumerate() {
            println!("regs[{:02}]: 0x{:X}", i, x);

        }
        println!();
    }
}
