#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Reg {
    Rax = 0,
    Rcx,
    Rdx,
    Rbx,
    Rsp,
    Rbp,
    Rsi,
    Rdi,
    R8,
    R9,
    R10,
    R11,
    R12,
    R13,
    R14,
    R15,
}

impl Reg {
    fn low(self) -> u8 {
        self as u8 & 0b111
    }

    fn ext(self) -> u8 {
        (self as u8 >> 3) & 1
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum Cond {
    O = 0x0,
    NO = 0x1,
    E = 0x4,
    NE = 0x5,
    S = 0x8,
    NS = 0x9,
    L = 0xC,
    GE = 0xD,
    LE = 0xE,
    G = 0xF,
}

#[derive(Debug, Clone, Copy)]
pub enum Alu {
    Add,
    Sub,
    Or,
    And,
    Xor,
    Cmp,
}

impl Alu {
    fn digit(self) -> u8 {
        match self {
            Alu::Add => 0,
            Alu::Sub => 5,
            Alu::Or => 1,
            Alu::And => 4,
            Alu::Xor => 6,
            Alu::Cmp => 7,
        }
    }

    fn rr_opcode(self) -> u8 {
        match self {
            Alu::Add => 0x01,
            Alu::Sub => 0x29,
            Alu::Or => 0x09,
            Alu::And => 0x21,
            Alu::Xor => 0x31,
            Alu::Cmp => 0x39,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Label(usize);

#[derive(Debug, Clone, Copy)]
pub struct DataLabel(usize);

enum Target {
    Code(Label),
    Data(DataLabel),
}

struct Fixup {
    pos: usize,
    target: Target,
}

pub struct Assembler {
    pub code: Vec<u8>,
    pub rodata: Vec<u8>,
    labels: Vec<Option<usize>>,
    fixups: Vec<Fixup>,
}

impl Assembler {
    pub fn new() -> Self {
        Self {
            code: Vec::new(),
            rodata: Vec::new(),
            labels: Vec::new(),
            fixups: Vec::new(),
        }
    }

    /// Basic Utils

    fn byte(&mut self, b: u8) {
        self.code.push(b);
    }

    fn bytes(&mut self, b: &[u8]) {
        self.code.extend_from_slice(b);
    }

    fn rex(&mut self, w: bool, reg_ext: u8, rm_ext: u8) {
        let rex = 0x40 | (w as u8) << 3 | reg_ext << 2 | rm_ext;
        if rex != 0x40 {
            self.byte(rex);
        }
    }

    fn modrm_rr(&mut self, reg: u8, rm: Reg) {
        self.byte(0b11 << 6 | (reg & 7) << 3 | rm.low());
    }

    fn modrm_mem(&mut self, reg: u8, base: Reg, disp: i32) {
        let needs_disp = disp != 0 || base.low() == 5;
        let md = if !needs_disp {
            0b00
        } else if (-128..=127).contains(&disp) {
            0b01
        } else {
            0b10
        };
        self.byte(md << 6 | (reg & 7) << 3 | base.low());
        // rsp/r12 in rm would say "SIB follow"
        if base.low() == 4 {
            self.byte(0x24);
        }
        match md {
            0b01 => self.byte(disp as i8 as u8),
            0b10 => self.bytes(&disp.to_le_bytes()),
            _ => {}
        }
    }

    /// Labels & Data

    pub fn new_label(&mut self) -> Label {
        self.labels.push(None);
        Label(self.labels.len() - 1)
    }

    pub fn bind(&mut self, label: Label) {
        assert!(self.labels[label.0].is_none(), "ICE: label bound twice");
        self.labels[label.0] = Some(self.code.len());
    }

    pub fn offset_of(&self, label: Label) -> usize {
        self.labels[label.0].expect("ICE: label never bound")
    }

    pub fn data(&mut self, bytes: &[u8]) -> DataLabel {
        while self.rodata.len() % 8 != 0 {
            self.rodata.push(0);
        }
        let offset = self.rodata.len();
        self.rodata.extend_from_slice(bytes);
        DataLabel(offset)
    }

    pub fn data_str(&mut self, s: &str) -> DataLabel {
        let mut bytes = (s.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(s.as_bytes());
        self.data(&bytes)
    }

    fn rel32(&mut self, target: Target) {
        self.fixups.push(Fixup {
            pos: self.code.len(),
            target,
        });
        self.bytes(&[0; 4]); // fix in link()
    }

    pub fn link(&mut self, code_addr: u64, rodata_addr: u64) {
        for fixup in &self.fixups {
            let target = match &fixup.target {
                Target::Code(l) => {
                    code_addr + self.labels[l.0].expect("ICE: label never bound") as u64
                }
                Target::Data(d) => rodata_addr + d.0 as u64,
            };
            let next_ip = code_addr + fixup.pos as u64 + 4;
            let rel = i32::try_from(target as i64 - next_ip as i64).expect("ICE: jump too far");
            self.code[fixup.pos..fixup.pos + 4].copy_from_slice(&rel.to_le_bytes());
        }
    }

    /// Instructions

    pub fn push(&mut self, r: Reg) {
        self.rex(false, 0, r.ext());
        self.byte(0x50 + r.low());
    }

    pub fn pop(&mut self, r: Reg) {
        self.rex(false, 0, r.ext());
        self.byte(0x58 + r.low());
    }

    pub fn ret(&mut self) {
        self.byte(0xC3);
    }

    pub fn syscall(&mut self) {
        self.bytes(&[0x0F, 0x05]);
    }

    pub fn cqo(&mut self) {
        self.bytes(&[0x48, 0x99]);
    }

    pub fn mov_rr(&mut self, dst: Reg, src: Reg) {
        if dst == src {
            return;
        }

        self.rex(true, src.ext(), dst.ext());
        self.byte(0x89);
        self.modrm_rr(src as u8, dst);
    }

    pub fn mov_ri(&mut self, dst: Reg, imm: i64) {
        if let Ok(imm32) = u32::try_from(imm) {
            // move r32, imm32
            self.rex(false, 0, dst.ext());
            self.byte(0xB8 + dst.low());
            self.bytes(&imm32.to_le_bytes());
        } else if let Ok(imm32) = i32::try_from(imm) {
            // move r64, imm32 (sign-extended)
            self.rex(true, 0, dst.ext());
            self.byte(0xC7);
            self.modrm_rr(0, dst);
            self.bytes(&imm32.to_le_bytes());
        } else {
            // move r64, imm64
            self.rex(true, 0, dst.ext());
            self.byte(0xB8 + dst.low()); // long format, imm64
            self.bytes(&imm.to_le_bytes());
        }
    }

    pub fn zero(&mut self, r: Reg) {
        self.rex(false, r.ext(), r.ext());
        self.byte(0x31);
        self.modrm_rr(r as u8, r);
    }

    pub fn load(&mut self, dst: Reg, base: Reg, disp: i32) {
        self.rex(true, dst.ext(), base.ext());
        self.byte(0x8B);
        self.modrm_mem(dst as u8, base, disp);
    }

    pub fn load8(&mut self, dst: Reg, base: Reg) {
        self.rex(true, dst.ext(), base.ext());
        self.bytes(&[0x0F, 0xB6]);
        self.modrm_mem(dst as u8, base, 0);
    }

    pub fn store(&mut self, base: Reg, disp: i32, src: Reg) {
        self.rex(true, src.ext(), base.ext());
        self.byte(0x89);
        self.modrm_mem(src as u8, base, disp);
    }

    pub fn store8(&mut self, base: Reg, src: Reg) {
        let rex = 0x40 | src.ext() << 2 | base.ext();
        if rex != 0x40 || src.low() >= 4 {
            self.byte(rex);
        }
        self.byte(0x88);
        self.modrm_mem(src as u8, base, 0);
    }

    pub fn lea(&mut self, dst: Reg, base: Reg, disp: i32) {
        self.rex(true, dst.ext(), base.ext());
        self.byte(0x8D);
        self.modrm_mem(dst as u8, base, disp);
    }

    pub fn lea_data(&mut self, dst: Reg, data: DataLabel) {
        self.rex(true, dst.ext(), 0);
        self.byte(0x8D);
        self.byte(0b00 << 6 | dst.low() << 3 | 0b101);
        self.rel32(Target::Data(data));
    }

    pub fn alu_rr(&mut self, op: Alu, dst: Reg, src: Reg) {
        self.rex(true, src.ext(), dst.ext());
        self.byte(op.rr_opcode());
        self.modrm_rr(src as u8, dst);
    }

    pub fn alu_ri(&mut self, op: Alu, dst: Reg, imm: i32) {
        self.rex(true, 0, dst.ext());
        if let Ok(imm8) = i8::try_from(imm) {
            self.byte(0x83); // short format, imm8 extended
            self.modrm_rr(op.digit(), dst);
            self.byte(imm8 as u8);
        } else {
            self.byte(0x81);
            self.modrm_rr(op.digit(), dst);
            self.bytes(&imm.to_le_bytes());
        }
    }

    pub fn test_rr(&mut self, a: Reg, b: Reg) {
        self.rex(true, b.ext(), a.ext());
        self.byte(0x85);
        self.modrm_rr(b as u8, a);
    }

    pub fn imul_rr(&mut self, dst: Reg, src: Reg) {
        self.rex(true, dst.ext(), src.ext());
        self.bytes(&[0x0F, 0xAF]);
        self.modrm_rr(dst as u8, src);
    }

    fn group_f7(&mut self, digit: u8, r: Reg) {
        self.rex(true, 0, r.ext());
        self.byte(0xF7);
        self.modrm_rr(digit, r);
    }

    pub fn neg(&mut self, r: Reg) {
        self.group_f7(3, r);
    }

    pub fn div(&mut self, r: Reg) {
        self.group_f7(6, r);
    }

    pub fn idiv(&mut self, r: Reg) {
        self.group_f7(7, r);
    }

    pub fn setcc(&mut self, cc: Cond, dst: Reg) {
        assert!((dst as u8) < 4, "ICE: setcc only supports al/cl/dl/bl");
        self.bytes(&[0x0F, 0x90 + cc as u8]);
        self.modrm_rr(0, dst);
    }

    pub fn movzx8(&mut self, dst: Reg, src: Reg) {
        assert!((src as u8) < 4, "ICE: movzx8 only supports al/cl/dl/bl");
        self.rex(true, dst.ext(), 0);
        self.bytes(&[0x0F, 0xB6]);
        self.modrm_rr(dst as u8, src);
    }

    pub fn in_al_dx(&mut self) {
        self.byte(0xEC);
    }

    pub fn out_dx_al(&mut self) {
        self.byte(0xEE);
    }

    pub fn out_dx_eax(&mut self) {
        self.byte(0xEF);
    }

    pub fn hlt(&mut self) {
        self.byte(0xF4);
    }

    pub fn cli(&mut self) {
        self.byte(0xFA);
    }

    pub fn jmp(&mut self, label: Label) {
        self.byte(0xE9);
        self.rel32(Target::Code(label))
    }

    pub fn jcc(&mut self, cc: Cond, label: Label) {
        self.bytes(&[0x0F, 0x80 + cc as u8]);
        self.rel32(Target::Code(label));
    }

    pub fn call(&mut self, label: Label) {
        self.byte(0xE8);
        self.rel32(Target::Code(label));
    }
}
