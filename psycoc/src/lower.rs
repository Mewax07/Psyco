use std::collections::{HashMap, HashSet};

use crate::ir::*;
use crate::*;

type Block_ = crate::ast::Block;

#[derive(Clone, Copy)]
enum VarL {
    Reg(VReg),
    Mem(Mem),
}

#[derive(Clone, Copy)]
enum Val {
    S(Op),
    A(Mem),
}

enum PlaceL {
    Reg(VReg),
    Mem(Mem),
}

#[derive(Clone, Copy)]
enum PLoc {
    Val(Op),
    Mem(Mem),
}

pub struct Lower<'c, 'a> {
    cg: &'c mut Codegen<'a>,
    f: Func,
    cur: BlockId,
    scopes: Vec<HashMap<String, VarL>>,
    loops: Vec<(BlockId, BlockId)>,
    sret: Option<VReg>,
    addr_taken: HashSet<String>,
}

impl<'c, 'a> Lower<'c, 'a> {
    pub fn new(cg: &'c mut Codegen<'a>, name: &str) -> Self {
        Lower {
            cg,
            f: Func {
                name: name.to_string(),
                blocks: vec![ir::Block {
                    insts: Vec::new(),
                    term: Term::Unreachable,
                }],
                nvregs: 0,
                slots: Vec::new(),
            },
            cur: 0,
            scopes: Vec::new(),
            loops: Vec::new(),
            sret: None,
            addr_taken: HashSet::new(),
        }
    }

    // Building blocks

    fn emit(&mut self, i: Inst) {
        self.f.blocks[self.cur].insts.push(i);
    }

    fn new_block(&mut self) -> BlockId {
        self.f.blocks.push(ir::Block {
            insts: Vec::new(),
            term: Term::Unreachable,
        });
        self.f.blocks.len() - 1
    }

    fn terminate(&mut self, t: Term) {
        self.f.blocks[self.cur].term = t;
        self.cur = self.new_block();
    }

    fn jump(&mut self, b: BlockId) {
        self.terminate(Term::Jmp(b));
    }

    fn switch_to(&mut self, b: BlockId) {
        self.cur = b;
    }

    fn branch_if(&mut self, cc: Cond, a: Op, b: Op, target: BlockId) {
        let cont = self.new_block();
        self.f.blocks[self.cur].term = Term::Br {
            cc,
            a,
            b,
            t: target,
            f: cont,
        };
        self.cur = cont;
    }

    fn vreg(&mut self) -> VReg {
        self.f.new_vreg()
    }

    fn slot(&mut self, size: u64, align: u64) -> SlotId {
        self.f.slots.push((size.max(1), align.max(1)));
        self.f.slots.len() - 1
    }

    fn slot_ty(&mut self, t: &Type) -> SlotId {
        let (s, a) = (self.cg.info.size_of(t), self.cg.info.align_of(t));
        self.slot(s, a)
    }

    fn ty(&self, t: &Type) -> Ty {
        match t {
            Type::Int(i) => Ty {
                size: i.size() as u8,
                signed: i.signed(),
            },
            Type::Bool => Ty::U8,
            Type::Enum(_) => {
                let r = self.cg.info.int_repr(t).unwrap();
                Ty {
                    size: r.size() as u8,
                    signed: r.signed(),
                }
            }
            Type::Ref(..) | Type::Raw(..) | Type::Fn(_) | Type::Str | Type::Unit => Ty::U64,
            _ => panic!("ICE: {t:?} is not a scalar"),
        }
    }

    fn signed(&self, t: &Type) -> bool {
        self.cg.info.int_repr(t).is_some_and(|i| i.signed())
    }

    fn size(&self, t: &Type) -> u64 {
        self.cg.info.size_of(t)
    }

    fn load(&mut self, mem: Mem, ty: Ty) -> Op {
        let dst = self.vreg();
        self.emit(Inst::Load { dst, mem, ty });
        Op::V(dst)
    }

    fn bin(&mut self, op: BinK, a: Op, b: Op, ty: Ty, mode: Arith) -> Op {
        let dst = self.vreg();
        self.emit(Inst::Bin {
            dst,
            op,
            a,
            b,
            ty,
            mode,
        });
        Op::V(dst)
    }

    fn sym(&mut self, sym: Sym) -> Op {
        let dst = self.vreg();
        self.emit(Inst::SymAddr { dst, sym });
        Op::V(dst)
    }

    fn mem_addr(&mut self, m: Mem) -> Op {
        match m {
            Mem::Base(o, 0) => o,
            Mem::Base(o, d) => self.bin(BinK::Add, o, Op::I(d as i64), Ty::U64, Arith::Plain),
            _ => {
                let dst = self.vreg();
                self.emit(Inst::Addr { dst, mem: m });
                Op::V(dst)
            }
        }
    }

    fn scale(&mut self, n: Op, k: i64) -> Op {
        if k == 1 {
            return n;
        }
        if k > 0 && (k as u64).is_power_of_two() {
            return self.bin(
                BinK::Shl,
                n,
                Op::I(k.trailing_zeros() as i64),
                Ty::U64,
                Arith::Plain,
            );
        }
        self.bin(BinK::Mul, n, Op::I(k), Ty::U64, Arith::Plain)
    }

    fn mem_index(&mut self, m: Mem, idx: Op, esize: i64) -> Mem {
        if let Op::I(i) = idx {
            return m.offset((i * esize) as i32);
        }
        let off = self.scale(idx, esize);
        match m {
            Mem::Base(o, d) => {
                let sum = self.bin(BinK::Add, o, off, Ty::U64, Arith::Plain);
                Mem::Base(sum, d)
            }
            _ => {
                let a = self.mem_addr(m);
                let sum = self.bin(BinK::Add, a, off, Ty::U64, Arith::Plain);
                Mem::Base(sum, 0)
            }
        }
    }

    fn lookup(&self, name: &str) -> Option<VarL> {
        self.scopes.iter().rev().find_map(|s| s.get(name)).copied()
    }

    fn declare(&mut self, name: &str, v: VarL) {
        self.scopes.last_mut().unwrap().insert(name.to_string(), v);
    }

    fn new_var(&mut self, name: &str, t: &Type) -> VarL {
        if !t.is_aggregate() && !self.addr_taken.contains(name) {
            VarL::Reg(self.vreg())
        } else {
            VarL::Mem(Mem::Slot(self.slot_ty(t), 0))
        }
    }

    // Function

    pub fn lower(mut self, f: &Function) -> Func {
        collect_addr_taken(&f.body, &mut self.addr_taken);
        let sig = self.cg.info.functions[&f.name].clone();
        let has_sret = sig.ret.is_aggregate();
        let sret_slots = has_sret as i32;
        let n = f.params.len() as i32;
        let mut frame = HashMap::new();
        for (i, (param, t)) in f.params.iter().zip(&sig.params).enumerate() {
            let off = 16 + 8 * (sret_slots + n - 1 - i as i32);
            let v = if t.is_aggregate() {
                let p = self.load(Mem::Param(off), Ty::U64);
                VarL::Mem(Mem::Base(p, 0))
            } else if self.addr_taken.contains(&param.name) {
                VarL::Mem(Mem::Param(off))
            } else {
                let p = self.load(Mem::Param(off), Ty::U64);
                let r = self.vreg();
                self.emit(Inst::Mov { dst: r, src: p });
                VarL::Reg(r)
            };
            frame.insert(param.name.clone(), v);
        }
        if has_sret {
            let p = self.load(Mem::Param(16), Ty::U64);
            self.sret = p.vreg();
        }
        self.scopes = vec![frame];
        self.block(&f.body);
        self.terminate(Term::Ret(None));
        self.f
    }

    // Statements

    fn block(&mut self, b: &Block_) {
        self.scopes.push(HashMap::new());
        for s in &b.stmts {
            self.stmt(s);
        }
        self.scopes.pop();
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let {
                name,
                value,
                r#type,
                ..
            } => {
                let t = r#type.clone().expect("ICE: let not typed");
                let var = self.new_var(name, &t);
                match var {
                    VarL::Reg(r) => {
                        let v = match value {
                            Some(v) => self.scalar(v),
                            None => Op::I(0),
                        };
                        self.emit(Inst::Mov { dst: r, src: v });
                    }
                    VarL::Mem(m) => match value {
                        Some(v) => self.init(m, v, &t),
                        None => {
                            let size = self.size(&t);
                            self.emit(Inst::Zero { dst: m, size });
                        }
                    },
                }
                self.declare(name, var);
            }
            Stmt::Assign {
                target, op, value, ..
            } => self.assign(target, *op, value),
            Stmt::Expr(e) => {
                self.expr(e);
            }
            Stmt::Return { value, .. } => {
                let r = match value {
                    None => None,
                    Some(e) => match self.sret {
                        Some(sret) => {
                            let src = self.agg(e);
                            let size = self.size(e.r#type());
                            self.emit(Inst::Copy {
                                dst: Mem::Base(Op::V(sret), 0),
                                src,
                                size,
                            });
                            Some(Op::V(sret))
                        }
                        None => Some(self.scalar(e)),
                    },
                };
                self.terminate(Term::Ret(r));
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                let then_b = self.new_block();
                let else_b = else_block.as_ref().map(|_| self.new_block());
                let end = self.new_block();
                self.cond_br(cond, then_b, else_b.unwrap_or(end));
                self.switch_to(then_b);
                self.block(then_block);
                self.jump(end);
                if let (Some(b), Some(eb)) = (else_block, else_b) {
                    self.switch_to(eb);
                    self.block(b);
                    self.jump(end);
                }
                self.switch_to(end);
            }
            Stmt::While { cond, body, .. } => {
                let body_b = self.new_block();
                let cond_b = self.new_block();
                let end = self.new_block();
                self.jump(cond_b);
                self.switch_to(body_b);
                self.loops.push((cond_b, end));
                self.block(body);
                self.loops.pop();
                self.jump(cond_b);
                self.switch_to(cond_b);
                self.cond_br(cond, body_b, end);
                self.switch_to(end);
            }
            Stmt::Loop { body, .. } => {
                let top = self.new_block();
                let end = self.new_block();
                self.jump(top);
                self.switch_to(top);
                self.loops.push((top, end));
                self.block(body);
                self.loops.pop();
                self.jump(top);
                self.switch_to(end);
            }
            Stmt::For {
                var,
                start,
                end,
                body,
                ..
            } => self.lower_for(var, start, end, body),
            Stmt::Break(_) => {
                let (_, brk) = *self.loops.last().expect("ICE: break outside loop");
                self.jump(brk);
            }
            Stmt::Continue(_) => {
                let (cont, _) = *self.loops.last().expect("ICE: continue outside loop");
                self.jump(cont);
            }
            Stmt::Match {
                scrutinee, arms, ..
            } => {
                if simple_match(scrutinee, arms) {
                    self.simple_match(scrutinee, arms);
                } else {
                    self.lower_match(scrutinee, arms);
                }
            }
            Stmt::IfLet {
                pat,
                value,
                then_block,
                else_block,
                ..
            } => {
                let root = self.scrutinee(value);
                let else_b = self.new_block();
                let end = self.new_block();
                self.pat_test(pat, root, else_b);
                self.scopes.push(HashMap::new());
                self.pat_bind(pat, root);
                self.block(then_block);
                self.scopes.pop();
                self.jump(end);
                self.switch_to(else_b);
                if let Some(b) = else_block {
                    self.block(b);
                }
                self.jump(end);
                self.switch_to(end);
            }
            Stmt::LetElse {
                pat,
                value,
                else_block,
                ..
            } => {
                let root = self.scrutinee(value);
                let else_b = self.new_block();
                self.pat_test(pat, root, else_b);
                let ok = self.cur;
                self.switch_to(else_b);
                self.block(else_block);
                self.switch_to(ok);
                self.pat_bind(pat, root);
            }
            Stmt::WhileLet {
                pat, value, body, ..
            } => {
                let top = self.new_block();
                let end = self.new_block();
                self.jump(top);
                self.switch_to(top);
                let root = self.scrutinee(value);
                self.pat_test(pat, root, end);
                self.scopes.push(HashMap::new());
                self.pat_bind(pat, root);
                self.loops.push((top, end));
                self.block(body);
                self.loops.pop();
                self.scopes.pop();
                self.jump(top);
                self.switch_to(end);
            }
            Stmt::Block(b) => self.block(b),
        }
    }

    fn lower_for(&mut self, var: &str, start: &Expr, end: &Expr, body: &Block_) {
        let t = start.r#type().clone();
        let ty = self.ty(&t);
        let signed = self.signed(&t);
        let s = self.scalar(start);
        let e = self.scalar(end);
        let endv = self.vreg();
        self.emit(Inst::Mov { dst: endv, src: e });
        let v = self.new_var(var, &t);
        match v {
            VarL::Reg(r) => self.emit(Inst::Mov { dst: r, src: s }),
            VarL::Mem(m) => self.emit(Inst::Store {
                mem: m,
                src: s,
                size: ty.size,
            }),
        }
        let body_b = self.new_block();
        let cont_b = self.new_block();
        let cond_b = self.new_block();
        let end_b = self.new_block();
        self.jump(cond_b);

        self.switch_to(body_b);
        self.scopes.push(HashMap::new());
        self.declare(var, v);
        self.loops.push((cont_b, end_b));
        self.block(body);
        self.loops.pop();
        self.scopes.pop();
        self.jump(cont_b);

        self.switch_to(cont_b);
        match v {
            VarL::Reg(r) => {
                let n = self.bin(BinK::Add, Op::V(r), Op::I(1), Ty::U64, Arith::Plain);
                self.emit(Inst::Mov { dst: r, src: n });
            }
            VarL::Mem(m) => {
                let cur = self.load(m, ty);
                let n = self.bin(BinK::Add, cur, Op::I(1), Ty::U64, Arith::Plain);
                self.emit(Inst::Store {
                    mem: m,
                    src: n,
                    size: ty.size,
                });
            }
        }
        self.jump(cond_b);

        self.switch_to(cond_b);
        let cur = match v {
            VarL::Reg(r) => Op::V(r),
            VarL::Mem(m) => self.load(m, ty),
        };
        self.f.blocks[self.cur].term = Term::Br {
            cc: if signed { Cond::L } else { Cond::B },
            a: cur,
            b: Op::V(endv),
            t: body_b,
            f: end_b,
        };
        self.cur = end_b;
    }

    fn assign(&mut self, target: &Expr, op: Option<BinOp>, value: &Expr) {
        let t = target.r#type().clone();
        if let ExprKind::Var(name) = &target.kind {
            if let Some(VarL::Reg(r)) = self.lookup(name) {
                let v = self.scalar(value);
                let v = match op {
                    None => v,
                    Some(op) => self.binop(op, Op::V(r), v, &t),
                };
                self.emit(Inst::Mov { dst: r, src: v });
                return;
            }
        }
        let dst = self.mem_of(target);
        match op {
            None if t.is_aggregate() => {
                let src = self.agg(value);
                let size = self.size(&t);
                self.emit(Inst::Copy { dst, src, size });
            }
            None => {
                let v = self.scalar(value);
                let size = self.ty(&t).size;
                self.emit(Inst::Store {
                    mem: dst,
                    src: v,
                    size,
                });
            }
            Some(op) => {
                let ty = self.ty(&t);
                let cur = self.load(dst, ty);
                let v = self.scalar(value);
                let r = self.binop(op, cur, v, &t);
                self.emit(Inst::Store {
                    mem: dst,
                    src: r,
                    size: ty.size,
                });
            }
        }
    }

    fn init(&mut self, dst: Mem, e: &Expr, t: &Type) {
        match &e.kind {
            ExprKind::StructLit { fields, .. } => {
                let Type::Struct(sid) = t else { unreachable!() };
                for (name, value, _) in fields {
                    let fl = self.cg.info.field(*sid, name).clone();
                    self.init(dst.offset(fl.offset as i32), value, &fl.r#type);
                }
            }
            ExprKind::ArrayLit(items) => {
                let Type::Array(elem, _) = t else {
                    unreachable!()
                };
                let esize = self.size(elem) as i32;
                for (i, item) in items.iter().enumerate() {
                    self.init(dst.offset(i as i32 * esize), item, elem);
                }
            }
            ExprKind::ArrayRepeat { value, .. } => {
                let Type::Array(elem, n) = t else {
                    unreachable!()
                };
                let (elem, n) = ((**elem).clone(), *n);
                let esize = self.size(&elem);
                if is_zero(&self.cg.info, value) {
                    self.emit(Inst::Zero {
                        dst,
                        size: esize * n,
                    });
                } else if elem.is_aggregate() {
                    let tmp = Mem::Slot(self.slot_ty(&elem), 0);
                    self.init(tmp, value, &elem);
                    if n <= 4 {
                        for i in 0..n as i32 {
                            self.emit(Inst::Copy {
                                dst: dst.offset(i * esize as i32),
                                src: tmp,
                                size: esize,
                            });
                        }
                    } else {
                        // A loop: i in 0..n
                        let i = self.vreg();
                        self.emit(Inst::Mov {
                            dst: i,
                            src: Op::I(0),
                        });
                        let body = self.new_block();
                        let end = self.new_block();
                        self.jump(body);
                        self.switch_to(body);
                        let d = self.mem_index(dst, Op::V(i), esize as i64);
                        self.emit(Inst::Copy {
                            dst: d,
                            src: tmp,
                            size: esize,
                        });
                        let next = self.bin(BinK::Add, Op::V(i), Op::I(1), Ty::U64, Arith::Plain);
                        self.emit(Inst::Mov { dst: i, src: next });
                        self.f.blocks[self.cur].term = Term::Br {
                            cc: Cond::B,
                            a: Op::V(i),
                            b: Op::I(n as i64),
                            t: body,
                            f: end,
                        };
                        self.cur = end;
                    }
                } else {
                    let v = self.scalar(value);
                    let size = self.ty(&elem).size;
                    if n <= 16 {
                        for i in 0..n as i32 {
                            self.emit(Inst::Store {
                                mem: dst.offset(i * esize as i32),
                                src: v,
                                size,
                            });
                        }
                    } else {
                        self.emit(Inst::Fill {
                            dst,
                            value: v,
                            count: n,
                            size,
                        });
                    }
                }
            }
            ExprKind::VariantLit { sum, variant, args } => {
                let l = self.cg.info.sums[*sum].clone();
                let v = &l.variants[*variant];
                if l.niche.is_some() {
                    if v.fields.is_empty() {
                        self.emit(Inst::Zero { dst, size: l.size });
                    }
                } else {
                    self.emit(Inst::Store {
                        mem: dst,
                        src: Op::I(v.tag as i64),
                        size: l.tag.size() as u8,
                    });
                }
                for (a, (ft, off)) in args.iter().zip(&v.fields) {
                    self.init(dst.offset(*off as i32), a, ft);
                }
            }
            _ if t.is_aggregate() => {
                let src = self.agg(e);
                let size = self.size(t);
                self.emit(Inst::Copy { dst, src, size });
            }
            _ => {
                let v = self.scalar(e);
                let size = self.ty(t).size;
                self.emit(Inst::Store {
                    mem: dst,
                    src: v,
                    size,
                });
            }
        }
    }

    // Places

    fn place(&mut self, e: &Expr) -> PlaceL {
        match &e.kind {
            ExprKind::Var(name) => {
                if let Some(v) = self.lookup(name) {
                    return match v {
                        VarL::Reg(r) => PlaceL::Reg(r),
                        VarL::Mem(m) => PlaceL::Mem(m),
                    };
                }
                if let Some(loc) = self.cg.statics.get(name).copied() {
                    let a = self.sym(match loc {
                        StaticLoc::Ro(d) => Sym::Data(d),
                        StaticLoc::Rw(d) => Sym::Rw(d),
                    });
                    return PlaceL::Mem(Mem::Base(a, 0));
                }
                self.temp_place(e)
            }
            ExprKind::Field { base, name } => {
                let Type::Struct(sid) = base.r#type() else {
                    panic!("ICE: field access on non-struct")
                };
                let off = self.cg.info.field(*sid, name).offset as i32;
                let m = self.mem_of(base);
                PlaceL::Mem(m.offset(off))
            }
            ExprKind::Index { base, index } => PlaceL::Mem(self.index_place(base, index)),
            ExprKind::Unary {
                op: UnaryOp::Deref,
                operand,
            } => {
                let p = self.scalar(operand);
                PlaceL::Mem(Mem::Base(p, 0))
            }
            _ => self.temp_place(e),
        }
    }

    fn temp_place(&mut self, e: &Expr) -> PlaceL {
        match self.expr(e) {
            Val::A(m) => PlaceL::Mem(m),
            Val::S(v) => {
                let t = e.r#type().clone();
                let s = Mem::Slot(self.slot_ty(&t), 0);
                let size = self.ty(&t).size;
                self.emit(Inst::Store {
                    mem: s,
                    src: v,
                    size,
                });
                PlaceL::Mem(s)
            }
        }
    }

    fn mem_of(&mut self, e: &Expr) -> Mem {
        match self.place(e) {
            PlaceL::Mem(m) => m,
            PlaceL::Reg(_) => panic!("ICE: address of a register variable in {}", self.f.name),
        }
    }

    fn index_place(&mut self, base: &Expr, index: &Expr) -> Mem {
        let trap = self.cg.rt.panic_bounds;
        match base.r#type().clone() {
            Type::Array(elem, n) => {
                let esize = self.size(&elem) as i64;
                let m = self.mem_of(base);
                if let ExprKind::Int(i) = index.kind {
                    return m.offset((i as i64 * esize) as i32);
                }
                let i = self.scalar(index);
                self.emit(Inst::Check {
                    a: i,
                    b: Op::I(n as i64),
                    cc: Cond::AE,
                    trap,
                });
                self.mem_index(m, i, esize)
            }
            Type::Slice(elem, _) => {
                let esize = self.size(&elem) as i64;
                let m = self.mem_of(base);
                let i = self.scalar(index);
                let len = self.load(m.offset(8), Ty::U64);
                self.emit(Inst::Check {
                    a: i,
                    b: len,
                    cc: Cond::AE,
                    trap,
                });
                let ptr = self.load(m, Ty::U64);
                self.mem_index(Mem::Base(ptr, 0), i, esize)
            }
            other => panic!("ICE: index on {other:?}"),
        }
    }

    fn addr_of(&mut self, e: &Expr) -> Op {
        let m = self.mem_of(e);
        self.mem_addr(m)
    }

    // Expressions

    fn scalar(&mut self, e: &Expr) -> Op {
        match self.expr(e) {
            Val::S(o) => o,
            Val::A(m) => self.mem_addr(m),
        }
    }

    fn agg(&mut self, e: &Expr) -> Mem {
        match self.expr(e) {
            Val::A(m) => m,
            Val::S(o) => Mem::Base(o, 0),
        }
    }

    fn expr(&mut self, e: &Expr) -> Val {
        let t = e.r#type().clone();
        match &e.kind {
            ExprKind::Int(v) => Val::S(Op::I(*v as i64)),
            ExprKind::Bool(b) => Val::S(Op::I(*b as i64)),
            ExprKind::Str(s) | ExprKind::Bytes(s) => {
                let l = self.cg.intern_str(s);
                Val::S(self.sym(Sym::Data(l)))
            }
            ExprKind::CStr(s) => {
                let l = self.cg.intern_cstr(s);
                Val::S(self.sym(Sym::Data(l)))
            }
            ExprKind::WStr(s) => {
                let l = self.cg.intern_wstr(s);
                Val::S(self.sym(Sym::Data(l)))
            }
            ExprKind::Var(name)
                if self.lookup(name).is_none() && !self.cg.statics.contains_key(name) =>
            {
                let label = self.cg.functions[name];
                Val::S(self.sym(Sym::Code(label)))
            }
            ExprKind::Var(_) | ExprKind::Field { .. } | ExprKind::Index { .. } => {
                match self.place(e) {
                    PlaceL::Reg(r) => Val::S(Op::V(r)),
                    PlaceL::Mem(m) => {
                        if t.is_aggregate() {
                            Val::A(m)
                        } else {
                            let ty = self.ty(&t);
                            Val::S(self.load(m, ty))
                        }
                    }
                }
            }
            ExprKind::Unary { op, operand } => self.unary(*op, operand, &t),
            ExprKind::Binary { op, lhs, rhs } => match op {
                BinOp::And | BinOp::Or => {
                    let res = self.vreg();
                    let tb = self.new_block();
                    let fb = self.new_block();
                    let end = self.new_block();
                    self.cond_br(e, tb, fb);
                    self.switch_to(tb);
                    self.emit(Inst::Mov {
                        dst: res,
                        src: Op::I(1),
                    });
                    self.jump(end);
                    self.switch_to(fb);
                    self.emit(Inst::Mov {
                        dst: res,
                        src: Op::I(0),
                    });
                    self.jump(end);
                    self.switch_to(end);
                    Val::S(Op::V(res))
                }
                _ if op.is_comparison() => {
                    let (cc, a, b) = self.compare(*op, lhs, rhs);
                    let dst = self.vreg();
                    self.emit(Inst::Set { dst, cc, a, b });
                    Val::S(Op::V(dst))
                }
                _ => {
                    let a = self.scalar(lhs);
                    let b = self.scalar(rhs);
                    Val::S(self.binop(*op, a, b, &t))
                }
            },
            ExprKind::Cast { expr, .. } => {
                let v = self.scalar(expr);
                let from = expr.r#type().clone();
                match &t {
                    Type::Int(it) if matches!(from, Type::Int(_) | Type::Enum(_) | Type::Bool) => {
                        let f = self.ty(&from);
                        let (ts, tsg) = (it.size() as u8, it.signed());
                        if f.size > ts
                            || (f.size == ts && f.signed != tsg)
                            || (f.size < ts && f.signed && !tsg)
                        {
                            let dst = self.vreg();
                            self.emit(Inst::Ext {
                                dst,
                                a: v,
                                ty: Ty {
                                    size: ts,
                                    signed: tsg,
                                },
                            });
                            return Val::S(Op::V(dst));
                        }
                        Val::S(v)
                    }
                    Type::Raw(..) if from == Type::Str => {
                        Val::S(self.bin(BinK::Add, v, Op::I(8), Ty::U64, Arith::Plain))
                    }
                    _ => Val::S(v),
                }
            }
            ExprKind::Call {
                callee,
                args,
                target,
            } => self.call(callee, args, target, &t),
            ExprKind::StructLit { .. }
            | ExprKind::ArrayLit(_)
            | ExprKind::ArrayRepeat { .. }
            | ExprKind::VariantLit { .. } => {
                let s = Mem::Slot(self.slot_ty(&t), 0);
                self.init(s, e, &t);
                Val::A(s)
            }
            ExprKind::Range { base, start, end } => {
                self.range(base, start.as_deref(), end.as_deref())
            }
            ExprKind::Unsize(inner) => {
                let Type::Ref(arr, _) = inner.r#type() else {
                    unreachable!()
                };
                let Type::Array(_, n) = **arr else {
                    unreachable!()
                };
                let p = self.scalar(inner);
                let s = Mem::Slot(self.slot(16, 8), 0);
                self.emit(Inst::Store {
                    mem: s,
                    src: p,
                    size: 8,
                });
                self.emit(Inst::Store {
                    mem: s.offset(8),
                    src: Op::I(n as i64),
                    size: 8,
                });
                Val::A(s)
            }
            ExprKind::TryOp {
                inner,
                ok,
                fail,
                ret_sum,
                ret_fail,
            } => self.lower_try(inner, *ok, *fail, *ret_sum, *ret_fail),
            ExprKind::TypedInt(..)
            | ExprKind::Path(..)
            | ExprKind::Sizeof(_)
            | ExprKind::Turbofish { .. }
            | ExprKind::TypePath { .. }
            | ExprKind::Try(_) => panic!(
                "ICE: {:?} should have been rewritten by the type checker",
                e.kind
            ),
        }
    }

    fn binop(&mut self, op: BinOp, a: Op, b: Op, t: &Type) -> Op {
        let ty = self.ty(t);
        let (k, mode) = match op {
            BinOp::Add => (BinK::Add, Arith::Checked),
            BinOp::Sub => (BinK::Sub, Arith::Checked),
            BinOp::Mul => (BinK::Mul, Arith::Checked),
            BinOp::AddWrap => (BinK::Add, Arith::Wrap),
            BinOp::SubWrap => (BinK::Sub, Arith::Wrap),
            BinOp::MulWrap => (BinK::Mul, Arith::Wrap),
            BinOp::Div => (BinK::Div, Arith::Checked),
            BinOp::Mod => (BinK::Mod, Arith::Checked),
            BinOp::BitAnd => (BinK::And, Arith::Plain),
            BinOp::BitOr => (BinK::Or, Arith::Plain),
            BinOp::BitXor => (BinK::Xor, Arith::Plain),
            BinOp::Shl => (BinK::Shl, Arith::Checked),
            BinOp::Shr => (BinK::Shr, Arith::Checked),
            _ => unreachable!("ICE: binop {op:?}"),
        };
        let (a, b) = match (a, b) {
            (Op::I(_), Op::V(_)) if k.commutative() => (b, a),
            _ => (a, b),
        };
        self.bin(k, a, b, ty, mode)
    }

    fn compare(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr) -> (Cond, Op, Op) {
        let signed = self.signed(lhs.r#type());
        let cc = match (op, signed) {
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
        };
        let a = self.scalar(lhs);
        let b = self.scalar(rhs);
        match (a, b) {
            (Op::I(_), Op::V(_)) => (cc.swap(), b, a),
            _ => (cc, a, b),
        }
    }

    fn cond_br(&mut self, e: &Expr, t: BlockId, f: BlockId) {
        match &e.kind {
            ExprKind::Bool(b) => self.jump(if *b { t } else { f }),
            ExprKind::Unary {
                op: UnaryOp::Not,
                operand,
            } => self.cond_br(operand, f, t),
            ExprKind::Binary {
                op: BinOp::And,
                lhs,
                rhs,
            } => {
                let mid = self.new_block();
                self.cond_br(lhs, mid, f);
                self.switch_to(mid);
                self.cond_br(rhs, t, f);
            }
            ExprKind::Binary {
                op: BinOp::Or,
                lhs,
                rhs,
            } => {
                let mid = self.new_block();
                self.cond_br(lhs, t, mid);
                self.switch_to(mid);
                self.cond_br(rhs, t, f);
            }
            ExprKind::Binary { op, lhs, rhs } if op.is_comparison() => {
                let (cc, a, b) = self.compare(*op, lhs, rhs);
                self.terminate(Term::Br { cc, a, b, t, f });
            }
            _ => {
                let v = self.scalar(e);
                self.terminate(Term::Br {
                    cc: Cond::NE,
                    a: v,
                    b: Op::I(0),
                    t,
                    f,
                });
            }
        }
    }

    fn unary(&mut self, op: UnaryOp, operand: &Expr, t: &Type) -> Val {
        match op {
            UnaryOp::Neg => {
                let a = self.scalar(operand);
                let ty = self.ty(t);
                let dst = self.vreg();
                self.emit(Inst::Neg { dst, a, ty });
                Val::S(Op::V(dst))
            }
            UnaryOp::Not => {
                let a = self.scalar(operand);
                Val::S(self.bin(BinK::Xor, a, Op::I(1), Ty::U8, Arith::Plain))
            }
            UnaryOp::BitNot => {
                let a = self.scalar(operand);
                let ty = self.ty(t);
                let dst = self.vreg();
                self.emit(Inst::BitNot { dst, a, ty });
                Val::S(Op::V(dst))
            }
            UnaryOp::Ref(_) => Val::S(self.addr_of(operand)),
            UnaryOp::Deref => {
                let p = self.scalar(operand);
                if t.is_aggregate() {
                    Val::A(Mem::Base(p, 0))
                } else {
                    let ty = self.ty(t);
                    Val::S(self.load(Mem::Base(p, 0), ty))
                }
            }
        }
    }

    fn range(&mut self, base: &Expr, start: Option<&Expr>, end: Option<&Expr>) -> Val {
        let trap = self.cg.rt.panic_bounds;
        let (elem, array_len) = match base.r#type() {
            Type::Array(e, n) => ((**e).clone(), Some(*n)),
            Type::Slice(e, _) => ((**e).clone(), None),
            _ => unreachable!(),
        };
        let esize = self.size(&elem) as i64;
        let m = self.mem_of(base);
        let (ptr, len) = match array_len {
            Some(n) => (self.mem_addr(m), Op::I(n as i64)),
            None => (self.load(m, Ty::U64), self.load(m.offset(8), Ty::U64)),
        };
        let s = match start {
            Some(s) => self.scalar(s),
            None => Op::I(0),
        };
        let e = match end {
            Some(e) => self.scalar(e),
            None => len,
        };
        self.emit(Inst::Check {
            a: e,
            b: len,
            cc: Cond::A,
            trap,
        });
        self.emit(Inst::Check {
            a: s,
            b: e,
            cc: Cond::A,
            trap,
        });
        let new_len = self.bin(BinK::Sub, e, s, Ty::U64, Arith::Plain);
        let off = self.scale(s, esize);
        let new_ptr = self.bin(BinK::Add, ptr, off, Ty::U64, Arith::Plain);
        let slot = Mem::Slot(self.slot(16, 8), 0);
        self.emit(Inst::Store {
            mem: slot,
            src: new_ptr,
            size: 8,
        });
        self.emit(Inst::Store {
            mem: slot.offset(8),
            src: new_len,
            size: 8,
        });
        Val::A(slot)
    }

    fn lower_try(
        &mut self,
        inner: &Expr,
        ok: usize,
        fail: usize,
        ret_sum: usize,
        ret_fail: usize,
    ) -> Val {
        let Type::Sum(id) = inner.r#type().clone() else {
            unreachable!()
        };
        let l = self.cg.info.sums[id].clone();
        let rl = self.cg.info.sums[ret_sum].clone();
        let m = self.agg(inner);
        let ok_b = self.new_block();
        let fail_b = self.new_block();
        match l.niche {
            Some(_) => {
                let x = self.load(m, Ty::U64);
                self.terminate(Term::Br {
                    cc: Cond::NE,
                    a: x,
                    b: Op::I(0),
                    t: ok_b,
                    f: fail_b,
                });
            }
            None => {
                let tag = self.load(
                    m,
                    Ty {
                        size: l.tag.size() as u8,
                        signed: l.tag.signed(),
                    },
                );
                self.terminate(Term::Br {
                    cc: Cond::E,
                    a: tag,
                    b: Op::I(l.variants[ok].tag as i64),
                    t: ok_b,
                    f: fail_b,
                });
            }
        }

        self.switch_to(fail_b);
        let sret = self
            .sret
            .expect("ICE: '?' in a function without an enum result");
        let dst = Mem::Base(Op::V(sret), 0);
        match rl.niche {
            Some(_) => self.emit(Inst::Zero { dst, size: rl.size }),
            None => self.emit(Inst::Store {
                mem: dst,
                src: Op::I(rl.variants[ret_fail].tag as i64),
                size: rl.tag.size() as u8,
            }),
        }
        if let (Some((et, src_off)), Some((_, dst_off))) = (
            l.variants[fail].fields.first().cloned(),
            rl.variants[ret_fail].fields.first().cloned(),
        ) {
            let src = m.offset(src_off as i32);
            let d = dst.offset(dst_off as i32);
            if et.is_aggregate() {
                let size = self.size(&et);
                self.emit(Inst::Copy { dst: d, src, size });
            } else {
                let ty = self.ty(&et);
                let v = self.load(src, ty);
                self.emit(Inst::Store {
                    mem: d,
                    src: v,
                    size: ty.size,
                });
            }
        }
        self.terminate(Term::Ret(Some(Op::V(sret))));
        self.switch_to(ok_b);
        let (vt, off) = l.variants[ok].fields[0].clone();
        if vt.is_aggregate() {
            Val::A(m.offset(off as i32))
        } else {
            let ty = self.ty(&vt);
            Val::S(self.load(m.offset(off as i32), ty))
        }
    }

    // Calls

    fn call(&mut self, callee: &Expr, args: &[Expr], target: &CallTarget, ret: &Type) -> Val {
        match target {
            CallTarget::Builtin(name) => self.builtin(name, args, ret),
            CallTarget::Direct(name) => {
                let label = self.cg.functions[name];
                self.native_call(CallT::Label(label), args, ret)
            }
            CallTarget::Indirect => {
                let Type::Fn(ft) = callee.r#type() else {
                    unreachable!()
                };
                if ft.abi == Abi::Efi {
                    let c = self.scalar(callee);
                    let ops: Vec<Op> = args.iter().map(|a| self.scalar(a)).collect();
                    let ret_ty = (*ret != Type::Unit).then(|| self.ty(ret));
                    let dst = ret_ty.map(|_| self.vreg());
                    self.emit(Inst::EfiCall {
                        dst,
                        callee: c,
                        args: ops,
                        ret: ret_ty,
                    });
                    Val::S(dst.map_or(Op::I(0), Op::V))
                } else {
                    let c = self.scalar(callee);
                    self.native_call(CallT::Op(c), args, ret)
                }
            }
            CallTarget::Unresolved => panic!("ICE: unresolved call"),
        }
    }

    fn native_call(&mut self, target: CallT, args: &[Expr], ret: &Type) -> Val {
        let mut ops = Vec::new();
        for a in args {
            let t = a.r#type().clone();
            let fresh = match &a.kind {
                ExprKind::Unsize(_) | ExprKind::Range { .. } => true,
                ExprKind::Call { target, .. } => {
                    matches!(target, CallTarget::Direct(_) | CallTarget::Indirect)
                }
                _ => false,
            };
            if t.is_aggregate() && fresh {
                // A temporary already: no copy.
                let m = self.agg(a);
                ops.push(self.mem_addr(m));
            } else if t.is_aggregate() {
                let s = Mem::Slot(self.slot_ty(&t), 0);
                self.init(s, a, &t);
                ops.push(self.mem_addr(s));
            } else {
                ops.push(self.scalar(a));
            }
        }
        if ret.is_aggregate() {
            let s = Mem::Slot(self.slot_ty(ret), 0);
            ops.push(self.mem_addr(s));
            self.emit(Inst::Call {
                dst: None,
                target,
                args: ops,
            });
            return Val::A(s);
        }
        if *ret == Type::Unit {
            self.emit(Inst::Call {
                dst: None,
                target,
                args: ops,
            });
            return Val::S(Op::I(0));
        }
        let dst = self.vreg();
        self.emit(Inst::Call {
            dst: Some(dst),
            target,
            args: ops,
        });
        Val::S(Op::V(dst))
    }

    fn intr(&mut self, kind: Intr, args: Vec<Op>, has_dst: bool) -> Val {
        let dst = has_dst.then(|| self.vreg());
        self.emit(Inst::Intr { kind, dst, args });
        Val::S(dst.map_or(Op::I(0), Op::V))
    }

    fn builtin(&mut self, name: &str, args: &[Expr], ret: &Type) -> Val {
        let rt = self.cg.rt;
        let ops = |s: &mut Self| -> Vec<Op> { args.iter().map(|a| s.scalar(a)).collect() };
        match name {
            "print" => {
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.emit(Inst::CallRt {
                            routine: rt.print_space,
                            arg: None,
                        });
                    }
                    if let Type::Slice(..) = arg.r#type() {
                        let m = self.agg(arg);
                        let slice = self.mem_addr(m);
                        self.emit(Inst::PrintBytes { slice });
                        continue;
                    }
                    let v = self.scalar(arg);
                    let routine = match arg.r#type() {
                        Type::Bool => rt.print_bool,
                        Type::Str => rt.print_str,
                        t if self.signed(t) => rt.print_int,
                        _ => rt.print_uint,
                    };
                    self.emit(Inst::CallRt {
                        routine,
                        arg: Some(v),
                    });
                }
                self.emit(Inst::CallRt {
                    routine: rt.print_newline,
                    arg: None,
                });
                Val::S(Op::I(0))
            }
            "panic" => {
                let v = self.scalar(&args[0]);
                self.terminate(Term::Panic(v));
                Val::S(Op::I(0))
            }
            "exit" => {
                let v = self.scalar(&args[0]);
                self.terminate(Term::Exit(v));
                Val::S(Op::I(0))
            }
            "outb" | "outw" | "outl" => {
                let size = match name {
                    "outb" => 1,
                    "outw" => 2,
                    _ => 4,
                };
                let o = ops(self);
                self.intr(Intr::Out(size), o, false)
            }
            "inb" | "inw" | "inl" => {
                let size = match name {
                    "inb" => 1,
                    "inw" => 2,
                    _ => 4,
                };
                let o = ops(self);
                self.intr(Intr::In(size), o, true)
            }
            "cli" => self.intr(Intr::Cli, Vec::new(), false),
            "sti" => self.intr(Intr::Sti, Vec::new(), false),
            "hlt" => self.intr(Intr::Hlt, Vec::new(), false),
            "pause" => self.intr(Intr::Pause, Vec::new(), false),
            "int3" => self.intr(Intr::Int3, Vec::new(), false),
            "ud2" => self.intr(Intr::Ud2, Vec::new(), false),
            "read_rflags" => self.intr(Intr::ReadFlags, Vec::new(), true),
            "read_cs" => self.intr(Intr::ReadSreg(Sreg::Cs as u8), Vec::new(), true),
            "read_ds" => self.intr(Intr::ReadSreg(Sreg::Ds as u8), Vec::new(), true),
            "read_ss" => self.intr(Intr::ReadSreg(Sreg::Ss as u8), Vec::new(), true),
            "read_tr" => self.intr(Intr::ReadTr, Vec::new(), true),
            "read_cr0" | "read_cr2" | "read_cr3" | "read_cr4" => {
                self.intr(Intr::ReadCr(name.as_bytes()[7] - b'0'), Vec::new(), true)
            }
            "write_cr0" | "write_cr3" | "write_cr4" => {
                let o = ops(self);
                self.intr(Intr::WriteCr(name.as_bytes()[8] - b'0'), o, false)
            }
            "rdmsr" => {
                let o = ops(self);
                self.intr(Intr::Rdmsr, o, true)
            }
            "wrmsr" => {
                let o = ops(self);
                self.intr(Intr::Wrmsr, o, false)
            }
            "rdtsc" => self.intr(Intr::Rdtsc, Vec::new(), true),
            "invlpg" | "lgdt" | "lidt" | "load_cs" | "load_ds" | "ltr" | "memcpy" | "memset"
            | "memset32" | "context_switch" | "rebase_statics" | "set_syscall_stack" => {
                let kind = match name {
                    "invlpg" => Intr::Invlpg,
                    "lgdt" => Intr::Lgdt,
                    "lidt" => Intr::Lidt,
                    "load_cs" => Intr::LoadCs,
                    "load_ds" => Intr::LoadDs,
                    "ltr" => Intr::Ltr,
                    "memcpy" => Intr::Memcpy,
                    "memset" => Intr::Memset,
                    "memset32" => Intr::Memset32,
                    "context_switch" => Intr::ContextSwitch,
                    "rebase_statics" => Intr::RebaseStatics,
                    _ => Intr::SetSyscallStack,
                };
                let o = ops(self);
                self.intr(kind, o, false)
            }
            "syscall" | "syscall6" => {
                let o = ops(self);
                self.intr(Intr::Syscall, o, true)
            }
            "iret_to" => {
                let o = ops(self);
                self.terminate(Term::IretTo(o));
                Val::S(Op::I(0))
            }
            "slice_from_raw" | "slice_from_raw_mut" => {
                let p = self.scalar(&args[0]);
                let n = self.scalar(&args[1]);
                let s = Mem::Slot(self.slot(16, 8), 0);
                self.emit(Inst::Store {
                    mem: s,
                    src: p,
                    size: 8,
                });
                self.emit(Inst::Store {
                    mem: s.offset(8),
                    src: n,
                    size: 8,
                });
                Val::A(s)
            }
            "efi_image_handle" => self.intr(Intr::EfiImageHandle, Vec::new(), true),
            "efi_system_table" => self.intr(Intr::EfiSystemTable, Vec::new(), true),
            "switch_stack" => {
                let stack = self.scalar(&args[0]);
                let f = self.scalar(&args[1]);
                self.terminate(Term::SwitchStack { stack, f });
                Val::S(Op::I(0))
            }
            _ if name.starts_with('.') => self.builtin_method(&name[1..], args, ret),
            _ => panic!("ICE: unknown builtin {name}"),
        }
    }

    fn builtin_method(&mut self, name: &str, args: &[Expr], ret: &Type) -> Val {
        let recv = &args[0];
        let rt_ty = recv.r#type().clone();
        match (name, &rt_ty) {
            ("len", Type::Array(_, n)) => {
                self.expr(recv);
                Val::S(Op::I(*n as i64))
            }
            ("len", Type::Slice(..)) => {
                let m = self.agg(recv);
                Val::S(self.load(m.offset(8), Ty::U64))
            }
            ("len", Type::Str) => {
                let s = self.scalar(recv);
                Val::S(self.load(Mem::Base(s, 0), Ty::U64))
            }
            ("as_ptr" | "as_mut_ptr", Type::Array(..)) => {
                let m = self.agg(recv);
                Val::S(self.mem_addr(m))
            }
            ("as_ptr" | "as_mut_ptr", Type::Slice(..)) => {
                let m = self.agg(recv);
                Val::S(self.load(m, Ty::U64))
            }
            ("as_ptr", Type::Str) => {
                let s = self.scalar(recv);
                Val::S(self.bin(BinK::Add, s, Op::I(8), Ty::U64, Arith::Plain))
            }
            ("as_bytes", Type::Str) => {
                let s = self.scalar(recv);
                let slot = Mem::Slot(self.slot(16, 8), 0);
                let len = self.load(Mem::Base(s, 0), Ty::U64);
                self.emit(Inst::Store {
                    mem: slot.offset(8),
                    src: len,
                    size: 8,
                });
                let p = self.bin(BinK::Add, s, Op::I(8), Ty::U64, Arith::Plain);
                self.emit(Inst::Store {
                    mem: slot,
                    src: p,
                    size: 8,
                });
                Val::A(slot)
            }
            ("is_null", Type::Raw(..)) => {
                let p = self.scalar(recv);
                let dst = self.vreg();
                self.emit(Inst::Set {
                    dst,
                    cc: Cond::E,
                    a: p,
                    b: Op::I(0),
                });
                Val::S(Op::V(dst))
            }
            ("add" | "sub" | "offset", Type::Raw(t, _)) => {
                let esize = self.size(t).max(1) as i64;
                let p = self.scalar(recv);
                let n = self.scalar(&args[1]);
                let off = self.scale(n, esize);
                let k = if name == "sub" { BinK::Sub } else { BinK::Add };
                Val::S(self.bin(k, p, off, Ty::U64, Arith::Plain))
            }
            ("read", Type::Raw(t, _)) => {
                let p = self.scalar(recv);
                if t.is_aggregate() {
                    Val::A(Mem::Base(p, 0))
                } else {
                    let ty = self.ty(t);
                    Val::S(self.load(Mem::Base(p, 0), ty))
                }
            }
            ("write", Type::Raw(t, _)) => {
                let t = (**t).clone();
                let p = self.scalar(recv);
                if t.is_aggregate() {
                    let src = self.agg(&args[1]);
                    let size = self.size(&t);
                    self.emit(Inst::Copy {
                        dst: Mem::Base(p, 0),
                        src,
                        size,
                    });
                } else {
                    let v = self.scalar(&args[1]);
                    let size = self.ty(&t).size;
                    self.emit(Inst::Store {
                        mem: Mem::Base(p, 0),
                        src: v,
                        size,
                    });
                }
                Val::S(Op::I(0))
            }
            _ => panic!("ICE: builtin method {name} on {rt_ty:?} -> {ret:?}"),
        }
    }

    // Patterns

    fn scrutinee(&mut self, e: &Expr) -> PLoc {
        match self.expr(e) {
            Val::S(o) => PLoc::Val(o),
            Val::A(m) => PLoc::Mem(m),
        }
    }

    fn ploc_load(&mut self, loc: PLoc, t: &Type) -> Op {
        match loc {
            PLoc::Val(o) => o,
            PLoc::Mem(m) => {
                let ty = self.ty(t);
                self.load(m, ty)
            }
        }
    }

    fn ploc_deref(&mut self, loc: PLoc) -> PLoc {
        match loc {
            PLoc::Val(o) => PLoc::Mem(Mem::Base(o, 0)),
            PLoc::Mem(m) => {
                let p = self.load(m, Ty::U64);
                PLoc::Mem(Mem::Base(p, 0))
            }
        }
    }

    fn pat_test(&mut self, p: &Pattern, loc: PLoc, fail: BlockId) {
        let loc = if p.deref { self.ploc_deref(loc) } else { loc };
        match &p.kind {
            PatKind::Or(alts) => {
                let ok = self.new_block();
                for (i, a) in alts.iter().enumerate() {
                    if i + 1 < alts.len() {
                        let next = self.new_block();
                        self.pat_test(a, loc, next);
                        self.jump(ok);
                        self.switch_to(next);
                    } else {
                        self.pat_test(a, loc, fail);
                    }
                }
                self.jump(ok);
                self.switch_to(ok);
            }
            PatKind::Const(v) => {
                let t = p.r#type.clone().expect("ICE: pattern not typed");
                let x = self.ploc_load(loc, &t);
                self.branch_if(Cond::NE, x, Op::I(*v as i64), fail);
            }
            PatKind::Ctor { sum, variant, args } => {
                let l = self.cg.info.sums[*sum].clone();
                let PLoc::Mem(m) = loc else {
                    panic!("ICE: enum value not in memory")
                };
                match l.niche {
                    Some(empty) => {
                        let x = self.load(m, Ty::U64);
                        let cc = if *variant == empty { Cond::NE } else { Cond::E };
                        self.branch_if(cc, x, Op::I(0), fail);
                    }
                    None => {
                        let tag = self.load(
                            m,
                            Ty {
                                size: l.tag.size() as u8,
                                signed: l.tag.signed(),
                            },
                        );
                        self.branch_if(Cond::NE, tag, Op::I(l.variants[*variant].tag as i64), fail);
                    }
                }
                for (a, (_, off)) in args.iter().zip(&l.variants[*variant].fields) {
                    self.pat_test(a, PLoc::Mem(m.offset(*off as i32)), fail);
                }
            }
            _ => {}
        }
    }

    fn pat_bind(&mut self, p: &Pattern, loc: PLoc) {
        match &p.kind {
            PatKind::Bind { name, by_ref, .. } => {
                let vt = p.r#type.clone().expect("ICE: pattern not typed");
                let (value, t) = match by_ref {
                    Some(m) => {
                        let PLoc::Mem(mem) = loc else {
                            panic!("ICE: reference to a value slot")
                        };
                        (
                            Val::S(self.mem_addr(mem)),
                            Type::Ref(Box::new(vt.clone()), *m),
                        )
                    }
                    None => match loc {
                        PLoc::Val(o) => (Val::S(o), vt.clone()),
                        PLoc::Mem(mem) if vt.is_aggregate() => (Val::A(mem), vt.clone()),
                        PLoc::Mem(mem) => {
                            let ty = self.ty(&vt);
                            (Val::S(self.load(mem, ty)), vt.clone())
                        }
                    },
                };
                let var = self.new_var(name, &t);
                match (var, value) {
                    (VarL::Reg(r), Val::S(o)) => self.emit(Inst::Mov { dst: r, src: o }),
                    (VarL::Mem(m), Val::S(o)) => {
                        let size = self.ty(&t).size;
                        self.emit(Inst::Store {
                            mem: m,
                            src: o,
                            size,
                        });
                    }
                    (VarL::Mem(m), Val::A(src)) => {
                        let size = self.size(&t);
                        self.emit(Inst::Copy { dst: m, src, size });
                    }
                    (VarL::Reg(_), Val::A(_)) => unreachable!(),
                }
                self.declare(name, var);
            }
            PatKind::Ctor { sum, variant, args } if args.iter().any(pat_binds) => {
                let loc = if p.deref { self.ploc_deref(loc) } else { loc };
                let PLoc::Mem(m) = loc else {
                    panic!("ICE: enum value not in memory")
                };
                let fields = self.cg.info.sums[*sum].variants[*variant].fields.clone();
                for (a, (_, off)) in args.iter().zip(&fields) {
                    self.pat_bind(a, PLoc::Mem(m.offset(*off as i32)));
                }
            }
            _ => {}
        }
    }

    fn simple_match(&mut self, scrutinee: &Expr, arms: &[MatchArm]) {
        let v = self.scalar(scrutinee);
        let end = self.new_block();
        let mut targets = Vec::new();
        let mut wildcard = None;
        for arm in arms {
            let b = self.new_block();
            for p in &arm.pats {
                match p.kind {
                    PatKind::Const(c) => self.branch_if(Cond::E, v, Op::I(c as i64), b),
                    _ => {
                        if wildcard.is_none() {
                            wildcard = Some(b);
                        }
                    }
                }
            }
            targets.push(b);
        }
        self.jump(wildcard.unwrap_or(end));
        for (arm, b) in arms.iter().zip(targets) {
            self.switch_to(b);
            self.block(&arm.body);
            self.jump(end);
        }
        self.switch_to(end);
    }

    fn lower_match(&mut self, scrutinee: &Expr, arms: &[MatchArm]) {
        let root = self.scrutinee(scrutinee);
        let end = self.new_block();
        for arm in arms {
            let next = self.new_block();
            let body = self.new_block();
            let n = arm.pats.len();
            for (i, p) in arm.pats.iter().enumerate() {
                if i + 1 < n {
                    let alt_fail = self.new_block();
                    self.pat_test(p, root, alt_fail);
                    self.jump(body);
                    self.switch_to(alt_fail);
                } else {
                    self.pat_test(p, root, next);
                }
            }
            self.jump(body);
            self.switch_to(body);
            self.scopes.push(HashMap::new());
            self.pat_bind(&arm.pats[0], root);
            if let Some(g) = &arm.guard {
                let ok = self.new_block();
                self.cond_br(g, ok, next);
                self.switch_to(ok);
            }
            self.block(&arm.body);
            self.scopes.pop();
            self.jump(end);
            self.switch_to(next);
        }
        self.jump(end);
        self.switch_to(end);
    }
}

fn simple_match(scrutinee: &Expr, arms: &[MatchArm]) -> bool {
    matches!(
        scrutinee.r#type(),
        Type::Int(_) | Type::Bool | Type::Enum(_)
    ) && arms.iter().all(|a| {
        a.guard.is_none()
            && a.pats
                .iter()
                .all(|p| matches!(p.kind, PatKind::Const(_) | PatKind::Wild))
    })
}

fn pat_binds(p: &Pattern) -> bool {
    match &p.kind {
        PatKind::Bind { .. } => true,
        PatKind::Ctor { args, .. } => args.iter().any(pat_binds),
        _ => false,
    }
}

pub(crate) fn is_zero(info: &ProgramInfo, e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(0) | ExprKind::Bool(false) => true,
        ExprKind::ArrayRepeat { value, .. } => is_zero(info, value),
        ExprKind::ArrayLit(items) => items.iter().all(|l| is_zero(info, l)),
        ExprKind::StructLit { fields, .. } => fields.iter().all(|(_, v, _)| is_zero(info, v)),
        ExprKind::VariantLit { sum, variant, args } => {
            let l = &info.sums[*sum];
            let tag_zero = match l.niche {
                Some(empty) => *variant == empty,
                None => l.variants[*variant].tag == 0,
            };
            tag_zero && args.iter().all(|a| is_zero(info, a))
        }
        _ => false,
    }
}

fn collect_addr_taken(b: &Block_, out: &mut HashSet<String>) {
    for s in &b.stmts {
        stmt_addr(s, out);
    }
}

fn root_name(e: &Expr) -> Option<&str> {
    match &e.kind {
        ExprKind::Var(n) => Some(n),
        ExprKind::Field { base, .. } | ExprKind::Index { base, .. } => root_name(base),
        _ => None,
    }
}

fn stmt_addr(s: &Stmt, out: &mut HashSet<String>) {
    match s {
        Stmt::Let { value, .. } => {
            if let Some(v) = value {
                expr_addr(v, out);
            }
        }
        Stmt::Assign { target, value, .. } => {
            expr_addr(target, out);
            expr_addr(value, out);
        }
        Stmt::Expr(e) => expr_addr(e, out),
        Stmt::Return { value, .. } => {
            if let Some(v) = value {
                expr_addr(v, out);
            }
        }
        Stmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            expr_addr(cond, out);
            collect_addr_taken(then_block, out);
            if let Some(b) = else_block {
                collect_addr_taken(b, out);
            }
        }
        Stmt::While { cond, body, .. } => {
            expr_addr(cond, out);
            collect_addr_taken(body, out);
        }
        Stmt::Loop { body, .. } => collect_addr_taken(body, out),
        Stmt::For {
            start, end, body, ..
        } => {
            expr_addr(start, out);
            expr_addr(end, out);
            collect_addr_taken(body, out);
        }
        Stmt::Break(_) | Stmt::Continue(_) => {}
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            expr_addr(scrutinee, out);
            for a in arms {
                if let Some(g) = &a.guard {
                    expr_addr(g, out);
                }
                collect_addr_taken(&a.body, out);
            }
        }
        Stmt::IfLet {
            value,
            then_block,
            else_block,
            ..
        } => {
            expr_addr(value, out);
            collect_addr_taken(then_block, out);
            if let Some(b) = else_block {
                collect_addr_taken(b, out);
            }
        }
        Stmt::LetElse { value, else_block, .. } => {
            expr_addr(value, out);
            collect_addr_taken(else_block, out);
        }
        Stmt::WhileLet { value, body, .. } => {
            expr_addr(value, out);
            collect_addr_taken(body, out);
        }
        Stmt::Block(b) => collect_addr_taken(b, out),
    }
}

fn expr_addr(e: &Expr, out: &mut HashSet<String>) {
    match &e.kind {
        ExprKind::Unary {
            op: UnaryOp::Ref(_),
            operand,
        } => {
            if let Some(n) = root_name(operand) {
                out.insert(n.to_string());
            }
            expr_addr(operand, out);
        }
        ExprKind::Range { base, start, end } => {
            if let Some(n) = root_name(base) {
                out.insert(n.to_string());
            }
            expr_addr(base, out);
            for x in [start, end].into_iter().flatten() {
                expr_addr(x, out);
            }
        }
        ExprKind::Unary { operand, .. } => expr_addr(operand, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            expr_addr(lhs, out);
            expr_addr(rhs, out);
        }
        ExprKind::Cast { expr, .. } => expr_addr(expr, out),
        ExprKind::Call { callee, args, .. } => {
            expr_addr(callee, out);
            for a in args {
                expr_addr(a, out);
            }
        }
        ExprKind::Field { base, .. } => expr_addr(base, out),
        ExprKind::Index { base, index } => {
            expr_addr(base, out);
            expr_addr(index, out);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, v, _) in fields {
                expr_addr(v, out);
            }
        }
        ExprKind::ArrayLit(items) => {
            for i in items {
                expr_addr(i, out);
            }
        }
        ExprKind::ArrayRepeat { value, .. } => expr_addr(value, out),
        ExprKind::Unsize(x) | ExprKind::Try(x) => expr_addr(x, out),
        ExprKind::TryOp { inner, .. } => expr_addr(inner, out),
        ExprKind::VariantLit { args, .. } => {
            for a in args {
                expr_addr(a, out);
            }
        }
        _ => {}
    }
}
