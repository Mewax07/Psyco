use std::collections::HashMap;

use crate::*;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Linux,
    Uefi,
}

#[derive(Clone, Copy)]
enum VarLoc {
    Frame(i32),
    Indirect(i32),
}

#[derive(Clone)]
struct Var {
    loc: VarLoc,
}

#[derive(Clone, Copy)]
enum StaticLoc {
    Ro(DataLabel),
    Rw(RwLabel),
}

#[derive(Clone, Copy)]
struct Place {
    base: Reg,
    disp: i32,
}

#[derive(Clone, Copy, PartialEq)]
enum OverflowKind {
    Add,
    Sub,
    Mul,
    Signed,
}

pub struct Codegen<'a> {
    asm: Assembler,
    rt: Runtime,
    info: &'a ProgramInfo,
    functions: HashMap<String, Label>,
    strings: HashMap<Vec<u8>, DataLabel>,
    cstrings: HashMap<Vec<u8>, DataLabel>,
    wstrings: HashMap<Vec<u16>, DataLabel>,
    statics: HashMap<String, StaticLoc>,
    scopes: Vec<HashMap<String, Var>>,
    frame_size: i32,
    ret_label: Option<Label>,
    sret: Option<i32>,
    loops: Vec<(Label, Label)>,
}

impl<'a> Codegen<'a> {
    pub fn new(info: &'a ProgramInfo) -> Self {
        let mut asm = Assembler::new();
        let rt = Runtime::new(&mut asm);
        Self {
            asm,
            rt,
            info,
            functions: HashMap::new(),
            strings: HashMap::new(),
            cstrings: HashMap::new(),
            wstrings: HashMap::new(),
            statics: HashMap::new(),
            scopes: Vec::new(),
            frame_size: 0,
            ret_label: None,
            sret: None,
            loops: Vec::new(),
        }
    }

    // Utils

    fn as_imm32(&self, e: &Expr) -> Option<i32> {
        match e.kind {
            ExprKind::Int(n) => i32::try_from(n).ok(),
            ExprKind::Bool(b) => Some(b as i32),
            _ => None,
        }
    }

    fn is_zero(&self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Int(0) | ExprKind::Bool(false) => true,
            ExprKind::ArrayRepeat { value, .. } => self.is_zero(value),
            ExprKind::ArrayLit(items) => items.iter().all(|l| self.is_zero(l)),
            ExprKind::StructLit { fields, .. } => fields.iter().all(|(_, v, _)| self.is_zero(v)),
            _ => false,
        }
    }

    fn always_exits(&self, block: &Block) -> bool {
        match block.stmts.last() {
            Some(Stmt::Return { .. } | Stmt::Break(_) | Stmt::Continue(_)) => true,
            Some(Stmt::If {
                then_block,
                else_block: Some(else_block),
                ..
            }) => self.always_exits(then_block) && self.always_exits(else_block),
            _ => false,
        }
    }

    // Generator

    pub fn generate(mut self, program: &Program, target: Target) -> Vec<u8> {
        for f in &program.functions {
            let label = self.asm.new_label();
            self.functions.insert(f.name.clone(), label);
        }
        self.layout_statics();

        let platform: Box<dyn Platform> = match target {
            Target::Linux => Box::new(LinuxPlatform),
            Target::Uefi => Box::new(UefiPlatform),
        };
        let entry = self.asm.new_label();
        let main = self.functions["main"];
        platform.gen_entry(&mut self.asm, &self.rt, entry, main);
        platform.gen_write(&mut self.asm, &self.rt);
        platform.gen_exit(&mut self.asm, &self.rt);
        self.gen_runtime();

        for f in &program.functions {
            self.gen_function(f);
        }

        match target {
            Target::Linux => ElfBuilder::new().build(&mut self.asm, entry),
            Target::Uefi => PeBuilder::new().build(&mut self.asm, entry),
        }
    }

    fn layout_statics(&mut self) {
        for s in &self.info.statics {
            let size = self.info.size_of(&s.r#type) as usize;
            let align = s.align as usize;
            let loc = match (&s.init, s.mutable) {
                (Some(bytes), false) => StaticLoc::Ro(self.asm.data_aligned(bytes, align)),
                (None, false) => StaticLoc::Ro(self.asm.data_aligned(&vec![0; size], align)),
                (Some(bytes), true) => StaticLoc::Rw(self.asm.rw_data(bytes, align)),
                (None, true) => StaticLoc::Rw(self.asm.bss(size, align)),
            };
            self.statics.insert(s.name.clone(), loc);
        }
    }

    fn gen_runtime(&mut self) {
        use Reg::*;
        let rt = self.rt;
        let a = &mut self.asm;

        // print_str : rax = Str
        a.bind(rt.print_str);
        a.load(Rdx, Rax, 0);
        a.lea(Rsi, Rax, 8);
        a.jmp(rt.sys_write);

        // print_int (signed) / print_uint (unsigned) : rax = value
        a.bind(rt.print_uint);
        a.zero(R8);
        let digits = a.new_label();
        a.jmp(digits);

        a.bind(rt.print_int);
        a.mov_rr(R8, Rax);
        let positive = a.new_label();
        a.test_rr(Rax, Rax);
        a.jcc(Cond::NS, positive);
        a.neg(Rax);
        a.bind(positive);

        a.bind(digits);
        a.push(Rbp);
        a.mov_rr(Rbp, Rsp);
        a.alu_ri(Alu::Sub, Rsp, 32);
        a.mov_rr(Rsi, Rbp);
        a.mov_ri(Rcx, 10);
        let digit = a.new_label();
        a.bind(digit);
        a.zero(Rdx);
        a.div(Rcx);
        a.alu_ri(Alu::Add, Rdx, b'0' as i32);
        a.alu_ri(Alu::Sub, Rsi, 1);
        a.store8(Rsi, Rdx);
        a.test_rr(Rax, Rax);
        a.jcc(Cond::NE, digit);
        let write = a.new_label();
        a.test_rr(R8, R8);
        a.jcc(Cond::NS, write);
        a.alu_ri(Alu::Sub, Rsi, 1);
        a.mov_ri(Rdx, b'-' as i64);
        a.store8(Rsi, Rdx);
        a.bind(write);
        a.mov_rr(Rdx, Rbp);
        a.alu_rr(Alu::Sub, Rdx, Rsi);
        a.call(rt.sys_write);
        a.mov_rr(Rsp, Rbp);
        a.pop(Rbp);
        a.ret();

        a.bind(rt.print_bool);
        let is_false = a.new_label();
        a.test_rr(Rax, Rax);
        a.jcc(Cond::E, is_false);
        a.lea_data(Rax, rt.str_true);
        a.jmp(rt.print_str);
        a.bind(is_false);
        a.lea_data(Rax, rt.str_false);
        a.jmp(rt.print_str);

        a.bind(rt.print_space);
        a.lea_data(Rax, rt.str_space);
        a.jmp(rt.print_str);

        a.bind(rt.print_newline);
        a.lea_data(Rax, rt.str_newline);
        a.jmp(rt.print_str);

        // panics
        let panic = a.new_label();
        a.bind(rt.panic_overflow);
        a.lea_data(Rax, rt.msg_overflow);
        a.jmp(panic);
        a.bind(rt.panic_div_zero);
        a.lea_data(Rax, rt.msg_div_zero);
        a.jmp(panic);
        a.bind(rt.panic_bounds);
        a.lea_data(Rax, rt.msg_bounds);
        a.bind(panic);
        a.alu_ri(Alu::And, Rsp, -16);
        a.call(rt.print_str);
        a.mov_ri(Rdi, 101);
        a.jmp(rt.sys_exit);

        // panic(msg) : "panic: " + msg + "\n"
        a.bind(rt.panic_msg);
        a.alu_ri(Alu::And, Rsp, -16);
        a.push(Rax);
        a.push(Rax);
        a.lea_data(Rax, rt.str_panic);
        a.call(rt.print_str);
        a.pop(Rax);
        a.call(rt.print_str);
        a.call(rt.print_newline);
        a.mov_ri(Rdi, 101);
        a.jmp(rt.sys_exit);
    }

    fn size(&self, t: &Type) -> u64 {
        self.info.size_of(t)
    }

    fn scalar(&self, t: &Type) -> (u64, bool) {
        match t {
            Type::Int(i) => (i.size(), i.signed()),
            Type::Bool => (1, false),
            Type::Enum(_) => {
                let r = self.info.int_repr(t).unwrap();
                (r.size(), r.signed())
            }
            Type::Ref(..) | Type::Raw(..) | Type::Fn(_) | Type::Str => (8, false),
            Type::Unit => (8, false),
            _ => panic!("ICE: {t:?} is not a scalar"),
        }
    }

    fn signed(&self, t: &Type) -> bool {
        self.info.int_repr(t).is_some_and(|i| i.signed())
    }

    fn load_val(&mut self, dst: Reg, p: Place, t: &Type) {
        let (size, signed) = self.scalar(t);
        self.asm.load_sized(dst, p.base, p.disp, size, signed);
    }

    fn store_val(&mut self, p: Place, src: Reg, t: &Type) {
        let (size, _) = self.scalar(t);
        self.asm.store_sized(p.base, p.disp, src, size);
    }

    fn alloc(&mut self, size: u64, align: u64) -> i32 {
        let size = size.max(1) as i32;
        let align = align.max(1) as i32;
        self.frame_size = (self.frame_size + size + align - 1) / align * align;
        -self.frame_size
    }

    fn alloc_ty(&mut self, t: &Type) -> i32 {
        let (size, align) = (self.info.size_of(t), self.info.align_of(t));
        self.alloc(size, align)
    }

    fn lookup(&self, name: &str) -> Option<&Var> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    fn declare(&mut self, name: &str, var: Var) {
        self.scopes
            .last_mut()
            .unwrap()
            .insert(name.to_string(), var);
    }

    fn gen_function(&mut self, f: &Function) {
        use Reg::*;
        let sig = self.info.functions[&f.name].clone();
        let interrupt = has_attr(&f.attrs, "interrupt");

        let body_label = if interrupt {
            let stub = self.functions[&f.name];
            let body = self.asm.new_label();
            self.gen_interrupt_stub(stub, body, sig.params.len() == 2);
            body
        } else {
            self.functions[&f.name]
        };

        let ret = self.asm.new_label();
        self.ret_label = Some(ret);
        self.frame_size = 0;
        self.loops.clear();

        self.asm.bind(body_label);
        self.asm.push(Rbp);
        self.asm.mov_rr(Rbp, Rsp);
        let frame_patch = self.asm.sub_rsp_placeholder();

        let has_sret = sig.ret.is_aggregate();
        let sret_slots = has_sret as i32;
        self.sret = has_sret.then_some(16);
        let n = f.params.len() as i32;
        let mut frame = HashMap::new();
        for (i, (param, r#type)) in f.params.iter().zip(&sig.params).enumerate() {
            let off = 16 + 8 * (sret_slots + n - 1 - i as i32);
            let loc = if r#type.is_aggregate() {
                VarLoc::Indirect(off)
            } else {
                VarLoc::Frame(off)
            };
            frame.insert(param.name.clone(), Var { loc });
        }
        self.scopes = vec![frame];

        self.gen_block(&f.body);

        self.asm.bind(ret);
        self.asm.mov_rr(Rsp, Rbp);
        self.asm.pop(Rbp);
        self.asm.ret();

        let frame = (self.frame_size + 15) / 16 * 16;
        self.asm.patch_i32(frame_patch, frame);
        self.scopes.clear();
    }

    fn gen_interrupt_stub(&mut self, stub: Label, body: Label, has_code: bool) {
        use Reg::*;
        const SAVED: [Reg; 15] = [
            Rax, Rcx, Rdx, Rbx, Rbp, Rsi, Rdi, R8, R9, R10, R11, R12, R13, R14, R15,
        ];
        let a = &mut self.asm;
        a.bind(stub);
        for r in SAVED {
            a.push(r);
        }
        a.cld();
        let saved_bytes = 8 * SAVED.len() as i32;
        if has_code {
            // [regs][code][rip cs rflags rsp ss]
            a.lea(Rax, Rsp, saved_bytes + 8);
            a.push(Rax); // frame
            a.load(Rax, Rsp, saved_bytes + 8);
            a.push(Rax); // code
            a.call(body);
            a.alu_ri(Alu::Add, Rsp, 16);
        } else {
            a.lea(Rax, Rsp, saved_bytes);
            a.push(Rax);
            a.call(body);
            a.alu_ri(Alu::Add, Rsp, 8);
        }
        for r in SAVED.iter().rev() {
            a.pop(*r);
        }
        if has_code {
            a.alu_ri(Alu::Add, Rsp, 8);
        }
        a.iretq();
    }

    fn gen_block(&mut self, block: &Block) {
        self.scopes.push(HashMap::new());
        for stmt in &block.stmts {
            self.gen_stmt(stmt);
        }
        self.scopes.pop();
    }

    fn gen_stmt(&mut self, stmt: &Stmt) {
        use Reg::*;
        match stmt {
            Stmt::Let {
                name,
                value,
                r#type,
                ..
            } => {
                let r#type = r#type.clone().expect("ICE: let not typed");
                let off = self.alloc_ty(&r#type);
                let dst = Place {
                    base: Rbp,
                    disp: off,
                };
                match value {
                    Some(v) => self.gen_init(dst, v, &r#type),
                    None => self.zero_fill(dst, self.size(&r#type)),
                }
                self.declare(
                    name,
                    Var {
                        loc: VarLoc::Frame(off),
                    },
                );
            }
            Stmt::Assign {
                target, op, value, ..
            } => self.gen_assign(target, *op, value),
            Stmt::Expr(e) => self.gen_expr(e),
            Stmt::Return { value, .. } => {
                if let Some(e) = value {
                    match self.sret {
                        Some(sret_off) => {
                            self.gen_expr(e);
                            self.asm.load(Rdi, Rbp, sret_off);
                            let size = self.size(e.r#type());
                            self.copy_to(Place { base: Rdi, disp: 0 }, Rax, size);
                            self.asm.load(Rax, Rbp, sret_off);
                        }
                        None => self.gen_expr(e),
                    }
                }
                let ret = self.ret_label.expect("ICE: return outside a function");
                self.asm.jmp(ret);
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                let else_label = self.asm.new_label();
                self.gen_branch(cond, false, else_label);
                self.gen_block(then_block);
                match else_block {
                    None => self.asm.bind(else_label),
                    Some(block) => {
                        let end_label = self.asm.new_label();
                        if !self.always_exits(then_block) {
                            self.asm.jmp(end_label);
                        }
                        self.asm.bind(else_label);
                        self.gen_block(block);
                        self.asm.bind(end_label);
                    }
                }
            }
            Stmt::While { cond, body, .. } => {
                let body_label = self.asm.new_label();
                let cond_label = self.asm.new_label();
                let end_label = self.asm.new_label();
                self.asm.jmp(cond_label);
                self.asm.bind(body_label);
                self.loops.push((cond_label, end_label));
                self.gen_block(body);
                self.loops.pop();
                self.asm.bind(cond_label);
                self.gen_branch(cond, true, body_label);
                self.asm.bind(end_label);
            }
            Stmt::Loop { body, .. } => {
                let top = self.asm.new_label();
                let end = self.asm.new_label();
                self.asm.bind(top);
                self.loops.push((top, end));
                self.gen_block(body);
                self.loops.pop();
                self.asm.jmp(top);
                self.asm.bind(end);
            }
            Stmt::For {
                var,
                start,
                end,
                body,
                ..
            } => self.gen_for(var, start, end, body),
            Stmt::Break(_) => {
                let (_, brk) = *self.loops.last().expect("ICE: break outside loop");
                self.asm.jmp(brk);
            }
            Stmt::Continue(_) => {
                let (cont, _) = *self.loops.last().expect("ICE: continue outside loop");
                self.asm.jmp(cont);
            }
            Stmt::Match {
                scrutinee, arms, ..
            } => {
                self.gen_expr(scrutinee);
                let end = self.asm.new_label();
                let mut labels = Vec::new();
                let mut wildcard = None;
                for arm in arms {
                    let label = self.asm.new_label();
                    match &arm.patterns {
                        None => wildcard = Some(label),
                        Some(pats) => {
                            for p in pats {
                                let ExprKind::Int(v) = p.kind else {
                                    panic!("ICE: match pattern not folded")
                                };
                                self.cmp_rax_const(v as i64);
                                self.asm.jcc(Cond::E, label);
                            }
                        }
                    }
                    labels.push(label);
                }
                self.asm.jmp(wildcard.unwrap_or(end));
                for (arm, label) in arms.iter().zip(labels) {
                    self.asm.bind(label);
                    self.gen_block(&arm.body);
                    if !self.always_exits(&arm.body) {
                        self.asm.jmp(end);
                    }
                }
                self.asm.bind(end);
            }
            Stmt::Block(b) => self.gen_block(b),
        }
    }

    fn cmp_rax_const(&mut self, v: i64) {
        match i32::try_from(v) {
            Ok(0) => self.asm.test_rr(Reg::Rax, Reg::Rax),
            Ok(imm) => self.asm.alu_ri(Alu::Cmp, Reg::Rax, imm),
            Err(_) => {
                self.asm.mov_ri(Reg::Rcx, v);
                self.asm.alu_rr(Alu::Cmp, Reg::Rax, Reg::Rcx);
            }
        }
    }

    fn gen_for(&mut self, var: &str, start: &Expr, end: &Expr, body: &Block) {
        use Reg::*;
        let r#type = start.r#type().clone();
        let signed = self.signed(&r#type);
        let var_off = self.alloc(8, 8);
        let end_off = self.alloc(8, 8);
        self.gen_expr(start);
        self.asm.store(Rbp, var_off, Rax);
        self.gen_expr(end);
        self.asm.store(Rbp, end_off, Rax);

        let body_label = self.asm.new_label();
        let cont_label = self.asm.new_label();
        let cond_label = self.asm.new_label();
        let end_label = self.asm.new_label();
        self.asm.jmp(cond_label);

        self.asm.bind(body_label);
        self.scopes.push(HashMap::new());
        self.declare(
            var,
            Var {
                loc: VarLoc::Frame(var_off),
            },
        );
        self.loops.push((cont_label, end_label));
        self.gen_block(body);
        self.loops.pop();
        self.scopes.pop();

        self.asm.bind(cont_label);
        self.asm.load(Rax, Rbp, var_off);
        self.asm.alu_ri(Alu::Add, Rax, 1);
        self.asm.store(Rbp, var_off, Rax);

        self.asm.bind(cond_label);
        self.asm.load(Rax, Rbp, var_off);
        self.asm.load(Rcx, Rbp, end_off);
        self.asm.alu_rr(Alu::Cmp, Rax, Rcx);
        self.asm
            .jcc(if signed { Cond::L } else { Cond::B }, body_label);
        self.asm.bind(end_label);
    }

    fn gen_assign(&mut self, target: &Expr, op: Option<BinOp>, value: &Expr) {
        use Reg::*;
        let r#type = target.r#type().clone();
        match op {
            None if r#type.is_aggregate() => {
                self.gen_addr(target);
                self.asm.push(Rax);
                self.gen_expr(value);
                self.asm.pop(Rdi);
                let size = self.size(&r#type);
                self.copy_to(Place { base: Rdi, disp: 0 }, Rax, size);
            }
            None => {
                if let Some(off) = self.frame_var(target) {
                    self.gen_expr(value);
                    self.store_val(
                        Place {
                            base: Rbp,
                            disp: off,
                        },
                        Rax,
                        &r#type,
                    );
                } else {
                    self.gen_addr(target);
                    self.asm.push(Rax);
                    self.gen_expr(value);
                    self.asm.pop(Rcx);
                    self.store_val(Place { base: Rcx, disp: 0 }, Rax, &r#type);
                }
            }
            Some(op) => {
                self.gen_addr(target);
                self.asm.push(Rax);
                self.load_val(Rax, Place { base: Rax, disp: 0 }, &r#type);
                self.asm.push(Rax);
                self.gen_expr(value);
                self.asm.pop(Rcx);
                self.emit_binop(op, &r#type);
                self.asm.pop(Rcx);
                self.store_val(Place { base: Rcx, disp: 0 }, Rax, &r#type);
            }
        }
    }

    fn frame_var(&self, e: &Expr) -> Option<i32> {
        if let ExprKind::Var(name) = &e.kind {
            if let Some(Var {
                loc: VarLoc::Frame(off),
                ..
            }) = self.lookup(name)
            {
                return Some(*off);
            }
        }
        None
    }

    fn copy_to(&mut self, dst: Place, src: Reg, size: u64) {
        use Reg::*;
        if size <= 64 && size % 8 == 0 {
            for i in (0..size as i32).step_by(8) {
                self.asm.load(Rcx, src, i);
                self.asm.store(dst.base, dst.disp + i, Rcx);
            }
            return;
        }
        if dst.base != Rdi || dst.disp != 0 {
            if src == Rdi {
                self.asm.mov_rr(Rsi, Rdi);
                self.asm.lea(Rdi, dst.base, dst.disp);
            } else {
                self.asm.lea(Rdi, dst.base, dst.disp);
                self.asm.mov_rr(Rsi, src);
            }
        } else {
            self.asm.mov_rr(Rsi, src);
        }
        self.asm.mov_ri(Rcx, size as i64);
        self.asm.rep_movsb();
    }

    fn zero_fill(&mut self, dst: Place, size: u64) {
        use Reg::*;
        if size <= 64 && size % 8 == 0 {
            self.asm.zero(Rcx);
            for i in (0..size as i32).step_by(8) {
                self.asm.store(dst.base, dst.disp + i, Rcx);
            }
            return;
        }
        self.asm.lea(Rdi, dst.base, dst.disp);
        self.asm.zero(Rax);
        self.asm.mov_ri(Rcx, size as i64);
        self.asm.rep_stosb();
    }

    fn gen_init(&mut self, dst: Place, e: &Expr, r#type: &Type) {
        use Reg::*;
        debug_assert!(dst.base == Rbp);
        match &e.kind {
            ExprKind::StructLit { fields, .. } => {
                let Type::Struct(sid) = r#type else {
                    unreachable!()
                };
                let sid = *sid;
                for (name, value, _) in fields {
                    let fl = self.info.field(sid, name).clone();
                    let p = Place {
                        base: Rbp,
                        disp: dst.disp + fl.offset as i32,
                    };
                    self.gen_init(p, value, &fl.r#type);
                }
            }
            ExprKind::ArrayLit(items) => {
                let Type::Array(elem, _) = r#type else {
                    unreachable!()
                };
                let esize = self.size(elem) as i32;
                for (i, item) in items.iter().enumerate() {
                    let p = Place {
                        base: Rbp,
                        disp: dst.disp + i as i32 * esize,
                    };
                    self.gen_init(p, item, elem);
                }
            }
            ExprKind::ArrayRepeat { value, .. } => {
                let Type::Array(elem, n) = r#type else {
                    unreachable!()
                };
                let (elem, n) = ((**elem).clone(), *n);
                let esize = self.size(&elem);
                if self.is_zero(value) {
                    self.zero_fill(dst, esize * n);
                } else if elem.is_aggregate() {
                    let tmp = self.alloc_ty(&elem);
                    let tmp_place = Place {
                        base: Rbp,
                        disp: tmp,
                    };
                    self.gen_init(tmp_place, value, &elem);
                    for i in 0..n as i32 {
                        self.asm.lea(Rax, Rbp, tmp);
                        let p = Place {
                            base: Rbp,
                            disp: dst.disp + i * esize as i32,
                        };
                        self.copy_to(p, Rax, esize);
                    }
                } else {
                    self.gen_expr(value);
                    if n <= 16 {
                        for i in 0..n as i32 {
                            let p = Place {
                                base: Rbp,
                                disp: dst.disp + i * esize as i32,
                            };
                            self.store_val(p, Rax, &elem);
                        }
                    } else {
                        let top = self.asm.new_label();
                        self.asm.lea(Rdi, Rbp, dst.disp);
                        self.asm.mov_ri(Rcx, n as i64);
                        self.asm.bind(top);
                        self.store_val(Place { base: Rdi, disp: 0 }, Rax, &elem);
                        self.asm.alu_ri(Alu::Add, Rdi, esize as i32);
                        self.asm.alu_ri(Alu::Sub, Rcx, 1);
                        self.asm.jcc(Cond::NE, top);
                    }
                }
            }
            _ if r#type.is_aggregate() => {
                self.gen_expr(e);
                let size = self.size(r#type);
                self.copy_to(dst, Rax, size);
            }
            _ => {
                self.gen_expr(e);
                self.store_val(dst, Rax, r#type);
            }
        }
    }

    fn gen_place(&mut self, e: &Expr) -> Place {
        use Reg::*;
        match &e.kind {
            ExprKind::Var(name) => {
                if let Some(var) = self.lookup(name) {
                    match var.loc {
                        VarLoc::Frame(off) => {
                            return Place {
                                base: Rbp,
                                disp: off,
                            };
                        }
                        VarLoc::Indirect(off) => {
                            self.asm.load(Rax, Rbp, off);
                            return Place { base: Rax, disp: 0 };
                        }
                    }
                }
                if let Some(loc) = self.statics.get(name).copied() {
                    match loc {
                        StaticLoc::Ro(d) => self.asm.lea_data(Rax, d),
                        StaticLoc::Rw(d) => self.asm.lea_rw(Rax, d),
                    }
                    return Place { base: Rax, disp: 0 };
                }
                self.temp_place(e)
            }
            ExprKind::Field { base, name } => {
                let Type::Struct(sid) = base.r#type() else {
                    panic!("ICE: field access on non-struct")
                };
                let off = self.info.field(*sid, name).offset as i32;
                let p = self.gen_place(base);
                Place {
                    base: p.base,
                    disp: p.disp + off,
                }
            }
            ExprKind::Index { base, index } => self.gen_index_place(base, index),
            ExprKind::Unary {
                op: UnaryOp::Deref,
                operand,
            } => {
                self.gen_expr(operand);
                Place { base: Rax, disp: 0 }
            }
            _ => self.temp_place(e),
        }
    }

    fn temp_place(&mut self, e: &Expr) -> Place {
        use Reg::*;
        if e.r#type().is_aggregate() {
            self.gen_expr(e);
            Place { base: Rax, disp: 0 }
        } else {
            let r#type = e.r#type().clone();
            let off = self.alloc_ty(&r#type);
            self.gen_expr(e);
            let p = Place {
                base: Rbp,
                disp: off,
            };
            self.store_val(p, Rax, &r#type);
            p
        }
    }

    fn gen_index_place(&mut self, base: &Expr, index: &Expr) -> Place {
        use Reg::*;
        match base.r#type().clone() {
            Type::Array(elem, n) => {
                let esize = self.size(&elem) as i64;
                if let ExprKind::Int(i) = index.kind {
                    let p = self.gen_place(base);
                    return Place {
                        base: p.base,
                        disp: p.disp + (i as i64 * esize) as i32,
                    };
                }
                self.gen_addr(base);
                self.asm.push(Rax);
                self.gen_expr(index);
                self.bounds_check_imm(n);
                self.scale(Rax, esize);
                self.asm.pop(Rcx);
                self.asm.alu_rr(Alu::Add, Rax, Rcx);
                Place { base: Rax, disp: 0 }
            }
            Type::Slice(elem, _) => {
                let esize = self.size(&elem) as i64;
                self.gen_addr(base);
                self.asm.push(Rax);
                self.gen_expr(index);
                self.asm.pop(Rcx);
                self.asm.load(Rdx, Rcx, 8);
                self.asm.alu_rr(Alu::Cmp, Rax, Rdx);
                self.asm.jcc(Cond::AE, self.rt.panic_bounds);
                self.scale(Rax, esize);
                self.asm.load(Rcx, Rcx, 0);
                self.asm.alu_rr(Alu::Add, Rax, Rcx);
                Place { base: Rax, disp: 0 }
            }
            other => panic!("ICE: index on {other:?}"),
        }
    }

    fn bounds_check_imm(&mut self, n: u64) {
        match i32::try_from(n) {
            Ok(imm) => self.asm.alu_ri(Alu::Cmp, Reg::Rax, imm),
            Err(_) => {
                self.asm.mov_ri(Reg::Rdx, n as i64);
                self.asm.alu_rr(Alu::Cmp, Reg::Rax, Reg::Rdx);
            }
        }
        self.asm.jcc(Cond::AE, self.rt.panic_bounds);
    }

    fn scale(&mut self, r: Reg, k: i64) {
        if k == 1 {
            return;
        }
        if k > 0 && (k as u64).is_power_of_two() {
            self.asm.shift_ri(Shift::Shl, r, k.trailing_zeros() as u8);
        } else {
            self.asm.imul_ri(r, r, k as i32);
        }
    }

    fn gen_addr(&mut self, e: &Expr) {
        let p = self.gen_place(e);
        if p.base != Reg::Rax || p.disp != 0 {
            self.asm.lea(Reg::Rax, p.base, p.disp);
        }
    }

    fn intern_str(&mut self, s: &[u8]) -> DataLabel {
        if let Some(&l) = self.strings.get(s) {
            return l;
        }
        let l = self.asm.data_str(s);
        self.strings.insert(s.to_vec(), l);
        l
    }

    fn gen_expr(&mut self, e: &Expr) {
        use Reg::*;
        match &e.kind {
            ExprKind::Int(0) | ExprKind::Bool(false) => self.asm.zero(Rax),
            ExprKind::Int(v) => self.asm.mov_ri(Rax, *v as i64),
            ExprKind::Bool(true) => self.asm.mov_ri(Rax, 1),
            ExprKind::Str(s) => {
                let l = self.intern_str(s);
                self.asm.lea_data(Rax, l);
            }
            ExprKind::CStr(s) => {
                let l = match self.cstrings.get(s) {
                    Some(&l) => l,
                    None => {
                        let mut bytes = s.clone();
                        bytes.push(0);
                        let l = self.asm.data_aligned(&bytes, 1);
                        self.cstrings.insert(s.clone(), l);
                        l
                    }
                };
                self.asm.lea_data(Rax, l);
            }
            ExprKind::WStr(s) => {
                let l = match self.wstrings.get(s) {
                    Some(&l) => l,
                    None => {
                        let mut bytes: Vec<u8> = s.iter().flat_map(|c| c.to_le_bytes()).collect();
                        bytes.extend_from_slice(&[0, 0]);
                        let l = self.asm.data_aligned(&bytes, 2);
                        self.wstrings.insert(s.clone(), l);
                        l
                    }
                };
                self.asm.lea_data(Rax, l);
            }
            ExprKind::Var(name)
                if self.lookup(name).is_none() && !self.statics.contains_key(name) =>
            {
                let label = self.functions[name];
                self.asm.lea_code(Rax, label);
            }
            ExprKind::Var(_) | ExprKind::Field { .. } | ExprKind::Index { .. } => {
                let p = self.gen_place(e);
                if e.r#type().is_aggregate() {
                    if p.base != Rax || p.disp != 0 {
                        self.asm.lea(Rax, p.base, p.disp);
                    }
                } else {
                    let r#type = e.r#type().clone();
                    self.load_val(Rax, p, &r#type);
                }
            }
            ExprKind::Unary { op, operand } => self.gen_unary(*op, operand, e.r#type()),
            ExprKind::Binary { op, lhs, rhs } => self.gen_binary(*op, lhs, rhs, e.r#type()),
            ExprKind::Cast { expr, .. } => self.gen_cast(expr, e.r#type()),
            ExprKind::Call {
                callee,
                args,
                target,
            } => self.gen_call(callee, args, target, e.r#type()),
            ExprKind::StructLit { .. } | ExprKind::ArrayLit(_) | ExprKind::ArrayRepeat { .. } => {
                let r#type = e.r#type().clone();
                let off = self.alloc_ty(&r#type);
                self.gen_init(
                    Place {
                        base: Rbp,
                        disp: off,
                    },
                    e,
                    &r#type,
                );
                self.asm.lea(Rax, Rbp, off);
            }
            ExprKind::Range { base, start, end } => {
                self.gen_range(base, start.as_deref(), end.as_deref())
            }
            ExprKind::Unsize(inner) => {
                let Type::Ref(arr, _) = inner.r#type() else {
                    unreachable!()
                };
                let Type::Array(_, n) = **arr else {
                    unreachable!()
                };
                self.gen_expr(inner);
                let off = self.alloc(16, 8);
                self.asm.store(Rbp, off, Rax);
                self.asm.mov_ri(Rax, n as i64);
                self.asm.store(Rbp, off + 8, Rax);
                self.asm.lea(Rax, Rbp, off);
            }
            ExprKind::TypedInt(..) | ExprKind::Path(..) | ExprKind::Sizeof(_) => {
                panic!(
                    "ICE: {:?} should have been rewritten by the type checker",
                    e.kind
                )
            }
        }
    }

    fn gen_unary(&mut self, op: UnaryOp, operand: &Expr, r#type: &Type) {
        use Reg::*;
        match op {
            UnaryOp::Neg => {
                self.gen_expr(operand);
                self.asm.neg(Rax);
                self.check_overflow(r#type, OverflowKind::Signed);
            }
            UnaryOp::Not => {
                self.gen_expr(operand);
                self.asm.alu_ri(Alu::Xor, Rax, 1);
            }
            UnaryOp::BitNot => {
                self.gen_expr(operand);
                self.asm.not(Rax);
                let (size, signed) = self.scalar(r#type);
                if !signed {
                    self.asm.extend(Rax, size, false);
                }
            }
            UnaryOp::Ref(_) => self.gen_addr(operand),
            UnaryOp::Deref => {
                self.gen_expr(operand);
                if !r#type.is_aggregate() {
                    self.load_val(Rax, Place { base: Rax, disp: 0 }, r#type);
                }
            }
        }
    }

    fn gen_cast(&mut self, expr: &Expr, to: &Type) {
        use Reg::*;
        self.gen_expr(expr);
        let from = expr.r#type();
        match to {
            Type::Int(t) => {
                if matches!(from, Type::Int(_) | Type::Enum(_) | Type::Bool) {
                    let (fsize, fsigned) = self.scalar(from);
                    if fsize > t.size()
                        || (fsize == t.size() && fsigned != t.signed())
                        || (fsize < t.size() && fsigned && !t.signed())
                    {
                        self.asm.extend(Rax, t.size(), t.signed());
                    }
                }
            }
            Type::Raw(..) if *from == Type::Str => {
                self.asm.alu_ri(Alu::Add, Rax, 8);
            }
            _ => {}
        }
    }

    fn gen_range(&mut self, base: &Expr, start: Option<&Expr>, end: Option<&Expr>) {
        use Reg::*;
        let (elem, array_len) = match base.r#type() {
            Type::Array(e, n) => ((**e).clone(), Some(*n)),
            Type::Slice(e, _) => ((**e).clone(), None),
            _ => unreachable!(),
        };
        let esize = self.size(&elem) as i64;
        // [ptr][len]
        self.gen_addr(base);
        match array_len {
            Some(n) => {
                self.asm.push(Rax);
                self.asm.mov_ri(Rax, n as i64);
                self.asm.push(Rax);
            }
            None => {
                self.asm.load(Rcx, Rax, 0);
                self.asm.load(Rdx, Rax, 8);
                self.asm.push(Rcx);
                self.asm.push(Rdx);
            }
        }
        match start {
            Some(s) => self.gen_expr(s),
            None => self.asm.zero(Rax),
        }
        self.asm.push(Rax);
        match end {
            Some(e) => self.gen_expr(e),
            None => self.asm.load(Rax, Rsp, 8),
        }
        self.asm.pop(Rcx);
        self.asm.pop(Rdx);
        self.asm.alu_rr(Alu::Cmp, Rax, Rdx);
        self.asm.jcc(Cond::A, self.rt.panic_bounds);
        self.asm.alu_rr(Alu::Cmp, Rcx, Rax);
        self.asm.jcc(Cond::A, self.rt.panic_bounds);
        self.asm.alu_rr(Alu::Sub, Rax, Rcx);
        self.asm.pop(Rdx);
        self.scale(Rcx, esize);
        self.asm.alu_rr(Alu::Add, Rdx, Rcx);
        let off = self.alloc(16, 8);
        self.asm.store(Rbp, off, Rdx);
        self.asm.store(Rbp, off + 8, Rax);
        self.asm.lea(Rax, Rbp, off);
    }

    fn gen_branch(&mut self, expr: &Expr, jump_if: bool, target: Label) {
        match &expr.kind {
            ExprKind::Bool(b) => {
                if *b == jump_if {
                    self.asm.jmp(target);
                }
            }
            ExprKind::Unary {
                op: UnaryOp::Not,
                operand,
            } => self.gen_branch(operand, !jump_if, target),
            ExprKind::Binary {
                op: BinOp::And,
                lhs,
                rhs,
            } => {
                if jump_if {
                    let skip = self.asm.new_label();
                    self.gen_branch(lhs, false, skip);
                    self.gen_branch(rhs, true, target);
                    self.asm.bind(skip);
                } else {
                    self.gen_branch(lhs, false, target);
                    self.gen_branch(rhs, false, target);
                }
            }
            ExprKind::Binary {
                op: BinOp::Or,
                lhs,
                rhs,
            } => {
                if jump_if {
                    self.gen_branch(lhs, true, target);
                    self.gen_branch(rhs, true, target);
                } else {
                    let skip = self.asm.new_label();
                    self.gen_branch(lhs, true, skip);
                    self.gen_branch(rhs, false, target);
                    self.asm.bind(skip);
                }
            }
            ExprKind::Binary { op, lhs, rhs } if op.is_comparison() => {
                let cc = self.gen_cmp(*op, lhs, rhs);
                self.asm.jcc(if jump_if { cc } else { cc.negate() }, target);
            }
            _ => {
                self.gen_expr(expr);
                self.asm.test_rr(Reg::Rax, Reg::Rax);
                self.asm
                    .jcc(if jump_if { Cond::NE } else { Cond::E }, target);
            }
        }
    }

    fn cmp_cond(&self, op: BinOp, operand_ty: &Type) -> Cond {
        let signed = self.signed(operand_ty);
        match (op, signed) {
            (BinOp::Eq, _) => Cond::E,
            (BinOp::Ne, _) => Cond::NE,
            (BinOp::Lt, true) => Cond::L,
            (BinOp::Le, true) => Cond::LE,
            (BinOp::Gt, true) => Cond::G,
            (BinOp::Ge, true) => Cond::GE,
            (BinOp::Lt, false) => Cond::B,
            (BinOp::Le, false) => Cond::BE,
            (BinOp::Gt, false) => Cond::A,
            (BinOp::Ge, false) => Cond::AE,
            _ => unreachable!(),
        }
    }

    fn gen_cmp(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr) -> Cond {
        let cc = self.cmp_cond(op, lhs.r#type());
        if let Some(imm) = self.as_imm32(rhs) {
            self.gen_expr(lhs);
            self.cmp_imm(Reg::Rax, imm);
            return cc;
        }
        if let Some(imm) = self.as_imm32(lhs) {
            self.gen_expr(rhs);
            self.cmp_imm(Reg::Rax, imm);
            return cc.swap();
        }
        self.gen_operands(lhs, rhs);
        self.asm.alu_rr(Alu::Cmp, Reg::Rcx, Reg::Rax);
        cc
    }

    fn cmp_imm(&mut self, r: Reg, imm: i32) {
        if imm == 0 {
            self.asm.test_rr(r, r);
        } else {
            self.asm.alu_ri(Alu::Cmp, r, imm);
        }
    }

    fn gen_operands(&mut self, lhs: &Expr, rhs: &Expr) {
        self.gen_expr(lhs);
        self.asm.push(Reg::Rax);
        self.gen_expr(rhs);
        self.asm.pop(Reg::Rcx);
    }

    fn gen_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr, result_ty: &Type) {
        use Reg::*;
        match op {
            BinOp::And | BinOp::Or => {
                let end_label = self.asm.new_label();
                self.gen_expr(lhs);
                self.asm.test_rr(Rax, Rax);
                let cc = if op == BinOp::And { Cond::E } else { Cond::NE };
                self.asm.jcc(cc, end_label);
                self.gen_expr(rhs);
                self.asm.bind(end_label);
            }
            _ if op.is_comparison() => {
                let cc = self.gen_cmp(op, lhs, rhs);
                self.asm.setcc(cc, Rax);
                self.asm.movzx8(Rax, Rax);
            }
            _ => {
                let r#type = result_ty.clone();
                if self.gen_binary_imm(op, lhs, rhs, &r#type) {
                    return;
                }
                self.gen_operands(lhs, rhs);
                self.emit_binop(op, &r#type);
            }
        }
    }

    fn gen_binary_imm(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr, r#type: &Type) -> bool {
        use Reg::*;
        let (size, signed) = self.scalar(r#type);
        let wide_unsigned = size == 8 && !signed;
        let commutative_imm = || {
            self.as_imm32(rhs)
                .map(|i| (lhs, i))
                .or_else(|| self.as_imm32(lhs).map(|i| (rhs, i)))
        };
        match op {
            BinOp::Add | BinOp::AddWrap | BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                let Some((other, imm)) = commutative_imm() else {
                    return false;
                };
                self.gen_expr(other);
                let alu = match op {
                    BinOp::Add | BinOp::AddWrap => Alu::Add,
                    BinOp::BitAnd => Alu::And,
                    BinOp::BitOr => Alu::Or,
                    _ => Alu::Xor,
                };
                self.asm.alu_ri(alu, Rax, imm);
                match op {
                    BinOp::Add => self.check_overflow(r#type, OverflowKind::Add),
                    BinOp::AddWrap => self.asm.extend(Rax, size, signed),
                    _ => {}
                }
                true
            }
            BinOp::Sub | BinOp::SubWrap => {
                let Some(imm) = self.as_imm32(rhs) else {
                    return false;
                };
                self.gen_expr(lhs);
                self.asm.alu_ri(Alu::Sub, Rax, imm);
                if op == BinOp::Sub {
                    self.check_overflow(r#type, OverflowKind::Sub);
                } else {
                    self.asm.extend(Rax, size, signed);
                }
                true
            }
            BinOp::Mul | BinOp::MulWrap if !(wide_unsigned && op == BinOp::Mul) => {
                let Some((other, imm)) = commutative_imm() else {
                    return false;
                };
                self.gen_expr(other);
                self.asm.imul_ri(Rax, Rax, imm);
                if op == BinOp::Mul {
                    self.check_overflow(r#type, OverflowKind::Mul);
                } else {
                    self.asm.extend(Rax, size, signed);
                }
                true
            }
            BinOp::Shl | BinOp::Shr => {
                let ExprKind::Int(count) = rhs.kind else {
                    return false;
                };
                self.gen_expr(lhs);
                let kind = match (op, signed) {
                    (BinOp::Shl, _) => Shift::Shl,
                    (_, true) => Shift::Sar,
                    (_, false) => Shift::Shr,
                };
                self.asm.shift_ri(kind, Rax, count as u8);
                if op == BinOp::Shl {
                    self.asm.extend(Rax, size, signed);
                }
                true
            }
            BinOp::Div | BinOp::Mod => {
                let ExprKind::Int(k) = rhs.kind else {
                    return false;
                };
                if k == 0 || k == -1 {
                    return false;
                }
                self.gen_expr(lhs);
                self.asm.mov_ri(Rcx, k as i64);
                if signed {
                    self.asm.cqo();
                    self.asm.idiv(Rcx);
                } else {
                    self.asm.zero(Rdx);
                    self.asm.div(Rcx);
                }
                if op == BinOp::Mod {
                    self.asm.mov_rr(Rax, Rdx);
                }
                true
            }
            _ => false,
        }
    }

    fn emit_binop(&mut self, op: BinOp, r#type: &Type) {
        use Reg::*;
        let (size, signed) = self.scalar(r#type);
        match op {
            BinOp::Add => {
                self.asm.alu_rr(Alu::Add, Rax, Rcx);
                self.check_overflow(r#type, OverflowKind::Add);
            }
            BinOp::AddWrap => {
                self.asm.alu_rr(Alu::Add, Rax, Rcx);
                self.asm.extend(Rax, size, signed);
            }
            BinOp::Sub => {
                self.asm.alu_rr(Alu::Sub, Rcx, Rax);
                self.asm.mov_rr(Rax, Rcx);
                self.check_overflow(r#type, OverflowKind::Sub);
            }
            BinOp::SubWrap => {
                self.asm.alu_rr(Alu::Sub, Rcx, Rax);
                self.asm.mov_rr(Rax, Rcx);
                self.asm.extend(Rax, size, signed);
            }
            BinOp::Mul => {
                if size == 8 && !signed {
                    self.asm.mul(Rcx); // OF = CF = (rdx != 0)
                    self.asm.jcc(Cond::O, self.rt.panic_overflow);
                } else {
                    self.asm.imul_rr(Rax, Rcx);
                    self.check_overflow(r#type, OverflowKind::Mul);
                }
            }
            BinOp::MulWrap => {
                self.asm.imul_rr(Rax, Rcx);
                self.asm.extend(Rax, size, signed);
            }
            BinOp::Div | BinOp::Mod => {
                self.asm.xchg_rax(Rcx); // rax = lhs, rcx = rhs
                self.gen_divmod(op == BinOp::Mod, r#type);
            }
            BinOp::BitAnd => self.asm.alu_rr(Alu::And, Rax, Rcx),
            BinOp::BitOr => self.asm.alu_rr(Alu::Or, Rax, Rcx),
            BinOp::BitXor => self.asm.alu_rr(Alu::Xor, Rax, Rcx),
            BinOp::Shl | BinOp::Shr => {
                self.asm.xchg_rax(Rcx);
                self.asm.alu_ri(Alu::Cmp, Rcx, size as i32 * 8);
                self.asm.jcc(Cond::AE, self.rt.panic_overflow);
                let kind = match (op, signed) {
                    (BinOp::Shl, _) => Shift::Shl,
                    (_, true) => Shift::Sar,
                    (_, false) => Shift::Shr,
                };
                self.asm.shift_cl(kind, Rax);
                if op == BinOp::Shl {
                    self.asm.extend(Rax, size, signed);
                }
            }
            _ => unreachable!("ICE: emit_binop({op:?})"),
        }
    }

    fn check_overflow(&mut self, r#type: &Type, kind: OverflowKind) {
        use Reg::*;
        let (size, signed) = self.scalar(r#type);
        let overflow = self.rt.panic_overflow;
        if size == 8 {
            let cc = match (signed, kind) {
                (true, _) => Cond::O,
                (false, OverflowKind::Add | OverflowKind::Sub) => Cond::B,
                (false, OverflowKind::Mul) => Cond::O,
                (false, OverflowKind::Signed) => unreachable!(),
            };
            self.asm.jcc(cc, overflow);
            return;
        }
        if kind == OverflowKind::Mul {
            self.asm.jcc(Cond::O, overflow);
        }
        self.asm.mov_rr(Rcx, Rax);
        self.asm.extend(Rcx, size, signed);
        self.asm.alu_rr(Alu::Cmp, Rax, Rcx);
        self.asm.jcc(Cond::NE, overflow);
    }

    fn gen_divmod(&mut self, is_mod: bool, r#type: &Type) {
        use Reg::*;
        let (size, signed) = self.scalar(r#type);
        let rt = self.rt;
        self.asm.test_rr(Rcx, Rcx);
        self.asm.jcc(Cond::E, rt.panic_div_zero);
        if !signed {
            self.asm.zero(Rdx);
            self.asm.div(Rcx);
            if is_mod {
                self.asm.mov_rr(Rax, Rdx);
            }
            return;
        }
        let normal = self.asm.new_label();
        let done = self.asm.new_label();
        self.asm.alu_ri(Alu::Cmp, Rcx, -1);
        self.asm.jcc(Cond::NE, normal);
        if is_mod {
            self.asm.zero(Rax); // x % -1 == 0
        } else {
            self.asm.neg(Rax); // x / -1 == -x (MIN / -1 déborde)
            if size == 8 {
                self.asm.jcc(Cond::O, rt.panic_overflow);
            } else {
                self.check_overflow(r#type, OverflowKind::Signed);
            }
        }
        self.asm.jmp(done);
        self.asm.bind(normal);
        self.asm.cqo();
        self.asm.idiv(Rcx);
        if is_mod {
            self.asm.mov_rr(Rax, Rdx);
        }
        self.asm.bind(done);
    }

    fn gen_call(&mut self, callee: &Expr, args: &[Expr], target: &CallTarget, ret: &Type) {
        match target {
            CallTarget::Builtin(name) => self.gen_builtin(name, args, ret),
            CallTarget::Direct(name) => {
                let label = self.functions[name];
                self.gen_native_call(None, Some(label), args, ret);
            }
            CallTarget::Indirect => {
                let Type::Fn(ft) = callee.r#type() else {
                    unreachable!()
                };
                if ft.abi == Abi::Efi {
                    self.gen_efi_call(callee, args, ret);
                } else {
                    self.gen_native_call(Some(callee), None, args, ret);
                }
            }
            CallTarget::Unresolved => panic!("ICE: unresolved call"),
        }
    }

    fn gen_native_call(
        &mut self,
        callee: Option<&Expr>,
        label: Option<Label>,
        args: &[Expr],
        ret: &Type,
    ) {
        use Reg::*;
        if let Some(c) = callee {
            self.gen_expr(c);
            self.asm.push(Rax);
        }
        for arg in args {
            let r#type = arg.r#type().clone();
            if r#type.is_aggregate() {
                let off = self.alloc_ty(&r#type);
                self.gen_init(
                    Place {
                        base: Rbp,
                        disp: off,
                    },
                    arg,
                    &r#type,
                );
                self.asm.lea(Rax, Rbp, off);
            } else {
                self.gen_expr(arg);
            }
            self.asm.push(Rax);
        }
        let mut slots = args.len() as i32;
        let sret_off = if ret.is_aggregate() {
            let off = self.alloc_ty(ret);
            self.asm.lea(Rax, Rbp, off);
            self.asm.push(Rax);
            slots += 1;
            Some(off)
        } else {
            None
        };
        match label {
            Some(l) => self.asm.call(l),
            None => {
                self.asm.load(Rax, Rsp, 8 * slots);
                self.asm.call_r(Rax);
                slots += 1;
            }
        }
        if slots > 0 {
            self.asm.alu_ri(Alu::Add, Rsp, 8 * slots);
        }
        if let Some(off) = sret_off {
            self.asm.lea(Rax, Rbp, off);
        }
    }

    fn gen_efi_call(&mut self, callee: &Expr, args: &[Expr], ret: &Type) {
        use Reg::*;
        self.gen_expr(callee);
        self.asm.push(Rax);
        for arg in args {
            self.gen_expr(arg);
            self.asm.push(Rax);
        }
        let n = args.len() as i32;
        self.asm.mov_rr(Rbx, Rsp);
        let stack_args = (n - 4).max(0);
        self.asm.alu_ri(Alu::Sub, Rsp, 32 + 8 * stack_args);
        self.asm.alu_ri(Alu::And, Rsp, -16);
        for i in 4..n {
            self.asm.load(Rax, Rbx, 8 * (n - 1 - i));
            self.asm.store(Rsp, 32 + 8 * (i - 4), Rax);
        }
        for (i, r) in [Rcx, Rdx, R8, R9].into_iter().enumerate().take(n as usize) {
            self.asm.load(r, Rbx, 8 * (n - 1 - i as i32));
        }
        self.asm.load(Rax, Rbx, 8 * n);
        self.asm.call_r(Rax);
        self.asm.lea(Rsp, Rbx, 8 * (n + 1));
        if *ret != Type::Unit {
            let (size, signed) = self.scalar(ret);
            self.asm.extend(Rax, size, signed);
        }
    }

    fn gen_builtin(&mut self, name: &str, args: &[Expr], ret: &Type) {
        use Reg::*;
        let rt = self.rt;
        match name {
            "print" => {
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.asm.call(rt.print_space);
                    }
                    self.gen_expr(arg);
                    let routine = match arg.r#type() {
                        Type::Bool => rt.print_bool,
                        Type::Str => rt.print_str,
                        t if self.signed(t) => rt.print_int,
                        _ => rt.print_uint,
                    };
                    self.asm.call(routine);
                }
                self.asm.call(rt.print_newline);
            }
            "panic" => {
                self.gen_expr(&args[0]);
                self.asm.jmp(rt.panic_msg);
            }
            "exit" => {
                self.gen_expr(&args[0]);
                self.asm.mov_rr(Rdi, Rax);
                self.asm.jmp(rt.sys_exit);
            }
            "outb" | "outw" | "outl" => {
                self.gen_operands(&args[0], &args[1]);
                self.asm.mov_rr(Rdx, Rcx);
                match name {
                    "outb" => self.asm.out_dx_al(),
                    "outw" => self.asm.out_dx_ax(),
                    _ => self.asm.out_dx_eax(),
                }
            }
            "inb" | "inw" | "inl" => {
                self.gen_expr(&args[0]);
                self.asm.mov_rr(Rdx, Rax);
                self.asm.zero(Rax);
                match name {
                    "inb" => self.asm.in_al_dx(),
                    "inw" => self.asm.in_ax_dx(),
                    _ => self.asm.in_eax_dx(),
                }
            }
            "cli" => self.asm.cli(),
            "sti" => self.asm.sti(),
            "hlt" => self.asm.hlt(),
            "pause" => self.asm.pause(),
            "int3" => self.asm.int3(),
            "ud2" => self.asm.ud2(),
            "read_cs" | "read_ds" | "read_ss" => {
                let sreg = match name {
                    "read_cs" => Sreg::Cs,
                    "read_ds" => Sreg::Ds,
                    _ => Sreg::Ss,
                };
                self.asm.zero(Rax);
                self.asm.mov_from_sreg(Rax, sreg);
            }
            "read_tr" => {
                self.asm.zero(Rax);
                self.asm.str_r(Rax);
            }
            "read_cr0" | "read_cr2" | "read_cr3" | "read_cr4" => {
                let cr = name.as_bytes()[7] - b'0';
                self.asm.mov_from_cr(Rax, cr);
            }
            "write_cr0" | "write_cr3" | "write_cr4" => {
                let cr = name.as_bytes()[8] - b'0';
                self.gen_expr(&args[0]);
                self.asm.mov_to_cr(cr, Rax);
            }
            "rdmsr" => {
                self.gen_expr(&args[0]);
                self.asm.mov_rr(Rcx, Rax);
                self.asm.rdmsr();
                self.combine_edx_eax();
            }
            "wrmsr" => {
                self.gen_operands(&args[0], &args[1]); // rcx = msr, rax = value
                self.asm.mov_rr(Rdx, Rax);
                self.asm.shift_ri(Shift::Shr, Rdx, 32);
                self.asm.wrmsr();
            }
            "rdtsc" => {
                self.asm.rdtsc();
                self.combine_edx_eax();
            }
            "invlpg" => {
                self.gen_expr(&args[0]);
                self.asm.invlpg(Rax);
            }
            "lgdt" | "lidt" => {
                self.gen_expr(&args[0]);
                if name == "lgdt" {
                    self.asm.lgdt(Rax);
                } else {
                    self.asm.lidt(Rax);
                }
            }
            "load_cs" => {
                self.gen_expr(&args[0]);
                self.asm.push(Rax);
                let after = self.asm.new_label();
                self.asm.lea_code(Rax, after);
                self.asm.push(Rax);
                self.asm.retfq();
                self.asm.bind(after);
            }
            "load_ds" => {
                self.gen_expr(&args[0]);
                for s in [Sreg::Ds, Sreg::Es, Sreg::Ss, Sreg::Fs, Sreg::Gs] {
                    self.asm.mov_sreg(s, Rax);
                }
            }
            "ltr" => {
                self.gen_expr(&args[0]);
                self.asm.ltr(Rax);
            }
            "memcpy" => {
                self.gen_expr(&args[0]);
                self.asm.push(Rax);
                self.gen_expr(&args[1]);
                self.asm.push(Rax);
                self.gen_expr(&args[2]);
                self.asm.mov_rr(Rcx, Rax);
                self.asm.pop(Rsi);
                self.asm.pop(Rdi);
                self.asm.rep_movsb();
            }
            "memset" => {
                self.gen_expr(&args[0]);
                self.asm.push(Rax);
                self.gen_expr(&args[1]);
                self.asm.push(Rax);
                self.gen_expr(&args[2]);
                self.asm.mov_rr(Rcx, Rax);
                self.asm.pop(Rax);
                self.asm.pop(Rdi);
                self.asm.rep_stosb();
            }
            "efi_image_handle" => {
                self.asm.lea_rw(Rax, rt.efi_image_handle);
                self.asm.load(Rax, Rax, 0);
            }
            "efi_system_table" => {
                self.asm.lea_rw(Rax, rt.efi_system_table);
                self.asm.load(Rax, Rax, 0);
            }
            "switch_stack" => {
                self.gen_operands(&args[0], &args[1]);
                self.asm.mov_rr(Rsp, Rcx);
                self.asm.alu_ri(Alu::And, Rsp, -16);
                self.asm.call_r(Rax);
                self.asm.zero(Rdi);
                self.asm.jmp(rt.sys_exit);
            }
            _ if name.starts_with('.') => self.gen_builtin_method(&name[1..], args, ret),
            _ => panic!("ICE: unknown builtin {name}"),
        }
    }

    fn combine_edx_eax(&mut self) {
        use Reg::*;
        self.asm.shift_ri(Shift::Shl, Rdx, 32);
        self.asm.alu_rr(Alu::Or, Rax, Rdx);
    }

    fn gen_builtin_method(&mut self, name: &str, args: &[Expr], ret: &Type) {
        use Reg::*;
        let recv = &args[0];
        let rt_ty = recv.r#type().clone();
        match (name, &rt_ty) {
            ("len", Type::Array(_, n)) => {
                self.gen_expr(recv);
                self.asm.mov_ri(Rax, *n as i64);
            }
            ("len", Type::Slice(..)) => {
                self.gen_expr(recv);
                self.asm.load(Rax, Rax, 8);
            }
            ("len", Type::Str) => {
                self.gen_expr(recv);
                self.asm.load(Rax, Rax, 0);
            }
            ("as_ptr" | "as_mut_ptr", Type::Array(..)) => self.gen_expr(recv),
            ("as_ptr" | "as_mut_ptr", Type::Slice(..)) => {
                self.gen_expr(recv);
                self.asm.load(Rax, Rax, 0);
            }
            ("as_ptr", Type::Str) => {
                self.gen_expr(recv);
                self.asm.alu_ri(Alu::Add, Rax, 8);
            }
            ("is_null", Type::Raw(..)) => {
                self.gen_expr(recv);
                self.asm.test_rr(Rax, Rax);
                self.asm.setcc(Cond::E, Rax);
                self.asm.movzx8(Rax, Rax);
            }
            ("add" | "sub" | "offset", Type::Raw(t, _)) => {
                let esize = self.size(t).max(1) as i64;
                self.gen_operands(recv, &args[1]);
                self.scale(Rax, esize);
                if name == "sub" {
                    self.asm.alu_rr(Alu::Sub, Rcx, Rax);
                    self.asm.mov_rr(Rax, Rcx);
                } else {
                    self.asm.alu_rr(Alu::Add, Rax, Rcx);
                }
            }
            ("read", Type::Raw(t, _)) => {
                self.gen_expr(recv);
                if !t.is_aggregate() {
                    let t = (**t).clone();
                    self.load_val(Rax, Place { base: Rax, disp: 0 }, &t);
                }
            }
            ("write", Type::Raw(t, _)) => {
                let t = (**t).clone();
                self.gen_operands(recv, &args[1]);
                if t.is_aggregate() {
                    let size = self.size(&t);
                    self.asm.mov_rr(Rdi, Rcx);
                    self.copy_to(Place { base: Rdi, disp: 0 }, Rax, size);
                } else {
                    self.store_val(Place { base: Rcx, disp: 0 }, Rax, &t);
                }
            }
            _ => panic!("ICE: builtin method {name} on {rt_ty:?} -> {ret:?}"),
        }
    }
}
