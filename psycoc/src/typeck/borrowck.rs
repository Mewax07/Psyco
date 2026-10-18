use super::*;

#[derive(Debug, Clone, PartialEq)]
enum Proj {
    Field(String),
    Index,
    Deref,
}

#[derive(Debug, Clone)]
struct Place {
    root: usize,
    proj: Vec<Proj>,
}

struct LocalInfo {
    name: String,
    decl: usize,
    scope_end: Option<usize>,
    uses: Vec<usize>,
}

struct Loan {
    place: Place,
    mutable: bool,
    created: usize,
    activate: usize,
    temp_end: usize,
    holders: Vec<usize>,
    span: Span,
}

struct Access {
    place: Place,
    write: bool,
    moved: bool,
    point: usize,
    span: Span,
}

struct Bck<'a> {
    tc: &'a TypeChecker,
    locals: Vec<LocalInfo>,
    scopes: Vec<Vec<(String, usize)>>,
    loans: Vec<Loan>,
    accesses: Vec<Access>,
    loops: Vec<(usize, usize)>,
    point: usize,
}

impl<'a> Bck<'a> {
    fn tick(&mut self) -> usize {
        self.point += 1;
        self.point
    }

    fn lookup(&self, name: &str) -> Option<usize> {
        self.scopes
            .iter()
            .rev()
            .find_map(|s| s.iter().rev().find(|(n, _)| n == name).map(|(_, id)| *id))
    }

    fn declare(&mut self, name: &str) -> usize {
        let id = self.locals.len();
        let decl = self.tick();
        self.locals.push(LocalInfo {
            name: name.to_string(),
            decl,
            scope_end: None,
            uses: Vec::new(),
        });
        self.scopes.last_mut().unwrap().push((name.to_string(), id));
        id
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        let end = self.tick();
        for (_, id) in self.scopes.pop().unwrap() {
            self.locals[id].scope_end = Some(end);
        }
    }

    fn use_local(&mut self, id: usize) {
        let p = self.tick();
        self.locals[id].uses.push(p);
    }

    fn access(&mut self, place: Place, write: bool, span: Span) {
        self.access_kind(place, write, false, span);
    }

    fn access_kind(&mut self, place: Place, write: bool, moved: bool, span: Span) {
        let point = self.tick();
        self.accesses.push(Access {
            place,
            write,
            moved,
            point,
            span,
        });
    }

    fn new_loan(&mut self, place: Place, mutable: bool, span: Span) -> usize {
        let created = self.tick();
        self.loans.push(Loan {
            place,
            mutable,
            created,
            activate: created,
            temp_end: created,
            holders: Vec::new(),
            span,
        });
        self.loans.len() - 1
    }

    fn held_by(&self, id: usize) -> Vec<usize> {
        (0..self.loans.len())
            .filter(|&l| self.loans[l].holders.contains(&id))
            .collect()
    }

    fn hold(&mut self, loans: &[usize], holder: usize) {
        for &l in loans {
            if !self.loans[l].holders.contains(&holder) {
                self.loans[l].holders.push(holder);
            }
        }
    }

    fn extend_temp(&mut self, loans: &[usize], to: usize) {
        for &l in loans {
            self.loans[l].temp_end = self.loans[l].temp_end.max(to);
        }
    }

    // Places

    fn place(&mut self, e: &Expr) -> Option<Place> {
        match &e.kind {
            ExprKind::Var(n) => self.lookup(n).map(|root| Place {
                root,
                proj: Vec::new(),
            }),
            ExprKind::Field { base, name } => {
                let mut p = self.place(base)?;
                p.proj.push(Proj::Field(name.clone()));
                Some(p)
            }
            ExprKind::Index { base, index } => {
                let p = match base.r#type() {
                    Type::Slice(..) => self.deref_place(base),
                    _ => self.place(base),
                };
                self.expr(index);
                let mut p = p?;
                p.proj.push(Proj::Index);
                Some(p)
            }
            ExprKind::Unary {
                op: UnaryOp::Deref,
                operand,
            } => self.deref_place(operand),
            _ => {
                self.expr(e);
                None
            }
        }
    }

    fn deref_place(&mut self, e: &Expr) -> Option<Place> {
        if let ExprKind::Var(n) = &e.kind {
            if let Some(id) = self.lookup(n) {
                self.use_local(id);
                return Some(Place {
                    root: id,
                    proj: vec![Proj::Deref],
                });
            }
            return None;
        }
        self.expr(e);
        None
    }

    fn expr(&mut self, e: &Expr) -> Vec<usize> {
        let carries = self.tc.carries_ref(e.r#type());
        match &e.kind {
            ExprKind::Var(n) => {
                let Some(id) = self.lookup(n) else {
                    return Vec::new();
                };
                if carries {
                    self.use_local(id);
                    let mut out = self.held_by(id);
                    if matches!(e.r#type(), Type::Ref(_, true) | Type::Slice(_, true)) {
                        let l = self.new_loan(
                            Place {
                                root: id,
                                proj: vec![Proj::Deref],
                            },
                            true,
                            e.span,
                        );
                        out.push(l);
                    }
                    out
                } else {
                    self.use_local(id);
                    let write = self.tc.is_move_type(e.r#type());
                    self.access_kind(
                        Place {
                            root: id,
                            proj: Vec::new(),
                        },
                        write,
                        write,
                        e.span,
                    );
                    Vec::new()
                }
            }
            ExprKind::Unary {
                op: UnaryOp::Ref(m),
                operand,
            } => self.borrow(operand, *m, e.span),
            ExprKind::Range { .. } => {
                let m = matches!(e.r#type(), Type::Slice(_, true));
                self.borrow_range(e, m)
            }
            ExprKind::Field { .. }
            | ExprKind::Index { .. }
            | ExprKind::Unary {
                op: UnaryOp::Deref, ..
            } => {
                let write = self.tc.is_move_type(e.r#type());
                if let Some(p) = self.place(e) {
                    self.access_kind(p, write, write, e.span);
                }
                Vec::new()
            }
            ExprKind::Unary { operand, .. } => {
                self.expr(operand);
                Vec::new()
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
                Vec::new()
            }
            ExprKind::Cast { expr, .. } => {
                self.expr(expr);
                Vec::new()
            }
            ExprKind::Call { callee, args, .. } => {
                self.expr(callee);
                let mut carried = Vec::new();
                let mut all = Vec::new();
                for a in args {
                    let c = self.expr(a);
                    all.extend(c.iter().copied());
                    if self.tc.carries_ref(a.r#type()) {
                        carried.extend(c);
                    }
                }
                let call = self.tick();
                for &l in &all {
                    if self.loans[l].activate == self.loans[l].created {
                        self.loans[l].activate = call;
                    }
                }
                self.extend_temp(&all, call);
                if carries { carried } else { Vec::new() }
            }
            ExprKind::VariantLit { args, .. } => {
                let mut out = Vec::new();
                for a in args {
                    let c = self.expr(a);
                    if self.tc.carries_ref(a.r#type()) {
                        out.extend(c);
                    }
                }
                out
            }
            ExprKind::Unsize(inner) => self.expr(inner),
            ExprKind::TryOp { inner, .. } => {
                let c = self.expr(inner);
                if carries { c } else { Vec::new() }
            }
            ExprKind::StructLit { fields, .. } => {
                for (_, v, _) in fields {
                    self.expr(v);
                }
                Vec::new()
            }
            ExprKind::ArrayLit(items) => {
                for i in items {
                    self.expr(i);
                }
                Vec::new()
            }
            ExprKind::ArrayRepeat { value, .. } => {
                self.expr(value);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn borrow(&mut self, operand: &Expr, mutable: bool, span: Span) -> Vec<usize> {
        let deref_root = self.deref_root(operand);
        match self.place(operand) {
            Some(p) => {
                let l = self.new_loan(p, mutable, span);
                let mut out = vec![l];
                if let Some(r) = deref_root {
                    out.extend(self.held_by(r));
                }
                out
            }
            None => Vec::new(),
        }
    }

    fn borrow_range(&mut self, e: &Expr, mutable: bool) -> Vec<usize> {
        let ExprKind::Range { base, start, end } = &e.kind else {
            unreachable!()
        };
        let deref_root = match base.r#type() {
            Type::Slice(..) => self.var_root(base),
            _ => self.deref_root(base),
        };
        let p = match base.r#type() {
            Type::Slice(..) => self.deref_place(base),
            _ => self.place(base),
        };
        for x in [start, end].into_iter().flatten() {
            self.expr(x);
        }
        match p {
            Some(mut p) => {
                p.proj.push(Proj::Index);
                let l = self.new_loan(p, mutable, e.span);
                let mut out = vec![l];
                if let Some(r) = deref_root {
                    out.extend(self.held_by(r));
                }
                out
            }
            None => Vec::new(),
        }
    }

    fn var_root(&self, e: &Expr) -> Option<usize> {
        match &e.kind {
            ExprKind::Var(n) => self.lookup(n),
            _ => None,
        }
    }

    fn deref_root(&self, e: &Expr) -> Option<usize> {
        match &e.kind {
            ExprKind::Unary {
                op: UnaryOp::Deref,
                operand,
            } => self.var_root(operand),
            ExprKind::Field { base, .. } => self.deref_root(base),
            ExprKind::Index { base, .. } => match base.r#type() {
                Type::Slice(..) => self.var_root(base),
                _ => self.deref_root(base),
            },
            _ => None,
        }
    }

    // Statements

    fn block(&mut self, b: &Block) {
        self.push_scope();
        for s in &b.stmts {
            self.stmt(s);
        }
        self.pop_scope();
    }

    fn stmt_end(&mut self, carried: &[usize]) {
        let p = self.tick();
        self.extend_temp(carried, p);
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let {
                name,
                value,
                r#type,
                ..
            } => {
                let carried = match value {
                    Some(v) => self.expr(v),
                    None => Vec::new(),
                };
                let id = self.declare(name);
                if r#type.as_ref().is_some_and(|t| self.tc.carries_ref(t)) {
                    self.hold(&carried, id);
                }
                self.stmt_end(&carried);
            }
            Stmt::Assign {
                target, op, value, ..
            } => {
                let carried = self.expr(value);
                if let ExprKind::Var(n) = &target.kind {
                    if let Some(id) = self.lookup(n) {
                        if self.tc.carries_ref(target.r#type()) {
                            self.use_local(id);
                            self.hold(&carried, id);
                            self.stmt_end(&carried);
                            return;
                        }
                    }
                }
                if let Some(p) = self.place(target) {
                    if op.is_some() {
                        self.access(p.clone(), false, target.span);
                    }
                    self.access(p, true, target.span);
                }
                self.stmt_end(&carried);
            }
            Stmt::Expr(e) => {
                let c = self.expr(e);
                self.stmt_end(&c);
            }
            Stmt::Return { value, .. } => {
                let c = value.as_ref().map(|v| self.expr(v)).unwrap_or_default();
                self.stmt_end(&c);
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                let c = self.expr(cond);
                self.stmt_end(&c);
                self.block(then_block);
                if let Some(b) = else_block {
                    self.block(b);
                }
            }
            Stmt::While { cond, body, .. } => {
                let start = self.tick();
                let c = self.expr(cond);
                self.stmt_end(&c);
                self.block(body);
                let end = self.tick();
                self.loops.push((start, end));
            }
            Stmt::Loop { body, .. } => {
                let start = self.tick();
                self.block(body);
                let end = self.tick();
                self.loops.push((start, end));
            }
            Stmt::For {
                var,
                start,
                end,
                body,
                ..
            } => {
                self.expr(start);
                self.expr(end);
                let lstart = self.tick();
                self.push_scope();
                self.declare(var);
                self.block(body);
                self.pop_scope();
                let lend = self.tick();
                self.loops.push((lstart, lend));
            }
            Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::Match {
                scrutinee, arms, ..
            } => {
                let carried = self.expr(scrutinee);
                for arm in arms {
                    self.push_scope();
                    self.bind_pattern(&arm.pats[0], &carried);
                    if let Some(g) = &arm.guard {
                        let c = self.expr(g);
                        self.stmt_end(&c);
                    }
                    self.block(&arm.body);
                    self.pop_scope();
                }
                self.stmt_end(&carried);
            }
            Stmt::IfLet {
                pat,
                value,
                then_block,
                else_block,
                ..
            } => {
                let carried = self.expr(value);
                self.push_scope();
                self.bind_pattern(pat, &carried);
                self.block(then_block);
                self.pop_scope();
                self.stmt_end(&carried);
                if let Some(b) = else_block {
                    self.block(b);
                }
            }
            Stmt::LetElse {
                pat,
                value,
                else_block,
                ..
            } => {
                let carried = self.expr(value);
                self.block(else_block);
                self.bind_pattern(pat, &carried);
                self.stmt_end(&carried);
            }
            Stmt::WhileLet {
                pat, value, body, ..
            } => {
                let start = self.tick();
                let carried = self.expr(value);
                self.push_scope();
                self.bind_pattern(pat, &carried);
                self.block(body);
                self.pop_scope();
                self.stmt_end(&carried);
                let end = self.tick();
                self.loops.push((start, end));
            }
            Stmt::Block(b) => self.block(b),
        }
    }

    fn bind_pattern(&mut self, p: &Pattern, carried: &[usize]) {
        match &p.kind {
            PatKind::Bind { name, by_ref, .. } => {
                let id = self.declare(name);
                let looked = p.r#type.clone().expect("ICE: pattern not typed");
                if by_ref.is_some() || self.tc.carries_ref(&looked) {
                    self.hold(carried, id);
                }
            }
            PatKind::Ctor { args, .. } => {
                for a in args {
                    self.bind_pattern(a, carried);
                }
            }
            _ => {}
        }
    }

    // Checks

    fn loop_extend(&self, u: usize, decl: usize) -> (usize, usize) {
        let mut start = u;
        let mut end = u;
        for &(s, e) in &self.loops {
            if s <= u && u <= e && !(s <= decl && decl <= e) {
                start = start.min(s);
                end = end.max(e);
            }
        }
        (start, end)
    }

    fn range(&self, l: &Loan) -> (usize, usize) {
        let mut start = l.created;
        let mut end = l.temp_end;
        for &h in &l.holders {
            let info = &self.locals[h];
            for &u in &info.uses {
                let (s, e) = self.loop_extend(u, info.decl);
                if u >= l.created || s <= l.created {
                    end = end.max(e);
                    if s <= l.created {
                        start = start.min(s);
                    }
                }
            }
            let (s, e) = self.loop_extend(l.created, info.decl);
            if s < l.created && info.uses.iter().any(|&u| s <= u && u <= e) {
                start = start.min(s);
                end = end.max(e);
            }
        }
        (start, end)
    }

    fn overlap(a: &Place, b: &Place) -> bool {
        if a.root != b.root {
            return false;
        }
        for (x, y) in a.proj.iter().zip(&b.proj) {
            match (x, y) {
                (Proj::Field(f), Proj::Field(g)) if f != g => return false,
                _ => {}
            }
        }
        true
    }

    fn show(&self, p: &Place) -> String {
        let mut s = self.locals[p.root].name.clone();
        let mut deref = false;
        for x in &p.proj {
            match x {
                Proj::Field(f) => {
                    if deref {
                        s = format!("(*{s})");
                        deref = false;
                    }
                    s.push('.');
                    s.push_str(f);
                }
                Proj::Index => {
                    if deref {
                        s = format!("(*{s})");
                        deref = false;
                    }
                    s.push_str("[_]");
                }
                Proj::Deref => {
                    if deref {
                        s = format!("*{s}");
                    }
                    deref = true;
                }
            }
        }
        if deref {
            s = format!("*{s}");
        }
        s
    }

    fn check(&self) -> TypeResult<()> {
        let ranges: Vec<(usize, usize)> = self.loans.iter().map(|l| self.range(l)).collect();
        for (i, l) in self.loans.iter().enumerate() {
            let (start, end) = ranges[i];
            let kind = if l.mutable { "mutable" } else { "shared" };
            if let Some(dead) = self.locals[l.place.root].scope_end {
                if end > dead {
                    return self.tc.err(
                        format!(
                            "'{}' does not live long enough: it is borrowed here and the borrow is still used after the end of its block",
                            self.locals[l.place.root].name
                        ),
                        l.span,
                    );
                }
            }
            for a in &self.accesses {
                if a.point <= start || a.point > end || !Self::overlap(&l.place, &a.place) {
                    continue;
                }
                if a.point < l.created {
                    continue;
                }
                if !a.write && a.point < l.activate {
                    continue;
                }
                if l.mutable || a.write {
                    let what = if a.moved {
                        "move"
                    } else if a.write {
                        "assign to"
                    } else {
                        "use"
                    };
                    return self.tc.err(
                        format!(
                            "cannot {what} '{}' while it is borrowed ({kind} borrow at line {}, still used later)",
                            self.show(&a.place),
                            l.span.line
                        ),
                        a.span,
                    );
                }
            }
            for (j, m) in self.loans.iter().enumerate() {
                if i == j || m.created <= start || m.created > end || m.created < l.created {
                    continue;
                }
                if !(l.mutable || m.mutable) || !Self::overlap(&l.place, &m.place) {
                    continue;
                }
                if !m.mutable && m.created < l.activate && ranges[j].1 < l.activate {
                    continue;
                }
                let how = if m.mutable { "as mutable" } else { "" };
                return self.tc.err(
                    format!(
                        "cannot borrow '{}' {how}{}while it is borrowed ({kind} borrow at line {}, still used later)",
                        self.show(&m.place),
                        if how.is_empty() { "" } else { " " },
                        l.span.line
                    ),
                    m.span,
                );
            }
        }
        Ok(())
    }
}

impl TypeChecker {
    pub(super) fn borrow_check(&self, f: &Function) -> TypeResult<()> {
        let mut b = Bck {
            tc: self,
            locals: Vec::new(),
            scopes: vec![Vec::new()],
            loans: Vec::new(),
            accesses: Vec::new(),
            loops: Vec::new(),
            point: 0,
        };
        for p in &f.params {
            b.declare(&p.name);
        }
        b.block(&f.body);
        b.check()
    }
}
