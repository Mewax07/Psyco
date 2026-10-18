use std::collections::HashMap;

use super::*;

type Moved = Option<HashMap<usize, Span>>;

fn union(a: Moved, b: Moved) -> Moved {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some(mut a), Some(b)) => {
            for (k, v) in b {
                a.entry(k).or_insert(v);
            }
            Some(a)
        }
    }
}

struct LoopCtx {
    breaks: Moved,
    continues: Moved,
}

struct Mv<'a> {
    tc: &'a TypeChecker,
    names: Vec<String>,
    scopes: Vec<Vec<(String, usize)>>,
    state: Moved,
    loops: Vec<LoopCtx>,
    quiet: u32,
    error: Option<TypeError>,
}

impl Mv<'_> {
    fn fail(&mut self, message: String, span: Span) {
        if self.quiet == 0 && self.error.is_none() {
            self.error = Some(TypeError { message, span });
        }
    }

    fn lookup(&self, name: &str) -> Option<usize> {
        self.scopes
            .iter()
            .rev()
            .find_map(|s| s.iter().rev().find(|(n, _)| n == name).map(|(_, id)| *id))
    }

    fn declare(&mut self, name: &str) {
        let id = self.names.len();
        self.names.push(name.to_string());
        self.scopes.last_mut().unwrap().push((name.to_string(), id));
        if let Some(s) = &mut self.state {
            s.remove(&id);
        }
    }

    fn check_alive(&mut self, id: usize, span: Span) {
        if let Some(s) = &self.state {
            if let Some(at) = s.get(&id) {
                let msg = format!(
                    "use of moved value '{}' (moved at line {}; give it a new value first)",
                    self.names[id], at.line
                );
                self.fail(msg, span);
            }
        }
    }

    fn mark_moved(&mut self, id: usize, span: Span) {
        if let Some(s) = &mut self.state {
            s.insert(id, span);
        }
    }

    fn is_move(&self, t: &Type) -> bool {
        self.tc.is_move_type(t)
    }

    fn place_use(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Var(n) => {
                if let Some(id) = self.lookup(n) {
                    self.check_alive(id, e.span);
                }
            }
            ExprKind::Field { base, .. } => self.place_use(base),
            ExprKind::Index { base, index } => {
                self.place_use(base);
                self.value(index);
            }
            ExprKind::Unary {
                op: UnaryOp::Deref,
                operand,
            } => self.value(operand),
            _ => self.value(e),
        }
    }

    fn show(e: &Expr) -> String {
        match &e.kind {
            ExprKind::Var(n) => n.clone(),
            ExprKind::Field { base, name } => format!("{}.{name}", Self::show(base)),
            ExprKind::Index { base, .. } => format!("{}[_]", Self::show(base)),
            ExprKind::Unary {
                op: UnaryOp::Deref,
                operand,
            } => format!("*{}", Self::show(operand)),
            _ => "this value".into(),
        }
    }

    fn owned_root(&self, e: &Expr) -> Option<usize> {
        match &e.kind {
            ExprKind::Var(n) => self.lookup(n),
            ExprKind::Field { base, .. } => self.owned_root(base),
            _ => None,
        }
    }

    fn value(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Var(n) => {
                if let Some(id) = self.lookup(n) {
                    self.check_alive(id, e.span);
                    if self.is_move(e.r#type()) {
                        self.mark_moved(id, e.span);
                    }
                }
            }
            ExprKind::Field { .. }
            | ExprKind::Index { .. }
            | ExprKind::Unary {
                op: UnaryOp::Deref, ..
            } => {
                self.place_use(e);
                if self.is_move(e.r#type()) {
                    if let Some(id) = self.owned_root(e) {
                        self.mark_moved(id, e.span);
                        return;
                    }
                }
                if self.is_move(e.r#type()) {
                    let msg = format!(
                        "cannot move out of '{}' ({} owns a resource): borrow it, or use a method that takes it",
                        Self::show(e),
                        self.tc.type_name(e.r#type())
                    );
                    self.fail(msg, e.span);
                }
            }
            ExprKind::Unary {
                op: UnaryOp::Ref(_),
                operand,
            } => self.place_use(operand),
            ExprKind::Range { base, start, end } => {
                self.place_use(base);
                for x in [start, end].into_iter().flatten() {
                    self.value(x);
                }
            }
            ExprKind::Unary { operand, .. } => self.value(operand),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.value(lhs);
                self.value(rhs);
            }
            ExprKind::Cast { expr, .. } => self.value(expr),
            ExprKind::Call { callee, args, .. } => {
                self.value(callee);
                for a in args {
                    self.value(a);
                }
            }
            ExprKind::StructLit { fields, .. } => {
                for (_, v, _) in fields {
                    self.value(v);
                }
            }
            ExprKind::ArrayLit(items) => {
                for i in items {
                    self.value(i);
                }
            }
            ExprKind::ArrayRepeat { value, .. } => {
                if self.is_move(value.r#type()) {
                    let msg = format!(
                        "cannot repeat a {} value ('[x; n]' would copy it)",
                        self.tc.type_name(value.r#type())
                    );
                    self.fail(msg, e.span);
                }
                self.value(value);
            }
            ExprKind::VariantLit { args, .. } => {
                for a in args {
                    self.value(a);
                }
            }
            ExprKind::Unsize(x) => self.value(x),
            ExprKind::TryOp { inner, .. } => self.value(inner),
            _ => {}
        }
    }

    fn scrutinee(&mut self, e: &Expr, pats: &[&Pattern]) {
        if pats.iter().any(|p| self.binds_move(p)) {
            self.value(e);
        } else {
            match &e.kind {
                ExprKind::Var(_)
                | ExprKind::Field { .. }
                | ExprKind::Index { .. }
                | ExprKind::Unary {
                    op: UnaryOp::Deref, ..
                } => self.place_use(e),
                _ => self.value(e),
            }
        }
    }

    fn binds_move(&self, p: &Pattern) -> bool {
        match &p.kind {
            PatKind::Bind { by_ref: None, .. } => {
                p.r#type.as_ref().is_some_and(|t| self.is_move(t))
            }
            PatKind::Ctor { args, .. } => args.iter().any(|a| self.binds_move(a)),
            _ => false,
        }
    }

    fn bind(&mut self, p: &Pattern) {
        match &p.kind {
            PatKind::Bind { name, .. } => self.declare(name),
            PatKind::Ctor { args, .. } => {
                for a in args {
                    self.bind(a);
                }
            }
            _ => {}
        }
    }

    fn block(&mut self, b: &Block) {
        self.scopes.push(Vec::new());
        for s in &b.stmts {
            self.stmt(s);
        }
        self.scopes.pop();
    }

    fn run_loop(&mut self, f: &mut dyn FnMut(&mut Self)) -> (Moved, Moved) {
        let entry = self.state.clone();
        self.quiet += 1;
        self.loops.push(LoopCtx {
            breaks: None,
            continues: None,
        });
        f(self);
        let first = self.loops.pop().unwrap();
        self.quiet -= 1;
        let back = union(self.state.take(), first.continues);
        self.state = union(entry, back);
        let head = self.state.clone();
        self.loops.push(LoopCtx {
            breaks: None,
            continues: None,
        });
        f(self);
        let ctx = self.loops.pop().unwrap();
        (head, ctx.breaks)
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let { name, value, .. } => {
                if let Some(v) = value {
                    self.value(v);
                }
                self.declare(name);
            }
            Stmt::Assign {
                target, op, value, ..
            } => {
                self.value(value);
                match (&target.kind, op) {
                    (ExprKind::Var(n), None) => {
                        if let Some(id) = self.lookup(n) {
                            if let Some(st) = &mut self.state {
                                st.remove(&id);
                            }
                        }
                    }
                    _ => self.place_use(target),
                }
            }
            Stmt::Expr(e) => {
                self.value(e);
                if self.tc.expr_diverges(e) {
                    self.state = None;
                }
            }
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    self.value(v);
                }
                self.state = None;
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                self.value(cond);
                let before = self.state.clone();
                self.block(then_block);
                let after_then = self.state.take();
                self.state = before;
                if let Some(b) = else_block {
                    self.block(b);
                }
                self.state = union(after_then, self.state.take());
            }
            Stmt::While { cond, body, .. } => {
                let (head, breaks) = self.run_loop(&mut |m: &mut Self| {
                    m.value(cond);
                    m.block(body);
                });
                self.state = union(head, breaks);
            }
            Stmt::Loop { body, .. } => {
                let (_, breaks) = self.run_loop(&mut |m: &mut Self| m.block(body));
                self.state = breaks;
            }
            Stmt::For {
                var,
                start,
                end,
                body,
                ..
            } => {
                self.value(start);
                self.value(end);
                let (head, breaks) = self.run_loop(&mut |m: &mut Self| {
                    m.scopes.push(Vec::new());
                    m.declare(var);
                    m.block(body);
                    m.scopes.pop();
                });
                self.state = union(head, breaks);
            }
            Stmt::Break(_) => {
                let st = self.state.take();
                if let Some(l) = self.loops.last_mut() {
                    l.breaks = union(l.breaks.take(), st);
                }
            }
            Stmt::Continue(_) => {
                let st = self.state.take();
                if let Some(l) = self.loops.last_mut() {
                    l.continues = union(l.continues.take(), st);
                }
            }
            Stmt::Match {
                scrutinee, arms, ..
            } => {
                let pats: Vec<&Pattern> = arms.iter().flat_map(|a| a.pats.iter()).collect();
                self.scrutinee(scrutinee, &pats);
                let before = self.state.clone();
                let mut out = None;
                for arm in arms {
                    self.state = before.clone();
                    self.scopes.push(Vec::new());
                    self.bind(&arm.pats[0]);
                    if let Some(g) = &arm.guard {
                        self.value(g);
                    }
                    self.block(&arm.body);
                    self.scopes.pop();
                    out = union(out, self.state.take());
                }
                self.state = if arms.is_empty() { before } else { out };
            }
            Stmt::IfLet {
                pat,
                value,
                then_block,
                else_block,
                ..
            } => {
                self.scrutinee(value, &[pat]);
                let before = self.state.clone();
                self.scopes.push(Vec::new());
                self.bind(pat);
                self.block(then_block);
                self.scopes.pop();
                let after_then = self.state.take();
                self.state = before;
                if let Some(b) = else_block {
                    self.block(b);
                }
                self.state = union(after_then, self.state.take());
            }

            Stmt::LetElse {
                pat,
                value,
                else_block,
                ..
            } => {
                self.scrutinee(value, &[pat]);
                let before = self.state.clone();
                self.block(else_block);
                self.state = before;
                self.bind(pat);
            }
            Stmt::WhileLet {
                pat, value, body, ..
            } => {
                let (head, breaks) = self.run_loop(&mut |m: &mut Self| {
                    m.scrutinee(value, &[pat]);
                    m.scopes.push(Vec::new());
                    m.bind(pat);
                    m.block(body);
                    m.scopes.pop();
                });
                self.state = union(head, breaks);
            }
            Stmt::Block(b) => self.block(b),
        }
    }
}

impl TypeChecker {
    pub(crate) fn is_move_type(&self, t: &Type) -> bool {
        match t {
            Type::Struct(id) => {
                let def = &self.struct_defs[*id];
                has_attr(&def.attrs, "move")
                    || matches!(&self.layouts[*id], super::State::Done(l) if l.fields.iter().any(|f| self.is_move_type(&f.r#type)))
            }
            Type::Sum(id) => match &self.sums[*id] {
                super::State::Done(l) => l
                    .variants
                    .iter()
                    .any(|v| v.fields.iter().any(|(t, _)| self.is_move_type(t))),
                _ => false,
            },
            Type::Array(e, _) => self.is_move_type(e),
            _ => false,
        }
    }

    pub(super) fn move_check(&self, f: &Function) -> TypeResult<()> {
        let mut m = Mv {
            tc: self,
            names: Vec::new(),
            scopes: vec![Vec::new()],
            state: Some(HashMap::new()),
            loops: Vec::new(),
            quiet: 0,
            error: None,
        };
        for p in &f.params {
            m.declare(&p.name);
        }
        m.block(&f.body);
        match m.error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}
