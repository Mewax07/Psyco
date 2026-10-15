use std::{collections::HashSet, mem::take};

use crate::*;

fn qualify(module: &str, name: &str) -> String {
    if module.is_empty() {
        name.to_string()
    } else {
        format!("{module}::{name}")
    }
}

fn parent(m: &str) -> &str {
    m.rsplit_once("::").map_or("", |(p, _)| p)
}

struct Resolver {
    modules: HashSet<String>,
    types: HashSet<String>,
    values: HashSet<String>,
    consts: HashSet<String>,
    /// Context of the item being rewritten.
    module: String,
    type_params: Vec<String>,
    locals: Vec<HashSet<String>>,
}

pub fn resolve(program: &mut Program) -> Result<(), TypeError> {
    if program.modules.is_empty() {
        return Ok(());
    }
    let mut r = Resolver {
        modules: HashSet::new(),
        types: HashSet::new(),
        values: HashSet::new(),
        consts: HashSet::new(),
        module: String::new(),
        type_params: Vec::new(),
        locals: Vec::new(),
    };
    for m in &program.modules {
        let mut p = m.as_str();
        while !p.is_empty() {
            r.modules.insert(p.to_string());
            p = parent(p);
        }
    }

    // Full names of the types.
    let rename_type = |module: &str, name: &mut String, types: &mut HashSet<String>| {
        *name = qualify(module, name);
        types.insert(name.clone());
    };
    for s in &mut program.structs {
        rename_type(&s.module, &mut s.name, &mut r.types);
    }
    for e in &mut program.enums {
        rename_type(&e.module, &mut e.name, &mut r.types);
    }
    for t in &mut program.traits {
        rename_type(&t.module, &mut t.name, &mut r.types);
    }
    for s in &program.structs {
        if r.modules.contains(&s.name) {
            return Err(TypeError {
                message: format!("'{}' is both a module and a type", s.name),
                span: s.span,
            });
        }
    }

    // Full names of the values. `T::method` (from a plain impl) is named
    // after the full name of T.
    for f in &mut program.functions {
        r.module = f.module.clone();
        if let Some((owner, method)) = f.name.split_once("::") {
            if let Some(t) = r.find(owner, &r.types) {
                f.name = format!("{t}::{method}");
            }
        } else {
            f.name = qualify(&f.module, &f.name);
            r.values.insert(f.name.clone());
        }
    }
    for s in &mut program.statics {
        s.name = qualify(&s.module, &s.name);
        r.values.insert(s.name.clone());
    }
    for c in &mut program.consts {
        c.name = qualify(&c.module, &c.name);
        r.values.insert(c.name.clone());
        r.consts.insert(c.name.clone());
    }

    // References.
    for s in &mut program.structs {
        r.enter(&s.module, &s.generics);
        r.bounds(&mut s.generics);
        for f in &mut s.fields {
            r.ty(&mut f.r#type);
        }
    }
    for e in &mut program.enums {
        r.enter(&e.module, &e.generics);
        r.bounds(&mut e.generics);
        if let Some(t) = &mut e.repr {
            r.ty(t);
        }
        for v in &mut e.variants {
            for t in v.fields.iter_mut().flatten() {
                r.ty(t);
            }
            if let Some(x) = &mut v.value {
                r.expr(x);
            }
        }
    }
    for s in &mut program.statics {
        r.enter(&s.module, &[]);
        r.ty(&mut s.r#type);
        if let Some(x) = &mut s.init {
            r.expr(x);
        }
    }
    for c in &mut program.consts {
        r.enter(&c.module, &[]);
        if let Some(t) = &mut c.r#type {
            r.ty(t);
        }
        r.expr(&mut c.value);
    }
    for f in &mut program.functions {
        let module = f.module.clone();
        r.enter(&module, &[]);
        r.function(f, &[]);
    }
    for t in &mut program.traits {
        let module = t.module.clone();
        for (m, _) in &mut t.methods {
            r.enter(&module, &[]);
            r.function(m, &[]);
        }
    }
    for imp in &mut program.impls {
        r.enter(&imp.module, &imp.generics);
        r.bounds(&mut imp.generics);
        r.ty(&mut imp.self_ty);
        if let Some(t) = &mut imp.trait_name {
            if let Some(full) = r.find(t, &r.types) {
                *t = full;
            }
        }
        let outer: Vec<String> = imp.generics.iter().map(|g| g.name.clone()).collect();
        for m in &mut imp.methods {
            r.enter(&imp.module, &imp.generics);
            r.function(m, &outer);
        }
    }
    Ok(())
}

impl Resolver {
    fn enter(&mut self, module: &str, generics: &[GenericParam]) {
        self.module = module.to_string();
        self.type_params = generics.iter().map(|g| g.name.clone()).collect();
        self.locals = vec![HashSet::new()];
    }

    /// Full name of `name` seen from the current module, if `set` has it.
    fn find(&self, name: &str, set: &HashSet<String>) -> Option<String> {
        let mut m = self.module.as_str();
        loop {
            let c = qualify(m, name);
            if set.contains(&c) {
                return Some(c);
            }
            if m.is_empty() {
                return None;
            }
            m = parent(m);
        }
    }

    fn is_local(&self, name: &str) -> bool {
        self.locals.iter().any(|s| s.contains(name))
    }

    fn bind(&mut self, name: &str) {
        self.locals.last_mut().unwrap().insert(name.to_string());
    }

    fn bounds(&mut self, generics: &mut [GenericParam]) {
        for g in generics {
            for b in &mut g.bounds {
                if let Some(full) = self.find(b, &self.types) {
                    *b = full;
                }
            }
        }
    }

    fn function(&mut self, f: &mut Function, outer: &[String]) {
        self.type_params.extend(outer.iter().cloned());
        self.type_params
            .extend(f.generics.iter().map(|g| g.name.clone()));
        self.bounds(&mut f.generics);
        for p in &mut f.params {
            self.ty(&mut p.r#type);
            let n = p.name.clone();
            self.bind(&n);
        }
        if let Some(t) = &mut f.return_type {
            self.ty(t);
        }
        self.block(&mut f.body);
    }

    fn type_name(&self, n: &mut String) {
        if n == "Self"
            || self.type_params.contains(n)
            || [
                "u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize", "bool",
                "Bool", "Int", "Str",
            ]
            .contains(&n.as_str())
        {
            return;
        }
        if let Some(full) = self.find(n, &self.types) {
            *n = full;
        }
    }

    fn ty(&mut self, t: &mut TypeExpr) {
        match &mut t.kind {
            TypeExprKind::Named(n) => self.type_name(n),
            TypeExprKind::Generic(n, args) => {
                self.type_name(n);
                for a in args {
                    self.ty(a);
                }
            }
            TypeExprKind::Ref { inner, .. } | TypeExprKind::Raw { inner, .. } => self.ty(inner),
            TypeExprKind::Slice { elem, .. } => self.ty(elem),
            TypeExprKind::Array(elem, count) => {
                self.ty(elem);
                self.expr(count);
            }
            TypeExprKind::Fn { params, ret, .. } => {
                for p in params {
                    self.ty(p);
                }
                if let Some(r) = ret {
                    self.ty(r);
                }
            }
            TypeExprKind::Unit | TypeExprKind::Resolved(_) => {}
        }
    }

    fn block(&mut self, b: &mut Block) {
        self.locals.push(HashSet::new());
        for s in &mut b.stmts {
            self.stmt(s);
        }
        self.locals.pop();
    }

    fn stmt(&mut self, s: &mut Stmt) {
        match s {
            Stmt::Let {
                name,
                expr_type,
                value,
                ..
            } => {
                if let Some(t) = expr_type {
                    self.ty(t);
                }
                if let Some(v) = value {
                    self.expr(v);
                }
                let n = name.clone();
                self.bind(&n);
            }
            Stmt::Assign { target, value, .. } => {
                self.expr(target);
                self.expr(value);
            }
            Stmt::Expr(e) => self.expr(e),
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    self.expr(v);
                }
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                self.expr(cond);
                self.block(then_block);
                if let Some(b) = else_block {
                    self.block(b);
                }
            }
            Stmt::While { cond, body, .. } => {
                self.expr(cond);
                self.block(body);
            }
            Stmt::Loop { body, .. } => self.block(body),
            Stmt::For {
                var,
                start,
                end,
                body,
                ..
            } => {
                self.expr(start);
                self.expr(end);
                self.locals.push(HashSet::new());
                let v = var.clone();
                self.bind(&v);
                self.block(body);
                self.locals.pop();
            }
            Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::Match {
                scrutinee, arms, ..
            } => {
                self.expr(scrutinee);
                for arm in arms {
                    self.locals.push(HashSet::new());
                    for p in &mut arm.pats {
                        self.pattern(p);
                    }
                    if let Some(g) = &mut arm.guard {
                        self.expr(g);
                    }
                    self.block(&mut arm.body);
                    self.locals.pop();
                }
            }
            Stmt::IfLet {
                pat,
                value,
                then_block,
                else_block,
                ..
            } => {
                self.expr(value);
                self.locals.push(HashSet::new());
                self.pattern(pat);
                self.block(then_block);
                self.locals.pop();
                if let Some(b) = else_block {
                    self.block(b);
                }
            }
            Stmt::WhileLet {
                pat, value, body, ..
            } => {
                self.expr(value);
                self.locals.push(HashSet::new());
                self.pattern(pat);
                self.block(body);
                self.locals.pop();
            }
            Stmt::Block(b) => self.block(b),
        }
    }

    fn pattern(&mut self, p: &mut Pattern) {
        let span = p.span;
        match &mut p.kind {
            PatKind::Ident(n) => {
                if !self.is_local(n) {
                    if let Some(c) = self.find(n, &self.consts) {
                        if c != *n {
                            p.kind = PatKind::Lit(Box::new(Expr::new(ExprKind::Var(c), span)));
                            return;
                        }
                    }
                }
                let n = n.clone();
                self.bind(&n);
            }
            PatKind::MutBind(n) => {
                let n = n.clone();
                self.bind(&n);
            }
            PatKind::Path(owner, member) => {
                if let Some(m) = self.find(owner, &self.modules) {
                    let full = qualify(&m, member);
                    p.kind = PatKind::Lit(Box::new(Expr::new(ExprKind::Var(full), span)));
                } else {
                    self.type_name(owner);
                }
            }
            PatKind::Variant { owner, args, .. } => {
                if let Some(o) = owner {
                    self.type_name(o);
                }
                for a in args {
                    self.pattern(a);
                }
            }
            PatKind::Or(alts) => {
                for a in alts {
                    self.pattern(a);
                }
            }
            PatKind::Lit(e) => self.expr(e),
            _ => {}
        }
    }

    fn expr(&mut self, e: &mut Expr) {
        match &mut e.kind {
            ExprKind::Var(n) => {
                if !self.is_local(n) {
                    if let Some(full) = self.find(n, &self.values) {
                        *n = full;
                    }
                }
            }
            ExprKind::Path(owner, member) => {
                if owner == "Self" || self.type_params.contains(owner) {
                    return;
                }
                if let Some(m) = self.find(owner, &self.modules) {
                    e.kind = ExprKind::Var(qualify(&m, member));
                } else {
                    self.type_name(owner);
                }
            }
            ExprKind::Turbofish {
                owner,
                owner_args,
                member,
                member_args,
            } => {
                for a in owner_args.iter_mut().chain(member_args.iter_mut()) {
                    self.ty(a);
                }
                match member {
                    Some(mb) => {
                        if let Some(m) = self.find(owner, &self.modules) {
                            e.kind = ExprKind::Turbofish {
                                owner: qualify(&m, mb),
                                owner_args: take(member_args),
                                member: None,
                                member_args: Vec::new(),
                            };
                        } else {
                            self.type_name(owner);
                        }
                    }
                    None => {
                        if let Some(full) = self.find(owner, &self.values) {
                            *owner = full;
                        }
                    }
                }
            }
            ExprKind::TypePath {
                ty, member_args, ..
            } => {
                self.ty(ty);
                for a in member_args {
                    self.ty(a);
                }
            }
            ExprKind::StructLit { name, args, fields } => {
                self.type_name(name);
                for a in args {
                    self.ty(a);
                }
                for (_, v, _) in fields {
                    self.expr(v);
                }
            }
            ExprKind::Unary { operand, .. } => self.expr(operand),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::Cast { expr, r#type } => {
                self.expr(expr);
                self.ty(r#type);
            }
            ExprKind::Call { callee, args, .. } => {
                self.expr(callee);
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::Field { base, .. } => self.expr(base),
            ExprKind::Index { base, index } => {
                self.expr(base);
                self.expr(index);
            }
            ExprKind::ArrayLit(items) => {
                for i in items {
                    self.expr(i);
                }
            }
            ExprKind::ArrayRepeat { value, count } => {
                self.expr(value);
                self.expr(count);
            }
            ExprKind::Sizeof(t) => self.ty(t),
            ExprKind::Range { base, start, end } => {
                self.expr(base);
                if let Some(s) = start {
                    self.expr(s);
                }
                if let Some(x) = end {
                    self.expr(x);
                }
            }
            ExprKind::Unsize(inner) | ExprKind::Try(inner) | ExprKind::TryOp { inner, .. } => {
                self.expr(inner)
            }
            ExprKind::VariantLit { args, .. } => {
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::Int(_)
            | ExprKind::TypedInt(..)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::CStr(_)
            | ExprKind::Bytes(_)
            | ExprKind::WStr(_) => {}
        }
    }
}
