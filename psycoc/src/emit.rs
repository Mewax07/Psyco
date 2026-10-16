use crate::ir::*;
use crate::regalloc::*;
use crate::*;

use Reg::*;

pub struct Emitter<'e> {
    asm: &'e mut Assembler,
    rt: Runtime,
    alloc: &'e Alloc,
    slot_off: Vec<i32>,
    spill_base: i32,
    labels: Vec<Label>,
}

impl<'e> Emitter<'e> {
    pub fn emit(asm: &'e mut Assembler, rt: Runtime, f: &Func, alloc: &'e Alloc, entry: Label) {
        let order = layout(f);
        let nsaved = alloc.saved.len() as i32;
        let mut cur = 8 * nsaved;
        let mut slot_off = Vec::new();
        for &(size, align) in &f.slots {
            let (size, align) = (size as i32, align.min(16) as i32);
            cur = (cur + size + align - 1) / align * align;
            slot_off.push(-cur);
        }
        cur = (cur + 7) / 8 * 8;
        let spill_base = -cur;
        cur += 8 * alloc.nspills as i32;
        let frame = (cur + 15) / 16 * 16 - 8 * nsaved;

        let labels = (0..f.blocks.len()).map(|_| asm.new_label()).collect();
        let mut e = Emitter {
            asm,
            rt,
            alloc,
            slot_off,
            spill_base,
            labels,
        };
        e.asm.bind(entry);
        e.asm.push(Rbp);
        e.asm.mov_rr(Rbp, Rsp);
        for r in &alloc.saved {
            e.asm.push(*r);
        }
        if frame > 0 {
            e.asm.alu_ri(Alu::Sub, Rsp, frame);
        }
        for (k, &b) in order.iter().enumerate() {
            let next = order.get(k + 1).copied();
            e.asm.bind(e.labels[b]);
            for i in &f.blocks[b].insts {
                e.inst(i);
            }
            e.term(&f.blocks[b].term, next);
        }
    }

    // Locations

    fn loc(&self, v: VReg) -> Loc {
        self.alloc.loc[v as usize]
    }

    fn spill_off(&self, s: u32) -> i32 {
        self.spill_base - 8 * (s as i32 + 1)
    }

    fn reg_of(&self, o: Op) -> Option<Reg> {
        match o {
            Op::V(v) => match self.loc(v) {
                Loc::Reg(r) => Some(r),
                _ => None,
            },
            Op::I(_) => None,
        }
    }

    fn mov_imm(&mut self, r: Reg, i: i64) {
        if i == 0 {
            self.asm.zero(r);
        } else {
            self.asm.mov_ri(r, i);
        }
    }

    fn get(&mut self, o: Op, scratch: Reg) -> Reg {
        match o {
            Op::I(i) => {
                self.mov_imm(scratch, i);
                scratch
            }
            Op::V(v) => match self.loc(v) {
                Loc::Reg(r) => r,
                Loc::Spill(s) => {
                    let off = self.spill_off(s);
                    self.asm.load(scratch, Rbp, off);
                    scratch
                }
                Loc::None => {
                    self.asm.zero(scratch);
                    scratch
                }
            },
        }
    }

    fn get_into(&mut self, o: Op, dst: Reg) {
        let r = self.get(o, dst);
        self.asm.mov_rr(dst, r);
    }

    fn dreg(&self, v: VReg, scratch: Reg) -> Reg {
        match self.loc(v) {
            Loc::Reg(r) => r,
            _ => scratch,
        }
    }

    fn set(&mut self, v: VReg, r: Reg) {
        match self.loc(v) {
            Loc::Reg(d) => self.asm.mov_rr(d, r),
            Loc::Spill(s) => {
                let off = self.spill_off(s);
                self.asm.store(Rbp, off, r);
            }
            Loc::None => {}
        }
    }

    fn mem(&mut self, m: Mem, s1: Reg, s2: Reg) -> Addr {
        match m {
            Mem::Base(o, d) => Addr::base(self.get(o, s1), d),
            Mem::Slot(s, d) => Addr::base(Rbp, self.slot_off[s] + d),
            Mem::Param(d) => Addr::base(Rbp, d),
            Mem::Index(b, i, s, d) => {
                let rb = self.get(b, s1);
                let ri = self.get(i, s2);
                Addr::indexed(rb, ri, s, d)
            }
        }
    }

    fn mem_simple(&mut self, m: Mem, s1: Reg, s2: Reg) -> (Reg, i32) {
        let a = self.mem(m, s1, s2);
        if a.index.is_some() {
            self.asm.lea_at(s1, a);
            return (s1, 0);
        }
        (a.base, a.disp)
    }

    fn push_op(&mut self, o: Op) {
        match o {
            Op::I(i) if i32::try_from(i).is_ok() => self.asm.push_imm(i as i32),
            _ => {
                let r = self.get(o, Rax);
                self.asm.push(r);
            }
        }
    }

    fn cmp(&mut self, a: Op, b: Op) {
        if let (Op::V(v), Op::I(i)) = (a, b) {
            if let (Loc::Spill(sl), true) = (self.loc(v), i32::try_from(i).is_ok()) {
                let off = self.spill_off(sl);
                self.asm.cmp_mi(Addr::base(Rbp, off), i as i32);
                return;
            }
        }
        let ra = self.get(a, Rax);
        if let Op::V(v) = b {
            if let Loc::Spill(sl) = self.loc(v) {
                let off = self.spill_off(sl);
                self.asm.alu_rm(Alu::Cmp, ra, Addr::base(Rbp, off));
                return;
            }
        }
        match b {
            Op::I(i) if i32::try_from(i).is_ok() => {
                if i == 0 {
                    self.asm.test_rr(ra, ra);
                } else {
                    self.asm.alu_ri(Alu::Cmp, ra, i as i32);
                }
            }
            _ => {
                let rb = self.get(b, Rcx);
                self.asm.alu_rr(Alu::Cmp, ra, rb);
            }
        }
    }

    fn check_canonical(&mut self, r: Reg, ty: Ty) {
        self.asm.mov_rr(Rcx, r);
        self.asm.extend(Rcx, ty.size as u64, ty.signed);
        self.asm.alu_rr(Alu::Cmp, r, Rcx);
        self.asm.jcc(Cond::NE, self.rt.panic_overflow);
    }

    // Instructions

    fn inst(&mut self, i: &Inst) {
        let rt = self.rt;
        match i {
            Inst::Mov { dst, src } => match (self.loc(*dst), *src) {
                (Loc::Reg(d), s) => self.get_into(s, d),
                (Loc::Spill(_), s) => {
                    let r = self.get(s, Rax);
                    self.set(*dst, r);
                }
                (Loc::None, _) => {}
            },
            Inst::Bin {
                dst,
                op,
                a,
                b,
                ty,
                mode,
            } => self.bin(*dst, *op, *a, *b, *ty, *mode),
            Inst::Neg { dst, a, ty } => {
                let d = self.dreg(*dst, Rax);
                self.get_into(*a, d);
                self.asm.neg(d);
                if ty.size == 8 {
                    self.asm.jcc(Cond::O, rt.panic_overflow);
                } else {
                    self.check_canonical(d, *ty);
                }
                self.set(*dst, d);
            }
            Inst::BitNot { dst, a, ty } => {
                let d = self.dreg(*dst, Rax);
                self.get_into(*a, d);
                self.asm.not(d);
                if !ty.signed {
                    self.asm.extend(d, ty.size as u64, false);
                }
                self.set(*dst, d);
            }
            Inst::Ext { dst, a, ty } => {
                let d = self.dreg(*dst, Rax);
                self.get_into(*a, d);
                self.asm.extend(d, ty.size as u64, ty.signed);
                self.set(*dst, d);
            }
            Inst::Set { dst, cc, a, b } => {
                self.cmp(*a, *b);
                let d = self.dreg(*dst, Rax);
                self.asm.setcc(*cc, d);
                self.asm.movzx8(d, d);
                self.set(*dst, d);
            }
            Inst::Load { dst, mem, ty } => {
                let a = self.mem(*mem, Rax, Rdx);
                let d = self.dreg(*dst, Rax);
                self.asm.load_sized_at(d, a, ty.size as u64, ty.signed);
                self.set(*dst, d);
            }
            Inst::Store { mem, src, size } => {
                let a = self.mem(*mem, Rax, Rdx);
                match *src {
                    Op::I(i) if i32::try_from(i).is_ok() => self.asm.store_imm_at(a, i as i32, *size as u64),
                    _ => {
                        let s = self.get(*src, Rcx);
                        self.asm.store_sized_at(a, s, *size as u64);
                    }
                }
            }
            Inst::Addr { dst, mem } => {
                let a = self.mem(*mem, Rax, Rdx);
                let d = self.dreg(*dst, Rax);
                self.asm.lea_at(d, a);
                self.set(*dst, d);
            }
            Inst::SymAddr { dst, sym } => {
                let d = self.dreg(*dst, Rax);
                match sym {
                    Sym::Data(l) => self.asm.lea_data(d, *l),
                    Sym::Rw(l) => self.asm.lea_rw(d, *l),
                    Sym::Code(l) => self.asm.lea_code(d, *l),
                }
                self.set(*dst, d);
            }
            Inst::Call { dst, target, args } => {
                for a in args {
                    self.push_op(*a);
                }
                match target {
                    CallT::Label(l) => self.asm.call(*l),
                    CallT::Op(o) => {
                        let r = self.get(*o, Rax);
                        self.asm.call_r(r);
                    }
                }
                if !args.is_empty() {
                    self.asm.alu_ri(Alu::Add, Rsp, 8 * args.len() as i32);
                }
                if let Some(d) = dst {
                    self.set(*d, Rax);
                }
            }
            Inst::EfiCall {
                dst,
                callee,
                args,
                ret,
            } => {
                self.push_op(*callee);
                for a in args {
                    self.push_op(*a);
                }
                let n = args.len() as i32;
                let a = &mut *self.asm;
                a.mov_rr(Rbx, Rsp);
                let stack_args = (n - 4).max(0);
                a.alu_ri(Alu::Sub, Rsp, 32 + 8 * stack_args);
                a.alu_ri(Alu::And, Rsp, -16);
                for i in 4..n {
                    a.load(Rax, Rbx, 8 * (n - 1 - i));
                    a.store(Rsp, 32 + 8 * (i - 4), Rax);
                }
                for (i, r) in [Rcx, Rdx, R8, R9].into_iter().enumerate().take(n as usize) {
                    a.load(r, Rbx, 8 * (n - 1 - i as i32));
                }
                a.load(Rax, Rbx, 8 * n);
                a.call_r(Rax);
                a.lea(Rsp, Rbx, 8 * (n + 1));
                if let (Some(d), Some(ty)) = (dst, ret) {
                    self.asm.extend(Rax, ty.size as u64, ty.signed);
                    self.set(*d, Rax);
                }
            }
            Inst::CallRt { routine, arg } => {
                if let Some(a) = arg {
                    self.get_into(*a, Rax);
                }
                self.asm.call(*routine);
            }
            Inst::PrintBytes { slice } => {
                let r = self.get(*slice, Rax);
                self.asm.load(Rdx, r, 8);
                self.asm.load(Rsi, r, 0);
                self.asm.call(rt.sys_write);
            }
            Inst::Copy { dst, src, size } => {
                if small_copy(*size) {
                    let (sb, sd) = self.mem_simple(*src, Rax, Rcx);
                    let (db, dd) = self.mem_simple(*dst, Rdx, Rcx);
                    for (i, c) in chunks(*size) {
                        self.asm.load_sized(Rcx, sb, sd + i, c, false);
                        self.asm.store_sized(db, dd + i, Rcx, c);
                    }
                } else {
                    let (sb, sd) = self.mem_simple(*src, Rax, Rcx);
                    self.asm.lea(Rax, sb, sd);
                    let (db, dd) = self.mem_simple(*dst, Rdx, Rcx);
                    self.asm.lea(Rdi, db, dd);
                    self.asm.mov_rr(Rsi, Rax);
                    self.asm.mov_ri(Rcx, *size as i64);
                    self.asm.rep_movsb();
                }
            }
            Inst::Zero { dst, size } => {
                let (db, dd) = self.mem_simple(*dst, Rax, Rdx);
                if small_copy(*size) {
                    self.asm.zero(Rcx);
                    for (i, c) in chunks(*size) {
                        self.asm.store_sized(db, dd + i, Rcx, c);
                    }
                } else {
                    self.asm.lea(Rdi, db, dd);
                    self.asm.zero(Rax);
                    self.asm.mov_ri(Rcx, *size as i64);
                    self.asm.rep_stosb();
                }
            }
            Inst::Fill {
                dst,
                value,
                count,
                size,
            } => {
                self.get_into(*value, Rax);
                let (db, dd) = self.mem_simple(*dst, Rdx, Rcx);
                self.asm.lea(Rdi, db, dd);
                self.asm.mov_ri(Rcx, *count as i64);
                match size {
                    1 => self.asm.rep_stosb(),
                    2 => self.asm.rep_stosw(),
                    4 => self.memset32_body(),
                    _ => self.asm.rep_stosq(),
                }
            }
            Inst::Check { a, b, cc, trap } => {
                self.cmp(*a, *b);
                self.asm.jcc(*cc, *trap);
            }
            Inst::Intr { kind, dst, args } => self.intr(*kind, *dst, args),
        }
    }

    fn bin(&mut self, dst: VReg, op: BinK, a: Op, b: Op, ty: Ty, mode: Arith) {
        let rt = self.rt;
        let size = ty.size as u64;
        match op {
            BinK::Div | BinK::Mod => {
                self.get_into(a, Rax);
                self.get_into(b, Rcx);
                self.divmod(op == BinK::Mod, ty);
                let r = if op == BinK::Mod { Rdx } else { Rax };
                self.set(dst, r);
                return;
            }
            BinK::Shl | BinK::Shr => {
                let kind = match (op, ty.signed) {
                    (BinK::Shl, _) => Shift::Shl,
                    (_, true) => Shift::Sar,
                    (_, false) => Shift::Shr,
                };
                let d = self.dreg(dst, Rax);
                match b {
                    Op::I(k) => {
                        if mode == Arith::Checked && !(0..size as i64 * 8).contains(&k) {
                            self.asm.jmp(rt.panic_overflow);
                            return;
                        }
                        self.get_into(a, d);
                        self.asm.shift_ri(kind, d, k as u8);
                    }
                    _ => {
                        self.get_into(b, Rcx);
                        if mode == Arith::Checked {
                            self.asm.alu_ri(Alu::Cmp, Rcx, size as i32 * 8);
                            self.asm.jcc(Cond::AE, rt.panic_overflow);
                        }
                        self.get_into(a, d);
                        self.asm.shift_cl(kind, d);
                    }
                }
                if op == BinK::Shl && mode != Arith::Plain {
                    self.asm.extend(d, size, ty.signed);
                }
                self.set(dst, d);
                return;
            }
            _ => {}
        }
        if op == BinK::Mul && mode == Arith::Checked && size == 8 && !ty.signed {
            self.get_into(a, Rax);
            let r = self.get(b, Rcx);
            self.asm.mul(r); // OF = CF = (rdx != 0)
            self.asm.jcc(Cond::O, rt.panic_overflow);
            self.set(dst, Rax);
            return;
        }
        let mut d = self.dreg(dst, Rax);
        if op == BinK::Add && mode == Arith::Plain {
            if let Some(ra) = self.reg_of(a) {
                if ra != d {
                    match b {
                        Op::I(i) if i32::try_from(i).is_ok() => {
                            self.asm.lea(d, ra, i as i32);
                            self.set(dst, d);
                            return;
                        }
                        Op::V(_) => {
                            if let Some(rb) = self.reg_of(b) {
                                if rb != d && rb != Rsp {
                                    self.asm.lea_at(d, Addr::indexed(ra, rb, 0, 0));
                                    self.set(dst, d);
                                    return;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        if op == BinK::Mul && !(mode == Arith::Checked && size == 8 && !ty.signed) {
            if let (Some(ra), Op::I(i)) = (self.reg_of(a), b) {
                if i32::try_from(i).is_ok() {
                    self.asm.imul_ri(d, ra, i as i32);
                    self.finish_bin(dst, d, op, ty, mode);
                    return;
                }
            }
        }
        let (a, b) = if self.reg_of(b) == Some(d) && op.commutative() && self.reg_of(a) != Some(d) {
            (b, a)
        } else {
            (a, b)
        };
        if self.reg_of(b) == Some(d) && self.reg_of(a) != Some(d) {
            d = Rax;
        }
        self.get_into(a, d);
        let alu = match op {
            BinK::Add => Some(Alu::Add),
            BinK::Sub => Some(Alu::Sub),
            BinK::And => Some(Alu::And),
            BinK::Or => Some(Alu::Or),
            BinK::Xor => Some(Alu::Xor),
            _ => None,
        };
        match (b, alu) {
            (Op::I(i), Some(alu)) if i32::try_from(i).is_ok() => self.asm.alu_ri(alu, d, i as i32),
            (Op::I(i), None) if i32::try_from(i).is_ok() => self.asm.imul_ri(d, d, i as i32),
            (Op::V(v), Some(alu)) if matches!(self.loc(v), Loc::Spill(_)) => {
                let Loc::Spill(sl) = self.loc(v) else { unreachable!() };
                let off = self.spill_off(sl);
                self.asm.alu_rm(alu, d, Addr::base(Rbp, off));
            }
            (_, Some(alu)) => {
                let rb = self.get(b, Rcx);
                self.asm.alu_rr(alu, d, rb);
            }
            (_, None) => {
                let rb = self.get(b, Rcx);
                self.asm.imul_rr(d, rb);
            }
        }
        self.finish_bin(dst, d, op, ty, mode);
    }

    fn finish_bin(&mut self, dst: VReg, d: Reg, op: BinK, ty: Ty, mode: Arith) {
        let rt = self.rt;
        let size = ty.size as u64;
        match mode {
            Arith::Checked => {
                if op == BinK::Mul {
                    self.asm.jcc(Cond::O, rt.panic_overflow);
                    if size < 8 {
                        self.check_canonical(d, ty);
                    }
                } else if size == 8 {
                    let cc = if ty.signed { Cond::O } else { Cond::B };
                    self.asm.jcc(cc, rt.panic_overflow);
                } else {
                    self.check_canonical(d, ty);
                }
            }
            Arith::Wrap => {
                if size < 8 {
                    self.asm.extend(d, size, ty.signed);
                }
            }
            Arith::Plain => {}
        }
        self.set(dst, d);
    }

    fn divmod(&mut self, is_mod: bool, ty: Ty) {
        let rt = self.rt;
        let a = &mut *self.asm;
        a.test_rr(Rcx, Rcx);
        a.jcc(Cond::E, rt.panic_div_zero);
        if !ty.signed {
            a.zero(Rdx);
            a.div(Rcx);
            return;
        }
        let normal = a.new_label();
        let done = a.new_label();
        a.alu_ri(Alu::Cmp, Rcx, -1);
        a.jcc(Cond::NE, normal);
        if is_mod {
            a.zero(Rdx); // x % -1 == 0
        } else {
            a.neg(Rax); // x / -1 == -x (MIN / -1 overflows)
            if ty.size == 8 {
                a.jcc(Cond::O, rt.panic_overflow);
            } else {
                self.check_canonical(Rax, ty);
            }
        }
        let a = &mut *self.asm;
        a.jmp(done);
        a.bind(normal);
        a.cqo();
        a.idiv(Rcx);
        a.bind(done);
    }

    fn intr(&mut self, kind: Intr, dst: Option<VReg>, args: &[Op]) {
        let rt = self.rt;
        let result = |e: &mut Self, r: Reg| {
            if let Some(d) = dst {
                e.set(d, r);
            }
        };
        match kind {
            Intr::Out(size) => {
                self.get_into(args[0], Rdx);
                self.get_into(args[1], Rax);
                match size {
                    1 => self.asm.out_dx_al(),
                    2 => self.asm.out_dx_ax(),
                    _ => self.asm.out_dx_eax(),
                }
            }
            Intr::In(size) => {
                self.get_into(args[0], Rdx);
                self.asm.zero(Rax);
                match size {
                    1 => self.asm.in_al_dx(),
                    2 => self.asm.in_ax_dx(),
                    _ => self.asm.in_eax_dx(),
                }
                result(self, Rax);
            }
            Intr::Cli => self.asm.cli(),
            Intr::Sti => self.asm.sti(),
            Intr::Hlt => self.asm.hlt(),
            Intr::Pause => self.asm.pause(),
            Intr::Int3 => self.asm.int3(),
            Intr::Ud2 => self.asm.ud2(),
            Intr::ReadFlags => {
                self.asm.pushfq();
                self.asm.pop(Rax);
                result(self, Rax);
            }
            Intr::ReadSreg(s) => {
                let sreg = [Sreg::Es, Sreg::Cs, Sreg::Ss, Sreg::Ds, Sreg::Fs, Sreg::Gs][s as usize];
                self.asm.zero(Rax);
                self.asm.mov_from_sreg(Rax, sreg);
                result(self, Rax);
            }
            Intr::ReadTr => {
                self.asm.zero(Rax);
                self.asm.str_r(Rax);
                result(self, Rax);
            }
            Intr::ReadCr(cr) => {
                self.asm.mov_from_cr(Rax, cr);
                result(self, Rax);
            }
            Intr::WriteCr(cr) => {
                self.get_into(args[0], Rax);
                self.asm.mov_to_cr(cr, Rax);
            }
            Intr::Rdmsr => {
                self.get_into(args[0], Rcx);
                self.asm.rdmsr();
                self.asm.shift_ri(Shift::Shl, Rdx, 32);
                self.asm.alu_rr(Alu::Or, Rax, Rdx);
                result(self, Rax);
            }
            Intr::Wrmsr => {
                self.get_into(args[0], Rcx);
                self.get_into(args[1], Rax);
                self.asm.mov_rr(Rdx, Rax);
                self.asm.shift_ri(Shift::Shr, Rdx, 32);
                self.asm.wrmsr();
            }
            Intr::Rdtsc => {
                self.asm.rdtsc();
                self.asm.shift_ri(Shift::Shl, Rdx, 32);
                self.asm.alu_rr(Alu::Or, Rax, Rdx);
                result(self, Rax);
            }
            Intr::Invlpg | Intr::Lgdt | Intr::Lidt | Intr::Ltr => {
                self.get_into(args[0], Rax);
                match kind {
                    Intr::Invlpg => self.asm.invlpg(Rax),
                    Intr::Lgdt => self.asm.lgdt(Rax),
                    Intr::Lidt => self.asm.lidt(Rax),
                    _ => self.asm.ltr(Rax),
                }
            }
            Intr::LoadCs => {
                self.get_into(args[0], Rax);
                self.asm.push(Rax);
                let after = self.asm.new_label();
                self.asm.lea_code(Rax, after);
                self.asm.push(Rax);
                self.asm.retfq();
                self.asm.bind(after);
            }
            Intr::LoadDs => {
                self.get_into(args[0], Rax);
                for s in [Sreg::Ds, Sreg::Es, Sreg::Ss, Sreg::Fs, Sreg::Gs] {
                    self.asm.mov_sreg(s, Rax);
                }
            }
            Intr::Memcpy | Intr::Memset | Intr::Memset32 => {
                for a in args {
                    self.push_op(*a);
                }
                self.asm.pop(Rcx);
                match kind {
                    Intr::Memcpy => {
                        self.asm.pop(Rsi);
                        self.asm.pop(Rdi);
                        self.memcpy_body();
                    }
                    Intr::Memset => {
                        self.asm.pop(Rax);
                        self.asm.pop(Rdi);
                        self.memset_body();
                    }
                    _ => {
                        self.asm.pop(Rax);
                        self.asm.pop(Rdi);
                        self.memset32_body();
                    }
                }
            }
            Intr::ContextSwitch => {
                self.get_into(args[0], Rcx);
                self.get_into(args[1], Rax);
                let a = &mut *self.asm;
                let resume = a.new_label();
                a.lea_code(Rdx, resume);
                a.push(Rdx);
                for r in [Rbp, Rbx, R12, R13, R14, R15] {
                    a.push(r);
                }
                a.store(Rcx, 0, Rsp);
                a.mov_rr(Rsp, Rax);
                for r in [R15, R14, R13, R12, Rbx, Rbp] {
                    a.pop(r);
                }
                a.ret();
                a.bind(resume);
            }
            Intr::Syscall => {
                for a in args.iter().rev() {
                    self.push_op(*a);
                }
                for r in [Rax, Rdi, Rsi, Rdx].into_iter().take(args.len()) {
                    self.asm.pop(r);
                }
                self.asm.syscall();
                result(self, Rax);
            }
            Intr::RebaseStatics => {
                self.get_into(args[0], Rax);
                for off in self.asm.abs_offsets() {
                    self.asm.lea_rw(Rcx, RwLabel::Data(off));
                    self.asm.load(Rdx, Rcx, 0);
                    self.asm.alu_rr(Alu::Add, Rdx, Rax);
                    self.asm.store(Rcx, 0, Rdx);
                }
            }
            Intr::SetSyscallStack => {
                self.get_into(args[0], Rax);
                self.asm.store_rw(rt.syscall_kernel_rsp, Rax);
            }
            Intr::EfiImageHandle | Intr::EfiSystemTable => {
                let l = if kind == Intr::EfiImageHandle {
                    rt.efi_image_handle
                } else {
                    rt.efi_system_table
                };
                self.asm.load_rw(Rax, l);
                result(self, Rax);
            }
        }
    }

    fn memcpy_body(&mut self) {
        let a = &mut *self.asm;
        let tail = a.new_label();
        let top = a.new_label();
        a.alu_ri(Alu::Cmp, Rcx, 64);
        a.jcc(Cond::B, tail);
        a.bind(top);
        for k in 0..4 {
            a.movdqu_load(k, Addr::base(Rsi, 16 * k as i32));
        }
        for k in 0..4 {
            a.movdqu_store(Addr::base(Rdi, 16 * k as i32), k);
        }
        a.alu_ri(Alu::Add, Rsi, 64);
        a.alu_ri(Alu::Add, Rdi, 64);
        a.alu_ri(Alu::Sub, Rcx, 64);
        a.alu_ri(Alu::Cmp, Rcx, 64);
        a.jcc(Cond::AE, top);
        a.bind(tail);
        a.mov_rr(Rdx, Rcx);
        a.shift_ri(Shift::Shr, Rcx, 3);
        a.rep_movsq();
        a.mov_rr(Rcx, Rdx);
        a.alu_ri(Alu::And, Rcx, 7);
        a.rep_movsb();
    }

    fn memset_body(&mut self) {
        let a = &mut *self.asm;
        a.movzx8(Rax, Rax);
        a.mov_ri(Rdx, 0x0101_0101_0101_0101);
        a.imul_rr(Rax, Rdx);
        a.mov_rr(Rdx, Rcx);
        a.shift_ri(Shift::Shr, Rcx, 3);
        a.rep_stosq();
        a.mov_rr(Rcx, Rdx);
        a.alu_ri(Alu::And, Rcx, 7);
        a.rep_stosb();
    }

    fn memset32_body(&mut self) {
        let a = &mut *self.asm;
        let tail = a.new_label();
        let top = a.new_label();
        a.movd_xmm_r(0, Rax);
        a.pshufd(0, 0, 0);
        a.alu_ri(Alu::Cmp, Rcx, 16);
        a.jcc(Cond::B, tail);
        a.bind(top);
        for k in 0..4 {
            a.movdqu_store(Addr::base(Rdi, 16 * k), 0);
        }
        a.alu_ri(Alu::Add, Rdi, 64);
        a.alu_ri(Alu::Sub, Rcx, 16);
        a.alu_ri(Alu::Cmp, Rcx, 16);
        a.jcc(Cond::AE, top);
        a.bind(tail);
        a.rep_stosd();
    }

    // Terminators

    fn epilogue(&mut self) {
        let n = self.alloc.saved.len() as i32;
        if n == 0 {
            self.asm.mov_rr(Rsp, Rbp);
        } else {
            self.asm.lea(Rsp, Rbp, -8 * n);
            for r in self.alloc.saved.iter().rev() {
                self.asm.pop(*r);
            }
        }
        self.asm.pop(Rbp);
        self.asm.ret();
    }

    fn term(&mut self, t: &Term, next: Option<BlockId>) {
        let rt = self.rt;
        match t {
            Term::Jmp(b) => {
                if Some(*b) != next {
                    self.asm.jmp(self.labels[*b]);
                }
            }
            Term::Br { cc, a, b, t, f } => {
                self.cmp(*a, *b);
                if Some(*t) == next {
                    self.asm.jcc(cc.negate(), self.labels[*f]);
                } else {
                    self.asm.jcc(*cc, self.labels[*t]);
                    if Some(*f) != next {
                        self.asm.jmp(self.labels[*f]);
                    }
                }
            }
            Term::Ret(v) => {
                if let Some(v) = v {
                    self.get_into(*v, Rax);
                }
                self.epilogue();
            }
            Term::Panic(v) => {
                self.get_into(*v, Rax);
                self.asm.jmp(rt.panic_msg);
            }
            Term::Exit(v) => {
                self.get_into(*v, Rdi);
                self.asm.jmp(rt.sys_exit);
            }
            Term::IretTo(ops) => {
                for o in ops.iter().rev() {
                    self.push_op(*o);
                }
                self.asm.iretq();
            }
            Term::SwitchStack { stack, f } => {
                self.get_into(*stack, Rcx);
                self.get_into(*f, Rax);
                self.asm.mov_rr(Rsp, Rcx);
                self.asm.alu_ri(Alu::And, Rsp, -16);
                self.asm.call_r(Rax);
                self.asm.zero(Rdi);
                self.asm.jmp(rt.sys_exit);
            }
            Term::Unreachable => {}
        }
    }
}
