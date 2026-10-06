use std::collections::HashMap;

use crate::{
    Alu, Assembler, Cond, DataLabel, ElfBuilder, Label, LinuxPlatform, PeBuilder, Platform, Reg, Runtime, UefiPlatform, ast::{BinOp, Block, Expr, ExprKind, Function, Program, Stmt, Type, UnaryOp},
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
                let end_label = self.asm.new_label();

                self.gen_expr(cond);
                self.asm.alu_ri(Alu::Cmp, Reg::Rax, 0);
                self.asm.jcc(Cond::E, else_label);
                self.gen_block(then_block);
                self.asm.jmp(end_label);
                self.asm.bind(else_label);
                if let Some(block) = else_block {
                    self.gen_block(block);
                }
                self.asm.bind(end_label);
            }
            Stmt::While { cond, body, .. } => {
                let start_label = self.asm.new_label();
                let end_label = self.asm.new_label();

                self.asm.bind(start_label);
                self.gen_expr(cond);
                self.asm.alu_ri(Alu::Cmp, Reg::Rax, 0);
                self.asm.jcc(Cond::E, end_label);
                self.gen_block(body);
                self.asm.jmp(start_label);
                self.asm.bind(end_label);
            }
        }
    }

    fn gen_expr(&mut self, expr: &Expr) {
        match &expr.kind {
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
        if op == BinOp::And || op == BinOp::Or {
            let end_label = self.asm.new_label();
            self.gen_expr(lhs);
            self.asm.alu_ri(Alu::Cmp, Reg::Rax, 0);
            let cc = if op == BinOp::And { Cond::E } else { Cond::NE };
            self.asm.jcc(cc, end_label);
            self.gen_expr(rhs);
            self.asm.bind(end_label);
            return;
        }

        self.gen_expr(lhs);
        self.asm.push(Reg::Rax);
        self.gen_expr(rhs);
        self.asm.mov_rr(Reg::Rcx, Reg::Rax);
        self.asm.pop(Reg::Rax); // rax = lhs, rcx = rhs

        let overflow = self.rt.panic_overflow;
        match op {
            BinOp::Add => {
                self.asm.alu_rr(Alu::Add, Reg::Rax, Reg::Rcx);
                self.asm.jcc(Cond::O, overflow);
            }
            BinOp::Sub => {
                self.asm.alu_rr(Alu::Sub, Reg::Rax, Reg::Rcx);
                self.asm.jcc(Cond::O, overflow);
            }
            BinOp::Mul => {
                self.asm.imul_rr(Reg::Rax, Reg::Rcx);
                self.asm.jcc(Cond::O, overflow);
            }
            BinOp::Div => self.gen_divmod(false),
            BinOp::Mod => self.gen_divmod(true),
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let cc = match op {
                    BinOp::Eq => Cond::E,
                    BinOp::Ne => Cond::NE,
                    BinOp::Lt => Cond::L,
                    BinOp::Le => Cond::LE,
                    BinOp::Gt => Cond::G,
                    BinOp::Ge => Cond::GE,
                    _ => unreachable!(),
                };
                self.asm.alu_rr(Alu::Cmp, Reg::Rax, Reg::Rcx);
                self.asm.setcc(cc, Reg::Rax);
                self.asm.movzx8(Reg::Rax, Reg::Rax);
            }
            BinOp::And | BinOp::Or => unreachable!(),
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
