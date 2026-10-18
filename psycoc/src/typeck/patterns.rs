use std::collections::HashMap;

use super::*;

pub(super) struct PatBinding {
    name: String,
    local: Local,
}

#[derive(Debug, Clone, PartialEq)]
enum Ctor {
    Variant(usize),
    Value(i128),
}

#[derive(Debug, Clone)]
enum DPat {
    Wild,
    Ctor(Ctor, Vec<DPat>),
    Or(Vec<DPat>),
}

impl TypeChecker {
    fn check_matchable(&self, t: &Type, span: Span) -> TypeResult<()> {
        let base = match t {
            Type::Ref(inner, _) => &**inner,
            t => t,
        };
        if matches!(
            base,
            Type::Int(_) | Type::Bool | Type::Enum(_) | Type::Sum(_)
        ) {
            Ok(())
        } else {
            self.err(
                format!("cannot match on a value of type {}", self.type_name(t)),
                span,
            )
        }
    }

    fn scrutinee_origin(&self, e: &Expr) -> u8 {
        if self.carries_ref(e.r#type()) {
            self.origin(e)
        } else {
            0
        }
    }

    pub(super) fn check_match(
        &mut self,
        scrutinee: &mut Expr,
        arms: &mut [MatchArm],
    ) -> TypeResult<bool> {
        let st = self.check_expr(scrutinee, None)?;
        self.check_matchable(&st, scrutinee.span)?;
        let origin = self.scrutinee_origin(scrutinee);
        let tys = vec![st.clone()];
        let mut rows: Vec<Vec<DPat>> = Vec::new();
        let mut all_diverge = true;
        for arm in arms.iter_mut() {
            let mut bindings = Vec::new();
            let alternatives = arm.pats.len();
            for p in arm.pats.iter_mut() {
                let mut b = Vec::new();
                self.check_pattern(p, &st, None, origin, &mut b)?;
                if alternatives > 1 && !b.is_empty() {
                    return self.err("patterns separated by '|' cannot bind names", p.span);
                }
                bindings = b;
            }
            for p in &arm.pats {
                let d = vec![self.deconstruct(p)];
                if !self.useful(&rows, &d, &tys) {
                    return self.err(
                        "this pattern is already covered by the arms above (it is never reached)",
                        p.span,
                    );
                }
                if arm.guard.is_none() {
                    rows.push(d);
                }
            }
            self.ctx.scopes.push(HashMap::new());
            for b in bindings {
                self.declare_local(&b.name, b.local);
            }
            let r = (|| -> TypeResult<bool> {
                if let Some(g) = &mut arm.guard {
                    self.coerce(g, &Type::Bool)?;
                }
                self.check_block(&mut arm.body)
            })();
            self.ctx.scopes.pop();
            all_diverge &= r?;
        }
        if let Some(w) = self.missing(&rows, &tys) {
            return self.err(
                format!(
                    "this match does not cover every value: {} is missing (add an arm or '_ =>')",
                    self.show_witness(&w[0], &st)
                ),
                scrutinee.span,
            );
        }
        Ok(all_diverge && !arms.is_empty())
    }

    pub(super) fn check_if_let(
        &mut self,
        pat: &mut Pattern,
        value: &mut Expr,
        then_block: &mut Block,
        else_block: &mut Option<Block>,
    ) -> TypeResult<bool> {
        let vt = self.check_expr(value, None)?;
        self.check_matchable(&vt, value.span)?;
        let origin = self.scrutinee_origin(value);
        let mut bindings = Vec::new();
        self.check_pattern(pat, &vt, None, origin, &mut bindings)?;
        self.ctx.scopes.push(HashMap::new());
        for b in bindings {
            self.declare_local(&b.name, b.local);
        }
        let t = self.check_block(then_block);
        self.ctx.scopes.pop();
        let t = t?;
        let e = match else_block {
            Some(b) => self.check_block(b)?,
            None => false,
        };
        Ok(t && e)
    }

    pub(super) fn check_let_else(
        &mut self,
        pat: &mut Pattern,
        value: &mut Expr,
        else_block: &mut Block,
        span: Span,
    ) -> TypeResult<bool> {
        let vt = self.check_expr(value, None)?;
        self.check_matchable(&vt, value.span)?;
        let origin = self.scrutinee_origin(value);
        let mut bindings = Vec::new();
        self.check_pattern(pat, &vt, None, origin, &mut bindings)?;
        if !self.check_block(else_block)? {
            return self.err(
                "the 'else' block of 'let PATTERN = VALUE else' must leave (return, break, continue, panic or exit)",
                span,
            );
        }
        for b in bindings {
            self.declare_local(&b.name, b.local);
        }
        Ok(false)
    }

    pub(super) fn check_while_let(
        &mut self,
        pat: &mut Pattern,
        value: &mut Expr,
        body: &mut Block,
    ) -> TypeResult<bool> {
        let vt = self.check_expr(value, None)?;
        self.check_matchable(&vt, value.span)?;
        let origin = self.scrutinee_origin(value);
        let mut bindings = Vec::new();
        self.check_pattern(pat, &vt, None, origin, &mut bindings)?;
        self.ctx.scopes.push(HashMap::new());
        for b in bindings {
            self.declare_local(&b.name, b.local);
        }
        self.ctx.loops.push(false);
        let r = self.check_block(body);
        self.ctx.loops.pop();
        self.ctx.scopes.pop();
        r?;
        Ok(false)
    }

    fn check_pattern(
        &mut self,
        pat: &mut Pattern,
        ty: &Type,
        mode: Option<bool>,
        origin: u8,
        bindings: &mut Vec<PatBinding>,
    ) -> TypeResult<()> {
        let span = pat.span;
        let looked_at = match ty {
            Type::Ref(inner, _) => (**inner).clone(),
            t => t.clone(),
        };

        if let PatKind::Ident(name) = &pat.kind {
            let name = name.clone();
            let mut kind = None;
            if let Type::Sum(id) = looked_at {
                if self.variant_index(id, &name).is_some() {
                    kind = Some(PatKind::Variant {
                        owner: None,
                        name: name.clone(),
                        args: Vec::new(),
                    });
                }
            }
            if kind.is_none() {
                if let Type::Enum(eid) = looked_at {
                    if self.enum_info(eid).variants.iter().any(|(n, _)| *n == name) {
                        let owner = self.enum_defs[eid].name.clone();
                        kind = Some(PatKind::Path(owner, name.clone()));
                    }
                }
            }
            if kind.is_none()
                && self.consts.contains_key(&name)
                && self.lookup_local(&name).is_none()
            {
                kind = Some(PatKind::Lit(Box::new(Expr::new(
                    ExprKind::Var(name.clone()),
                    span,
                ))));
            }
            pat.kind = kind.unwrap_or(PatKind::Bind {
                name,
                mutable: false,
                by_ref: None,
            });
        }
        if let PatKind::MutBind(name) = &pat.kind {
            pat.kind = PatKind::Bind {
                name: name.clone(),
                mutable: true,
                by_ref: None,
            };
        }

        match &mut pat.kind {
            PatKind::Wild => {
                pat.r#type = Some(ty.clone());
                return Ok(());
            }
            PatKind::Or(alts) => {
                for a in alts.iter_mut() {
                    let mut b = Vec::new();
                    self.check_pattern(a, ty, mode, origin, &mut b)?;
                    if !b.is_empty() {
                        return self.err("patterns separated by '|' cannot bind names", a.span);
                    }
                }
                pat.r#type = Some(ty.clone());
                return Ok(());
            }
            PatKind::Bind {
                name,
                mutable,
                by_ref,
            } => {
                *by_ref = if *mutable { None } else { mode };
                let bty = match *by_ref {
                    Some(m) => Type::Ref(Box::new(ty.clone()), m),
                    None => ty.clone(),
                };
                if by_ref.is_some() && self.contains_ref(ty) {
                    return self.err(
                        "references to values containing references are not supported (match on the value itself)",
                        span,
                    );
                }
                if bindings.iter().any(|b| b.name == *name) {
                    return self.err(format!("'{name}' is bound twice in this pattern"), span);
                }
                let local = Local {
                    origin: if self.carries_ref(&bty) { origin } else { 0 },
                    r#type: bty,
                    mutable: *mutable,
                };
                bindings.push(PatBinding {
                    name: name.clone(),
                    local,
                });
                pat.r#type = Some(ty.clone());
                return Ok(());
            }
            _ => {}
        }

        let mut ty = ty.clone();
        let mut mode = mode;
        if let Type::Ref(inner, m) = &ty {
            pat.deref = true;
            mode = Some(mode.map_or(*m, |old| old && *m));
            ty = (**inner).clone();
        }
        let kind = std::mem::replace(&mut pat.kind, PatKind::Wild);
        pat.kind = match kind {
            PatKind::Path(owner, member) => {
                if let Some(id) = self.pattern_owner(&owner, &ty) {
                    let Some(vi) = self.variant_index(id, &member) else {
                        return self.err(format!("'{owner}' has no variant '{member}'"), span);
                    };
                    self.check_variant_pattern(
                        id,
                        vi,
                        Vec::new(),
                        &ty,
                        mode,
                        origin,
                        bindings,
                        span,
                    )?
                } else {
                    let mut e = Expr::new(ExprKind::Path(owner, member), span);
                    self.const_pattern(&mut e, &ty)?
                }
            }
            PatKind::Variant { owner, name, args } => {
                let id = match &owner {
                    Some(o) => match self.pattern_owner(o, &ty) {
                        Some(id) => id,
                        None if self.enum_ids.contains_key(o) => {
                            return self.err(
                                format!("'{o}::{name}' has no data: write '{o}::{name}'"),
                                span,
                            );
                        }
                        None => return self.err(format!("unknown enum '{o}'"), span),
                    },
                    None => match &ty {
                        Type::Sum(id) if self.variant_index(*id, &name).is_some() => *id,
                        _ => {
                            return self.err(
                                format!(
                                    "unknown variant '{name}' for a value of type {}",
                                    self.type_name(&ty)
                                ),
                                span,
                            );
                        }
                    },
                };
                let Some(vi) = self.variant_index(id, &name) else {
                    let o = self.sum_defs[id].name.clone();
                    return self.err(format!("'{o}' has no variant '{name}'"), span);
                };
                self.check_variant_pattern(id, vi, args, &ty, mode, origin, bindings, span)?
            }
            PatKind::Lit(mut e) => self.const_pattern(&mut e, &ty)?,
            other => other,
        };
        pat.r#type = Some(ty);
        Ok(())
    }

    fn pattern_owner(&self, o: &str, ty: &Type) -> Option<usize> {
        if o == "Self" {
            if let Some(Type::Sum(id)) = &self.ctx.self_type {
                return Some(*id);
            }
        }
        if let Some(&id) = self.sum_ids.get(o) {
            return Some(id);
        }
        if let Type::Sum(id) = ty {
            if self.instance_of.get(ty).is_some_and(|(n, _)| n == o) {
                return Some(*id);
            }
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn check_variant_pattern(
        &mut self,
        id: usize,
        vi: usize,
        mut args: Vec<Pattern>,
        ty: &Type,
        mode: Option<bool>,
        origin: u8,
        bindings: &mut Vec<PatBinding>,
        span: Span,
    ) -> TypeResult<PatKind> {
        self.ensure_sum(id)?;
        let full = format!(
            "{}::{}",
            self.sum_layout(id).name,
            self.sum_layout(id).variants[vi].name
        );
        if *ty != Type::Sum(id) {
            return self.err(
                format!(
                    "pattern '{full}' cannot match a value of type {}",
                    self.type_name(ty)
                ),
                span,
            );
        }
        let fields: Vec<Type> = self.sum_layout(id).variants[vi]
            .fields
            .iter()
            .map(|(t, _)| t.clone())
            .collect();
        if fields.len() != args.len() {
            let msg = if args.is_empty() {
                format!(
                    "'{full}' has data: write '{full}(..)' with {} pattern(s)",
                    fields.len()
                )
            } else {
                format!(
                    "'{full}' has {} value(s), the pattern gives {}",
                    fields.len(),
                    args.len()
                )
            };
            return self.err(msg, span);
        }
        for (a, t) in args.iter_mut().zip(&fields) {
            self.check_pattern(a, t, mode, origin, bindings)?;
        }
        Ok(PatKind::Ctor {
            sum: id,
            variant: vi,
            args,
        })
    }

    fn const_pattern(&mut self, e: &mut Expr, ty: &Type) -> TypeResult<PatKind> {
        if !matches!(ty, Type::Int(_) | Type::Bool | Type::Enum(_)) {
            return self.err(
                format!(
                    "expected a pattern for {} (a variant, '_' or a name)",
                    self.type_name(ty)
                ),
                e.span,
            );
        }
        self.coerce(e, ty)?;
        match self.const_eval(e)? {
            Some(v) => Ok(PatKind::Const(v)),
            None => self.err("match patterns must be constants", e.span),
        }
    }

    fn deconstruct(&self, p: &Pattern) -> DPat {
        match &p.kind {
            PatKind::Const(v) => DPat::Ctor(Ctor::Value(*v), Vec::new()),
            PatKind::Ctor { variant, args, .. } => DPat::Ctor(
                Ctor::Variant(*variant),
                args.iter().map(|a| self.deconstruct(a)).collect(),
            ),
            PatKind::Or(alts) => DPat::Or(alts.iter().map(|a| self.deconstruct(a)).collect()),
            _ => DPat::Wild,
        }
    }

    fn ctors(&self, t: &Type) -> Option<Vec<(Ctor, Vec<Type>)>> {
        let t = match t {
            Type::Ref(inner, _) => &**inner,
            t => t,
        };
        match t {
            Type::Bool => Some(vec![
                (Ctor::Value(0), Vec::new()),
                (Ctor::Value(1), Vec::new()),
            ]),
            Type::Enum(id) => Some(
                self.enum_info(*id)
                    .variants
                    .iter()
                    .map(|(_, v)| (Ctor::Value(*v), Vec::new()))
                    .collect(),
            ),
            Type::Sum(id) => Some(
                self.sum_layout(*id)
                    .variants
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        (
                            Ctor::Variant(i),
                            v.fields.iter().map(|(t, _)| t.clone()).collect(),
                        )
                    })
                    .collect(),
            ),
            _ => None,
        }
    }

    fn field_types(&self, t: &Type, c: &Ctor) -> Vec<Type> {
        match (c, t) {
            (Ctor::Variant(i), _) => {
                let t = match t {
                    Type::Ref(inner, _) => &**inner,
                    t => t,
                };
                let Type::Sum(id) = t else { unreachable!() };
                self.sum_layout(*id).variants[*i]
                    .fields
                    .iter()
                    .map(|(t, _)| t.clone())
                    .collect()
            }
            (Ctor::Value(_), _) => Vec::new(),
        }
    }

    fn expand(rows: &[Vec<DPat>]) -> Vec<Vec<DPat>> {
        let mut out = Vec::new();
        for r in rows {
            match &r[0] {
                DPat::Or(alts) => {
                    let sub: Vec<Vec<DPat>> = alts
                        .iter()
                        .map(|a| {
                            let mut row = vec![a.clone()];
                            row.extend_from_slice(&r[1..]);
                            row
                        })
                        .collect();
                    out.extend(Self::expand(&sub));
                }
                _ => out.push(r.clone()),
            }
        }
        out
    }

    fn specialize(row: &[DPat], c: &Ctor, arity: usize) -> Option<Vec<DPat>> {
        match &row[0] {
            DPat::Ctor(c2, args) if c2 == c => {
                let mut out = args.clone();
                out.extend_from_slice(&row[1..]);
                Some(out)
            }
            DPat::Ctor(..) => None,
            DPat::Or(_) => unreachable!("or-patterns are expanded first"),
            DPat::Wild => {
                let mut out = vec![DPat::Wild; arity];
                out.extend_from_slice(&row[1..]);
                Some(out)
            }
        }
    }

    fn heads(rows: &[Vec<DPat>]) -> Vec<Ctor> {
        let mut used = Vec::new();
        for r in rows {
            if let DPat::Ctor(c, _) = &r[0] {
                if !used.contains(c) {
                    used.push(c.clone());
                }
            }
        }
        used
    }

    fn useful(&self, rows: &[Vec<DPat>], q: &[DPat], tys: &[Type]) -> bool {
        if q.is_empty() {
            return rows.is_empty();
        }
        let rows = &Self::expand(rows);
        match &q[0] {
            DPat::Or(alts) => alts.iter().any(|a| {
                let mut q2 = vec![a.clone()];
                q2.extend_from_slice(&q[1..]);
                self.useful(rows, &q2, tys)
            }),
            DPat::Ctor(c, args) => {
                let mut ftys = self.field_types(&tys[0], c);
                ftys.extend_from_slice(&tys[1..]);
                let m: Vec<_> = rows
                    .iter()
                    .filter_map(|r| Self::specialize(r, c, args.len()))
                    .collect();
                let q2 = Self::specialize(q, c, args.len()).unwrap();
                self.useful(&m, &q2, &ftys)
            }
            DPat::Wild => {
                let used = Self::heads(rows);
                match self.ctors(&tys[0]) {
                    Some(all) if all.iter().all(|(c, _)| used.contains(c)) => {
                        all.iter().any(|(c, f)| {
                            let mut ftys = f.clone();
                            ftys.extend_from_slice(&tys[1..]);
                            let m: Vec<_> = rows
                                .iter()
                                .filter_map(|r| Self::specialize(r, c, f.len()))
                                .collect();
                            let q2 = Self::specialize(q, c, f.len()).unwrap();
                            self.useful(&m, &q2, &ftys)
                        })
                    }
                    _ => {
                        let m: Vec<_> = rows
                            .iter()
                            .filter(|r| matches!(r[0], DPat::Wild))
                            .map(|r| r[1..].to_vec())
                            .collect();
                        self.useful(&m, &q[1..], &tys[1..])
                    }
                }
            }
        }
    }

    fn missing(&self, rows: &[Vec<DPat>], tys: &[Type]) -> Option<Vec<DPat>> {
        if tys.is_empty() {
            return if rows.is_empty() {
                Some(Vec::new())
            } else {
                None
            };
        }
        let rows = &Self::expand(rows);
        let used = Self::heads(rows);
        match self.ctors(&tys[0]) {
            Some(all) if all.iter().all(|(c, _)| used.contains(c)) => {
                for (c, f) in &all {
                    let mut ftys = f.clone();
                    ftys.extend_from_slice(&tys[1..]);
                    let m: Vec<_> = rows
                        .iter()
                        .filter_map(|r| Self::specialize(r, c, f.len()))
                        .collect();
                    if let Some(w) = self.missing(&m, &ftys) {
                        let mut out = vec![DPat::Ctor(c.clone(), w[..f.len()].to_vec())];
                        out.extend_from_slice(&w[f.len()..]);
                        return Some(out);
                    }
                }
                None
            }
            other => {
                let m: Vec<_> = rows
                    .iter()
                    .filter(|r| matches!(r[0], DPat::Wild))
                    .map(|r| r[1..].to_vec())
                    .collect();
                let w = self.missing(&m, &tys[1..])?;
                let head = match other {
                    Some(all) if !used.is_empty() => {
                        let (c, f) = all.iter().find(|(c, _)| !used.contains(c)).unwrap();
                        DPat::Ctor(c.clone(), vec![DPat::Wild; f.len()])
                    }
                    _ => DPat::Wild,
                };
                let mut out = vec![head];
                out.extend(w);
                Some(out)
            }
        }
    }

    fn show_witness(&self, w: &DPat, t: &Type) -> String {
        let t = match t {
            Type::Ref(inner, _) => &**inner,
            t => t,
        };
        match (w, t) {
            (DPat::Wild, _) => "_".into(),
            (DPat::Ctor(Ctor::Value(v), _), Type::Bool) => (*v != 0).to_string(),
            (DPat::Ctor(Ctor::Value(v), _), Type::Enum(id)) => {
                let info = self.enum_info(*id);
                let name = info
                    .variants
                    .iter()
                    .find(|(_, x)| x == v)
                    .map_or(v.to_string(), |(n, _)| n.clone());
                format!("{}::{name}", info.name)
            }
            (DPat::Ctor(Ctor::Value(v), _), _) => v.to_string(),
            (DPat::Ctor(Ctor::Variant(i), args), Type::Sum(id)) => {
                let l = self.sum_layout(*id);
                let v = &l.variants[*i];
                let mut s = format!("{}::{}", l.name, v.name);
                if !args.is_empty() {
                    let parts: Vec<String> = args
                        .iter()
                        .zip(&v.fields)
                        .map(|(a, (ft, _))| self.show_witness(a, ft))
                        .collect();
                    s.push_str(&format!("({})", parts.join(", ")));
                }
                s
            }
            _ => "_".into(),
        }
    }
}
