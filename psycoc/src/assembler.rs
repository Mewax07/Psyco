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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Cond {
    O = 0x0,
    NO = 0x1,
    B = 0x2,
    AE = 0x3,
    E = 0x4,
    NE = 0x5,
    BE = 0x6,
    A = 0x7,
    S = 0x8,
    NS = 0x9,
    L = 0xC,
    GE = 0xD,
    LE = 0xE,
    G = 0xF,
}

impl Cond {
    pub fn negate(self) -> Cond {
        use Cond::*;
        match self {
            O => NO,
            NO => O,
            B => AE,
            AE => B,
            E => NE,
            NE => E,
            BE => A,
            A => BE,
            S => NS,
            NS => S,
            L => GE,
            GE => L,
            LE => G,
            G => LE,
        }
    }

    pub fn swap(self) -> Cond {
        use Cond::*;
        match self {
            L => G,
            G => L,
            LE => GE,
            GE => LE,
            B => A,
            A => B,
            BE => AE,
            AE => BE,
            E | NE => self,
            O | NO | S | NS => panic!("ICE: Cond::swap on non-comparison {self:?}"),
        }
    }
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

#[derive(Debug, Clone, Copy)]
pub enum Shift {
    Shl,
    Shr,
    Sar,
}

impl Shift {
    fn digit(self) -> u8 {
        match self {
            Shift::Shl => 4,
            Shift::Shr => 5,
            Shift::Sar => 7,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Sreg {
    Es = 0,
    Cs = 1,
    Ss = 2,
    Ds = 3,
    Fs = 4,
    Gs = 5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Label(usize);

#[derive(Debug, Clone, Copy)]
pub struct DataLabel(usize);

#[derive(Debug, Clone, Copy)]
pub enum RwLabel {
    Data(usize),
    Bss(usize),
}

enum Target {
    Code(Label),
    Data(DataLabel),
    Rw(RwLabel),
}

struct Fixup {
    pos: usize,
    target: Target,
}

pub struct Assembler {
    pub code: Vec<u8>,
    pub rodata: Vec<u8>,
    pub data: Vec<u8>,
    pub bss_size: usize,
    labels: Vec<Option<usize>>,
    fixups: Vec<Fixup>,
}

impl Default for Assembler {
    fn default() -> Self {
        Self::new()
    }
}

impl Assembler {
    pub fn new() -> Self {
        Self {
            code: Vec::new(),
            rodata: Vec::new(),
            data: Vec::new(),
            bss_size: 0,
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

    fn rex_byte(&mut self, reg_ext: u8, rm_ext: u8, byte_reg: Reg) {
        let rex = 0x40 | reg_ext << 2 | rm_ext;
        if rex != 0x40 || (4..8).contains(&(byte_reg as u8)) {
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

    pub fn pos(&self) -> usize {
        self.code.len()
    }

    pub fn patch_i32(&mut self, pos: usize, value: i32) {
        self.code[pos..pos + 4].copy_from_slice(&value.to_le_bytes());
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

    pub fn data_aligned(&mut self, bytes: &[u8], align: usize) -> DataLabel {
        while self.rodata.len() % align.max(1) != 0 {
            self.rodata.push(0);
        }
        let offset = self.rodata.len();
        self.rodata.extend_from_slice(bytes);
        DataLabel(offset)
    }

    pub fn data(&mut self, bytes: &[u8]) -> DataLabel {
        while self.rodata.len() % 8 != 0 {
            self.rodata.push(0);
        }
        let offset = self.rodata.len();
        self.rodata.extend_from_slice(bytes);
        DataLabel(offset)
    }

    pub fn data_str(&mut self, s: &[u8]) -> DataLabel {
        let mut bytes = (s.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(s);
        self.data(&bytes)
    }

    pub fn rw_data(&mut self, bytes: &[u8], align: usize) -> RwLabel {
        while self.data.len() % align.max(1) != 0 {
            self.data.push(0);
        }
        let offset = self.data.len();
        self.data.extend_from_slice(bytes);
        RwLabel::Data(offset)
    }

    pub fn bss(&mut self, size: usize, align: usize) -> RwLabel {
        assert!(align <= 4096usize, "ICE: bss alignment too large");
        self.bss_size = self.bss_size.next_multiple_of(align.max(1));
        let offset = self.bss_size;
        self.bss_size += size;
        RwLabel::Bss(offset)
    }

    pub fn bss_offset(&self) -> usize {
        self.data.len().next_multiple_of(4096usize)
    }

    pub fn rw_size(&self) -> usize {
        if self.bss_size == 0 {
            self.data.len()
        } else {
            self.bss_offset() + self.bss_size
        }
    }

    fn rel32(&mut self, target: Target) {
        self.fixups.push(Fixup {
            pos: self.code.len(),
            target,
        });
        self.bytes(&[0; 4]); // fix in link()
    }

    pub fn link(&mut self, code_addr: u64, rodata_addr: u64, data_addr: u64) {
        let bss_addr = data_addr + self.bss_offset() as u64;
        for fixup in &self.fixups {
            let target = match &fixup.target {
                Target::Code(l) => {
                    code_addr + self.labels[l.0].expect("ICE: label never bound") as u64
                }
                Target::Data(d) => rodata_addr + d.0 as u64,
                Target::Rw(RwLabel::Data(o)) => data_addr + *o as u64,
                Target::Rw(RwLabel::Bss(o)) => bss_addr + *o as u64,
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

    pub fn push_imm(&mut self, imm: i32) {
        self.byte(0x68);
        self.bytes(&imm.to_le_bytes());
    }

    pub fn pop(&mut self, r: Reg) {
        self.rex(false, 0, r.ext());
        self.byte(0x58 + r.low());
    }

    pub fn ret(&mut self) {
        self.byte(0xC3);
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

    pub fn call_r(&mut self, r: Reg) {
        self.rex(false, 0, r.ext());
        self.byte(0xFF);
        self.modrm_rr(2, r);
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
        self.rex(false, dst.ext(), base.ext());
        self.bytes(&[0x0F, 0xB6]);
        self.modrm_mem(dst as u8, base, 0);
    }

    pub fn load_sized(&mut self, dst: Reg, base: Reg, disp: i32, size: u64, signed: bool) {
        match (size, signed) {
            (1, false) => {
                self.rex(false, dst.ext(), base.ext());
                self.bytes(&[0x0F, 0xB6]);
            }
            (1, true) => {
                self.rex(true, dst.ext(), base.ext());
                self.bytes(&[0x0F, 0xBE]);
            }
            (2, false) => {
                self.rex(false, dst.ext(), base.ext());
                self.bytes(&[0x0F, 0xB7]);
            }
            (2, true) => {
                self.rex(true, dst.ext(), base.ext());
                self.bytes(&[0x0F, 0xBF]);
            }
            (4, false) => {
                self.rex(false, dst.ext(), base.ext());
                self.byte(0x8B);
            }
            (4, true) => {
                self.rex(true, dst.ext(), base.ext());
                self.byte(0x63); // movsxd
            }
            (8, _) => {
                self.rex(true, dst.ext(), base.ext());
                self.byte(0x8B);
            }
            _ => panic!("ICE: load of size {size}"),
        }
        self.modrm_mem(dst as u8, base, disp);
    }

    pub fn store(&mut self, base: Reg, disp: i32, src: Reg) {
        self.rex(true, src.ext(), base.ext());
        self.byte(0x89);
        self.modrm_mem(src as u8, base, disp);
    }

    pub fn store8(&mut self, base: Reg, src: Reg) {
        self.rex_byte(src.ext(), base.ext(), src);
        self.byte(0x88);
        self.modrm_mem(src as u8, base, 0);
    }

    pub fn store_sized(&mut self, base: Reg, disp: i32, src: Reg, size: u64) {
        match size {
            1 => {
                self.rex_byte(src.ext(), base.ext(), src);
                self.byte(0x88);
            }
            2 => {
                self.byte(0x66);
                self.rex(false, src.ext(), base.ext());
                self.byte(0x89);
            }
            4 => {
                self.rex(false, src.ext(), base.ext());
                self.byte(0x89);
            }
            8 => {
                self.rex(true, src.ext(), base.ext());
                self.byte(0x89);
            }
            _ => panic!("ICE: store of size {size}"),
        }
        self.modrm_mem(src as u8, base, disp);
    }

    pub fn extend(&mut self, r: Reg, size: u64, signed: bool) {
        match (size, signed) {
            (1, false) => self.movzx8(r, r),
            (1, true) => {
                // movsx r64, r8
                self.rex(true, r.ext(), r.ext());
                self.bytes(&[0x0F, 0xBE]);
                self.modrm_rr(r as u8, r);
            }
            (2, false) => {
                self.rex(false, r.ext(), r.ext());
                self.bytes(&[0x0F, 0xB7]);
                self.modrm_rr(r as u8, r);
            }
            (2, true) => {
                self.rex(true, r.ext(), r.ext());
                self.bytes(&[0x0F, 0xBF]);
                self.modrm_rr(r as u8, r);
            }
            (4, false) => {
                // mov r32, r32
                self.rex(false, r.ext(), r.ext());
                self.byte(0x89);
                self.modrm_rr(r as u8, r);
            }
            (4, true) => {
                self.rex(true, r.ext(), r.ext());
                self.byte(0x63);
                self.modrm_rr(r as u8, r);
            }
            (8, _) => {}
            _ => panic!("ICE: extend to size {size}"),
        }
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

    pub fn lea_rw(&mut self, dst: Reg, data: RwLabel) {
        self.rex(true, dst.ext(), 0);
        self.byte(0x8D);
        self.byte(dst.low() << 3 | 0b101);
        self.rel32(Target::Rw(data));
    }

    pub fn lea_code(&mut self, dst: Reg, label: Label) {
        self.rex(true, dst.ext(), 0);
        self.byte(0x8D);
        self.byte(dst.low() << 3 | 0b101);
        self.rel32(Target::Code(label));
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

    pub fn imul_ri(&mut self, dst: Reg, src: Reg, imm: i32) {
        self.rex(true, dst.ext(), src.ext());
        if let Ok(imm8) = i8::try_from(imm) {
            self.byte(0x6B);
            self.modrm_rr(dst as u8, src);
            self.byte(imm8 as u8);
        } else {
            self.byte(0x69);
            self.modrm_rr(dst as u8, src);
            self.bytes(&imm.to_le_bytes());
        }
    }

    fn group_f7(&mut self, digit: u8, r: Reg) {
        self.rex(true, 0, r.ext());
        self.byte(0xF7);
        self.modrm_rr(digit, r);
    }

    pub fn not(&mut self, r: Reg) {
        self.group_f7(2, r);
    }

    pub fn neg(&mut self, r: Reg) {
        self.group_f7(3, r);
    }

    pub fn mul(&mut self, r: Reg) {
        self.group_f7(4, r);
    }

    pub fn div(&mut self, r: Reg) {
        self.group_f7(6, r);
    }

    pub fn idiv(&mut self, r: Reg) {
        self.group_f7(7, r);
    }

    pub fn sub_rsp_placeholder(&mut self) -> usize {
        self.bytes(&[0x48, 0x81, 0xEC]);
        let pos = self.pos();
        self.bytes(&[0; 4]);
        pos
    }

    /// `xchg rax, r`
    pub fn xchg_rax(&mut self, r: Reg) {
        self.rex(true, 0, r.ext());
        self.byte(0x90 + r.low());
    }

    pub fn cqo(&mut self) {
        self.bytes(&[0x48, 0x99]);
    }

    pub fn shift_cl(&mut self, op: Shift, r: Reg) {
        self.rex(true, 0, r.ext());
        self.byte(0xD3);
        self.modrm_rr(op.digit(), r);
    }

    pub fn shift_ri(&mut self, op: Shift, r: Reg, count: u8) {
        self.rex(true, 0, r.ext());
        if count == 1 {
            self.byte(0xD1);
            self.modrm_rr(op.digit(), r);
        } else {
            self.byte(0xC1);
            self.modrm_rr(op.digit(), r);
            self.byte(count);
        }
    }

    /// `setcc r8`
    pub fn setcc(&mut self, cc: Cond, dst: Reg) {
        self.rex_byte(0, dst.ext(), dst);
        self.bytes(&[0x0F, 0x90 + cc as u8]);
        self.modrm_rr(0, dst);
    }

    pub fn movzx8(&mut self, dst: Reg, src: Reg) {
        self.rex_byte(dst.ext(), src.ext(), src);
        self.bytes(&[0x0F, 0xB6]);
        self.modrm_rr(dst as u8, src);
    }

    /// `rep movsb`
    pub fn rep_movsb(&mut self) {
        self.bytes(&[0xF3, 0xA4]);
    }

    /// `rep stosb`
    pub fn rep_stosb(&mut self) {
        self.bytes(&[0xF3, 0xAA]);
    }

    pub fn cld(&mut self) {
        self.byte(0xFC);
    }

    pub fn syscall(&mut self) {
        self.bytes(&[0x0F, 0x05]);
    }

    pub fn in_al_dx(&mut self) {
        self.byte(0xEC);
    }

    pub fn in_ax_dx(&mut self) {
        self.bytes(&[0x66, 0xED]);
    }

    pub fn in_eax_dx(&mut self) {
        self.byte(0xED);
    }

    pub fn out_dx_al(&mut self) {
        self.byte(0xEE);
    }

    pub fn out_dx_ax(&mut self) {
        self.bytes(&[0x66, 0xEF]);
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

    pub fn sti(&mut self) {
        self.byte(0xFB);
    }

    pub fn pause(&mut self) {
        self.bytes(&[0xF3, 0x90]);
    }

    pub fn int3(&mut self) {
        self.byte(0xCC);
    }

    pub fn iretq(&mut self) {
        self.bytes(&[0x48, 0xCF]);
    }

    /// `retfq`
    pub fn retfq(&mut self) {
        self.bytes(&[0x48, 0xCB]);
    }

    /// `lgdt [r]`
    pub fn lgdt(&mut self, r: Reg) {
        self.rex(false, 0, r.ext());
        self.bytes(&[0x0F, 0x01]);
        self.modrm_mem(2, r, 0);
    }

    /// `lidt [r]`
    pub fn lidt(&mut self, r: Reg) {
        self.rex(false, 0, r.ext());
        self.bytes(&[0x0F, 0x01]);
        self.modrm_mem(3, r, 0);
    }

    /// `invlpg [r]`
    pub fn invlpg(&mut self, r: Reg) {
        self.rex(false, 0, r.ext());
        self.bytes(&[0x0F, 0x01]);
        self.modrm_mem(7, r, 0);
    }

    /// `mov dst, crN`
    pub fn mov_from_cr(&mut self, dst: Reg, cr: u8) {
        self.rex(false, 0, dst.ext());
        self.bytes(&[0x0F, 0x20]);
        self.modrm_rr(cr, dst);
    }

    /// `mov crN, src`
    pub fn mov_to_cr(&mut self, cr: u8, src: Reg) {
        self.rex(false, 0, src.ext());
        self.bytes(&[0x0F, 0x22]);
        self.modrm_rr(cr, src);
    }

    /// `mov sreg, ax`
    pub fn mov_sreg(&mut self, sreg: Sreg, src: Reg) {
        assert!(!matches!(sreg, Sreg::Cs), "ICE: cs can only be loaded with a far jump/return");
        self.rex(false, 0, src.ext());
        self.byte(0x8E);
        self.modrm_rr(sreg as u8, src);
    }

    /// `mov r32, sreg` (les bits hauts de la destination sont mis à zéro)
    pub fn mov_from_sreg(&mut self, dst: Reg, sreg: Sreg) {
        self.rex(false, 0, dst.ext());
        self.byte(0x8C);
        self.modrm_rr(sreg as u8, dst);
    }

    /// `str r32` : sélecteur du TSS courant
    pub fn str_r(&mut self, dst: Reg) {
        self.rex(false, 0, dst.ext());
        self.bytes(&[0x0F, 0x00]);
        self.modrm_rr(1, dst);
    }

    /// `ud2` : instruction invalide garantie (#UD)
    pub fn ud2(&mut self) {
        self.bytes(&[0x0F, 0x0B]);
    }

    /// `ltr r16`
    pub fn ltr(&mut self, src: Reg) {
        self.rex(false, 0, src.ext());
        self.bytes(&[0x0F, 0x00]);
        self.modrm_rr(3, src);
    }

    pub fn rdmsr(&mut self) {
        self.bytes(&[0x0F, 0x32]);
    }

    pub fn wrmsr(&mut self) {
        self.bytes(&[0x0F, 0x30]);
    }

    pub fn rdtsc(&mut self) {
        self.bytes(&[0x0F, 0x31]);
    }
}
