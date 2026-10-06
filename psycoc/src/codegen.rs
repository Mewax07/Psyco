use std::collections::HashMap;

use crate::{
    Alu, Assembler, Cond, DataLabel, ElfBuilder, Label, LinuxPlatform, PeBuilder, Platform, Reg,
    Runtime, UefiPlatform,
    ast::{BinOp, Block, Expr, ExprKind, Function, Program, Stmt, Type, UnaryOp},
};

pub struct Codegen {
    asm: Assembler,
    rt: Runtime,
    functions: HashMap<String, Label>,
    strings: HashMap<String, DataLabel>,
    scopes: Vec<HashMap<String, i32>>,
    next_local: i32,
    ret_label: Option<Label>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Linux,
    Uefi,
}

impl Codegen {
    pub fn new() -> Self {
        let mut asm = Assembler::new();
        let rt = Runtime::new(&mut asm);

        Self {
            asm,
            rt,
            functions: HashMap::new(),
            strings: HashMap::new(),
            scopes: Vec::new(),
            next_local: 0,
            ret_label: None,
        }
    }

    fn count_locals(&self, block: &Block) -> usize {
        block
            .stmts
            .iter()
            .map(|stmt| match stmt {
                Stmt::Let { .. } => 1,
                Stmt::While { body, .. } => self.count_locals(body),
                Stmt::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    self.count_locals(then_block)
                        + else_block.as_ref().map_or(0, |b| self.count_locals(b))
                }
                _ => 0,
            })
            .sum()
    }

    fn lookup(&self, name: &str) -> i32 {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .copied()
            .expect("ICE: unknown variable reached codegen")
    }

    fn intern(&mut self, s: &str) -> DataLabel {
        if let Some(&label) = self.strings.get(s) {
            return label;
        }
        let label = self.asm.data_str(s);
        self.strings.insert(s.to_string(), label);
        label
    }

    fn cmp_cond(&self, op: BinOp) -> Option<Cond> {
        Some(match op {
            BinOp::Eq => Cond::E,
            BinOp::Ne => Cond::NE,
            BinOp::Lt => Cond::L,
            BinOp::Le => Cond::LE,
            BinOp::Gt => Cond::G,
            BinOp::Ge => Cond::GE,
            _ => return None,
        })
    }

    fn as_imm32(&self, e: &Expr) -> Option<i32> {
        match e.kind {
            ExprKind::Int(n) => i32::try_from(n).ok(),
            ExprKind::Bool(b) => Some(b as i32),
            _ => None,
        }
    }

    fn always_exits(&self, block: &Block) -> bool {
        match block.stmts.last() {
            Some(Stmt::Return { .. }) => true,
            Some(Stmt::If {
                then_block,
                else_block: Some(else_block),
                ..
            }) => self.always_exits(then_block) && self.always_exits(else_block),
            _ => false,
        }
    }

    pub fn generate(&mut self, program: &Program, target: Target) -> Vec<u8> {
        let functions = program
            .functions
            .iter()
            .map(|f| (f.name.clone(), self.asm.new_label()))
            .collect();

        self.functions = functions;

        let entry = self.asm.new_label();
        self.gen_entry(entry);

        let platform: Box<dyn Platform> = match target {
            Target::Linux => Box::new(LinuxPlatform),
            Target::Uefi => Box::new(UefiPlatform),
        };
        platform.gen_write(&mut self.asm, self.rt.sys_write);
        platform.gen_exit(&mut self.asm, self.rt.sys_exit);

        self.gen_runtime();

        for f in &program.functions {
            self.gen_function(f);
        }

        match target {
            Target::Linux => ElfBuilder::new().build(&mut self.asm, entry),
            Target::Uefi => PeBuilder::new().build(&mut self.asm, entry),
        }
    }

    fn gen_entry(&mut self, entry: Label) {
        use Reg::*;
        let main = self.functions["main"];
        let a = &mut self.asm;
        a.bind(entry);
        a.call(main);
        a.mov_ri(Rdi, 0);
        a.jmp(self.rt.sys_exit);
    }

    fn gen_runtime(&mut self) {
        use Reg::*;
        let rt = self.rt;
        let a = &mut self.asm;

        // print_str
        a.bind(rt.print_str);
        a.load(Rdx, Rax, 0);
        a.lea(Rsi, Rax, 8);
        a.jmp(rt.sys_write);

        // print_int
        a.bind(rt.print_int);
        a.push(Rbp);
        a.mov_rr(Rbp, Rsp);
        a.alu_ri(Alu::Sub, Rsp, 32);
        a.mov_rr(R8, Rax);
        let positive = a.new_label();
        a.test_rr(Rax, Rax);
        a.jcc(Cond::NS, positive);
        a.neg(Rax);
        a.bind(positive);
        a.mov_rr(Rsi, Rbp);
        a.mov_ri(Rcx, 10);
        let digit = a.new_label();
        a.bind(digit);
        a.alu_rr(Alu::Xor, Rdx, Rdx);
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
        a.bind(panic);
        a.call(rt.print_str);
        a.mov_ri(Rdi, 101);
        a.jmp(rt.sys_exit);
    }

    fn gen_function(&mut self, f: &Function) {
        use Reg::*;
        let ret = self.asm.new_label();
        self.ret_label = Some(ret);

        self.asm.bind(self.functions[&f.name]);

        self.asm.push(Rbp);
        self.asm.mov_rr(Rbp, Rsp);
        let locals = self.count_locals(&f.body) as i32;
        if locals > 0 {
            self.asm.alu_ri(Alu::Sub, Rsp, locals * 8);
        }

        let n = f.params.len() as i32;
        let mut frame = HashMap::new();
        for (i, param) in f.params.iter().enumerate() {
            frame.insert(param.name.clone(), 16 + 8 * (n - 1 - i as i32));
        }
        self.scopes = vec![frame];
        self.next_local = 0;

        self.gen_block(&f.body);

        self.asm.bind(ret);
        self.asm.mov_rr(Rsp, Rbp);
        self.asm.pop(Rbp);
        self.asm.ret();
    }

    fn gen_block(&mut self, block: &Block) {
        self.scopes.push(HashMap::new());
        for stmt in &block.stmts {
            self.gen_stmt(stmt);
        }
        self.scopes.pop();
    }

    fn gen_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { name, value, .. } => {
                self.gen_expr(value);
                self.next_local -= 8;
                let offset = self.next_local;
                self.scopes.last_mut().unwrap().insert(name.clone(), offset);
                self.asm.store(Reg::Rbp, offset, Reg::Rax);
            }
            Stmt::Assign { name, value, .. } => {
                self.gen_expr(value);
                let offset = self.lookup(name);
                self.asm.store(Reg::Rbp, offset, Reg::Rax);
            }
            Stmt::Expr(e) => self.gen_expr(e),
            Stmt::Return { value, .. } => {
                if let Some(e) = value {
                    self.gen_expr(e);
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

                self.asm.jmp(cond_label);
                self.asm.bind(body_label);
                self.gen_block(body);
                self.asm.bind(cond_label);
                self.gen_branch(cond, true, body_label);
            }
        }
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
                    // jump if lhs && rhs
                    let skip = self.asm.new_label();
                    self.gen_branch(lhs, false, skip);
                    self.gen_branch(rhs, true, target);
                    self.asm.bind(skip);
                } else {
                    // jump if !(lhs && rhs)
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
                    // jump if !(lhs || rhs)
                    let skip = self.asm.new_label();
                    self.gen_branch(lhs, true, skip);
                    self.gen_branch(rhs, false, target);
                    self.asm.bind(skip);
                }
            }
            ExprKind::Binary { op, lhs, rhs } if self.cmp_cond(*op).is_some() => {
                let cc = self.gen_cmp(self.cmp_cond(*op).unwrap(), lhs, rhs);
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

    fn gen_cmp(&mut self, cc: Cond, lhs: &Expr, rhs: &Expr) -> Cond {
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

    fn gen_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Int(0) | ExprKind::Bool(false) => self.asm.zero(Reg::Rax),
            ExprKind::Int(n) => self.asm.mov_ri(Reg::Rax, *n),
            ExprKind::Bool(b) => self.asm.mov_ri(Reg::Rax, *b as i64),
            ExprKind::Str(s) => {
                let label = self.intern(s);
                self.asm.lea_data(Reg::Rax, label);
            }
            ExprKind::Var(name) => {
                let offset = self.lookup(name);
                self.asm.load(Reg::Rax, Reg::Rbp, offset);
            }
            ExprKind::Unary { op, operand } => {
                self.gen_expr(operand);
                match op {
                    UnaryOp::Neg => {
                        self.asm.neg(Reg::Rax);
                        self.asm.jcc(Cond::O, self.rt.panic_overflow);
                    }
                    UnaryOp::Not => self.asm.alu_ri(Alu::Xor, Reg::Rax, 1),
                }
            }
            ExprKind::Binary { op, lhs, rhs } => self.gen_binary(*op, lhs, rhs),
            ExprKind::Call { callee, args } => self.gen_call(callee, args),
        }
    }

    fn gen_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr) {
        use Reg::*;
        let overflow = self.rt.panic_overflow;

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
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let cc = self.gen_cmp(self.cmp_cond(op).unwrap(), lhs, rhs);
                self.asm.setcc(cc, Rax);
                self.asm.movzx8(Rax, Rax);
            }
            BinOp::Add | BinOp::Mul => {
                let imm_side = self.as_imm32(rhs)
                    .map(|imm| (lhs, imm))
                    .or_else(|| self.as_imm32(lhs).map(|imm| (rhs, imm)));

                match (op, imm_side) {
                    (BinOp::Add, Some((other, imm))) => {
                        self.gen_expr(other);
                        self.asm.alu_ri(Alu::Add, Rax, imm);
                    }
                    (BinOp::Mul, Some((other, imm))) => {
                        self.gen_expr(other);
                        self.asm.imul_ri(Rax, Rax, imm);
                    }
                    (BinOp::Add, None) => {
                        self.gen_operands(lhs, rhs);
                        self.asm.alu_rr(Alu::Add, Rax, Rcx);
                    }
                    (BinOp::Mul, None) => {
                        self.gen_operands(lhs, rhs);
                        self.asm.imul_rr(Rax, Rcx);
                    }
                    _ => unreachable!(),
                }
                self.asm.jcc(Cond::O, overflow);
            }
            BinOp::Sub => {
                if let Some(imm) = self.as_imm32(rhs) {
                    // x - k
                    self.gen_expr(lhs);
                    self.asm.alu_ri(Alu::Sub, Rax, imm);
                } else if let ExprKind::Int(k) = lhs.kind {
                    self.gen_expr(rhs);
                    self.asm.mov_rr(Rcx, Rax);
                    if k == 0 {
                        self.asm.zero(Rax);
                    } else {
                        self.asm.mov_ri(Rax, k);
                    }
                    self.asm.alu_rr(Alu::Sub, Rax, Rcx);
                } else {
                    self.gen_operands(lhs, rhs); // rcx = lhs, rax = rhs
                    self.asm.alu_rr(Alu::Sub, Rcx, Rax);
                    self.asm.mov_rr(Rax, Rcx);
                }
                self.asm.jcc(Cond::O, overflow);
            }
            BinOp::Div | BinOp::Mod => {
                let is_mod = op == BinOp::Mod;
                match rhs.kind {
                    ExprKind::Int(k) if k != 0 && k != -1 => {
                        self.gen_expr(lhs);
                        self.asm.mov_ri(Rcx, k);
                        self.asm.cqo();
                        self.asm.idiv(Rcx);
                        if is_mod {
                            self.asm.mov_rr(Rax, Rdx);
                        }
                    }
                    _ => {
                        self.gen_expr(lhs);
                        self.asm.push(Rax);
                        self.gen_expr(rhs);
                        self.asm.mov_rr(Rcx, Rax);
                        self.asm.pop(Rax); // rax = lhs, rcx = rhs
                        self.gen_divmod(is_mod);
                    }
                }
            }
        }
    }

    fn gen_divmod(&mut self, is_mod: bool) {
        let rt = self.rt;
        let a = &mut self.asm;
        let normal = a.new_label();
        let done = a.new_label();

        a.test_rr(Reg::Rcx, Reg::Rcx);
        a.jcc(Cond::E, rt.panic_div_zero);
        a.alu_ri(Alu::Cmp, Reg::Rcx, -1);
        a.jcc(Cond::NE, normal);
        if is_mod {
            a.alu_rr(Alu::Xor, Reg::Rax, Reg::Rax); // x % -1 == 0
        } else {
            a.neg(Reg::Rax); // x / -1 == -x
            a.jcc(Cond::O, rt.panic_overflow); // i64::MIN / -1
        }
        a.jmp(done);

        a.bind(normal);
        a.cqo();
        a.idiv(Reg::Rcx);
        if is_mod {
            a.mov_rr(Reg::Rax, Reg::Rdx); // rest
        }
        a.bind(done);
    }

    fn gen_call(&mut self, callee: &str, args: &[Expr]) {
        if callee == "print" {
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    self.asm.call(self.rt.print_space);
                }
                self.gen_expr(arg);
                let routine = match arg.r#type.expect("ICE: expression not typed") {
                    Type::Int => self.rt.print_int,
                    Type::Bool => self.rt.print_bool,
                    Type::Str => self.rt.print_str,
                    Type::Unit => unreachable!("ICE: Unit passed to print"),
                };
                self.asm.call(routine);
            }
            self.asm.call(self.rt.print_newline);
            return;
        }

        for arg in args {
            self.gen_expr(arg);
            self.asm.push(Reg::Rax);
        }
        let target = self.functions[callee];
        self.asm.call(target);
        if !args.is_empty() {
            self.asm.alu_ri(Alu::Add, Reg::Rsp, (args.len() * 8) as i32);
        }
    }
}
