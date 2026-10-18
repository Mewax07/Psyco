use std::collections::HashMap;

use super::*;

pub(super) struct Subst<'a> {
    pub map: &'a HashMap<String, TypeExpr>,
}

impl Subst<'_> {
    pub fn function(&self, f: &mut Function) {
        for p in &mut f.params {
            self.ty(&mut p.r#type);
        }
        if let Some(t) = &mut f.return_type {
            self.ty(t);
        }
        self.block(&mut f.body);
    }

    pub fn ty(&self, t: &mut TypeExpr) {
        match &mut t.kind {
            TypeExprKind::Named(n) => {
                if let Some(r) = self.map.get(n) {
                    let span = t.span;
                    *t = r.clone();
                    t.span = span;
                }
            }
            TypeExprKind::Generic(_, args) => {
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

    fn block(&self, b: &mut Block) {
        for s in &mut b.stmts {
            self.stmt(s);
        }
    }

    fn stmt(&self, s: &mut Stmt) {
        match s {
            Stmt::Let {
                expr_type, value, ..
            } => {
                if let Some(t) = expr_type {
                    self.ty(t);
                }
                if let Some(v) = value {
                    self.expr(v);
                }
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
                start, end, body, ..
            } => {
                self.expr(start);
                self.expr(end);
                self.block(body);
            }
            Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::Match {
                scrutinee, arms, ..
            } => {
                self.expr(scrutinee);
                for arm in arms {
                    for p in &mut arm.pats {
                        self.pattern(p);
                    }
                    if let Some(g) = &mut arm.guard {
                        self.expr(g);
                    }
                    self.block(&mut arm.body);
                }
            }
            Stmt::IfLet {
                pat,
                value,
                then_block,
                else_block,
                ..
            } => {
                self.pattern(pat);
                self.expr(value);
                self.block(then_block);
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
                self.pattern(pat);
                self.expr(value);
                self.block(else_block);
            }
            Stmt::WhileLet {
                pat, value, body, ..
            } => {
                self.pattern(pat);
                self.expr(value);
                self.block(body);
            }
            Stmt::Block(b) => self.block(b),
        }
    }

    fn pattern(&self, p: &mut Pattern) {
        match &mut p.kind {
            PatKind::Variant { owner, args, .. } => {
                if let Some(o) = owner {
                    if let Some(TypeExpr {
                        kind: TypeExprKind::Named(n),
                        ..
                    }) = self.map.get(o.as_str())
                    {
                        *o = n.clone();
                    }
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

    fn expr(&self, e: &mut Expr) {
        match &mut e.kind {
            ExprKind::Path(owner, member) => {
                if let Some(t) = self.map.get(owner.as_str()) {
                    e.kind = ExprKind::TypePath {
                        ty: t.clone(),
                        member: member.clone(),
                        member_args: Vec::new(),
                    };
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
                if let (Some(t), Some(m)) = (self.map.get(owner.as_str()), member.clone()) {
                    if owner_args.is_empty() {
                        e.kind = ExprKind::TypePath {
                            ty: t.clone(),
                            member: m,
                            member_args: std::mem::take(member_args),
                        };
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
            ExprKind::StructLit { args, fields, .. } => {
                for a in args {
                    self.ty(a);
                }
                for (_, v, _) in fields {
                    self.expr(v);
                }
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
            ExprKind::Unsize(inner) => self.expr(inner),
            ExprKind::VariantLit { args, .. } => {
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::Try(inner) | ExprKind::TryOp { inner, .. } => self.expr(inner),
            ExprKind::Int(_)
            | ExprKind::TypedInt(..)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::CStr(_)
            | ExprKind::Bytes(_)
            | ExprKind::WStr(_)
            | ExprKind::Var(_) => {}
        }
    }
}

fn type_map(params: &[GenericParam], args: &[Type]) -> HashMap<String, TypeExpr> {
    params
        .iter()
        .zip(args)
        .map(|(p, t)| {
            (
                p.name.clone(),
                TypeExpr {
                    kind: TypeExprKind::Resolved(t.clone()),
                    span: p.span,
                },
            )
        })
        .collect()
}

impl TypeChecker {
    fn count_instance(&mut self, span: Span) -> TypeResult<()> {
        self.instances += 1;
        if self.instances > 5000 {
            return self.err(
                "too many generic instantiations (does a generic function call itself with a bigger type?)",
                span,
            );
        }
        Ok(())
    }

    fn inst_name(&self, name: &str, args: &[Type]) -> String {
        let parts: Vec<String> = args.iter().map(|t| self.type_name(t)).collect();
        format!("{name}<{}>", parts.join(", "))
    }

    fn check_arity(
        &self,
        what: &str,
        params: &[GenericParam],
        args: &[Type],
        span: Span,
    ) -> TypeResult<()> {
        if params.len() != args.len() {
            return self.err(
                format!(
                    "'{what}' takes {} type argument(s), got {}",
                    params.len(),
                    args.len()
                ),
                span,
            );
        }
        Ok(())
    }

    pub(super) fn instantiate_struct(
        &mut self,
        name: &str,
        args: Vec<Type>,
        span: Span,
    ) -> TypeResult<usize> {
        let key = (name.to_string(), args.clone());
        if let Some(&id) = self.struct_inst.get(&key) {
            return Ok(id);
        }
        let tpl = self.struct_tpls[name].clone();
        self.check_arity(name, &tpl.generics, &args, span)?;
        self.check_bounds(&tpl.generics, &args, span)?;
        self.count_instance(span)?;
        let map = type_map(&tpl.generics, &args);
        let mut def = tpl;
        def.name = self.inst_name(name, &args);
        def.generics.clear();
        let subst = Subst { map: &map };
        for f in &mut def.fields {
            subst.ty(&mut f.r#type);
        }
        let id = self.struct_defs.len();
        self.struct_ids.insert(def.name.clone(), id);
        self.struct_defs.push(def);
        self.layouts.push(State::Todo);
        self.struct_inst.insert(key, id);
        self.instance_of
            .insert(Type::Struct(id), (name.to_string(), args));
        self.ensure_layout(id)?;
        Ok(id)
    }

    pub(super) fn instantiate_sum(
        &mut self,
        name: &str,
        args: Vec<Type>,
        span: Span,
    ) -> TypeResult<usize> {
        let key = (name.to_string(), args.clone());
        if let Some(&id) = self.sum_inst.get(&key) {
            return Ok(id);
        }
        let tpl = self.sum_tpls[name].clone();
        self.check_arity(name, &tpl.generics, &args, span)?;
        self.check_bounds(&tpl.generics, &args, span)?;
        self.count_instance(span)?;
        let map = type_map(&tpl.generics, &args);
        let mut def = tpl;
        def.name = self.inst_name(name, &args);
        def.generics.clear();
        let subst = Subst { map: &map };
        for v in &mut def.variants {
            for t in v.fields.iter_mut().flatten() {
                subst.ty(t);
            }
        }
        let id = self.sum_defs.len();
        self.sum_ids.insert(def.name.clone(), id);
        self.sum_defs.push(def);
        self.sums.push(State::Todo);
        self.sum_inst.insert(key, id);
        self.instance_of
            .insert(Type::Sum(id), (name.to_string(), args));
        self.ensure_sum(id)?;
        Ok(id)
    }

    pub(super) fn instantiate_type(
        &mut self,
        name: &str,
        args: Vec<Type>,
        span: Span,
    ) -> TypeResult<Type> {
        if self.struct_tpls.contains_key(name) {
            Ok(Type::Struct(self.instantiate_struct(name, args, span)?))
        } else if self.sum_tpls.contains_key(name) {
            Ok(Type::Sum(self.instantiate_sum(name, args, span)?))
        } else if self.type_name_taken(name) {
            self.err(format!("'{name}' takes no type arguments"), span)
        } else {
            self.err(format!("unknown type '{name}'"), span)
        }
    }

    pub(super) fn is_template(&self, name: &str) -> bool {
        self.struct_tpls.contains_key(name) || self.sum_tpls.contains_key(name)
    }

    pub(super) fn instantiate_fn(
        &mut self,
        name: &str,
        args: Vec<Type>,
        span: Span,
    ) -> TypeResult<String> {
        let tpl = self.fn_tpls[name].clone();
        self.check_arity(name, &tpl.generics, &args, span)?;
        let mangled = self.inst_name(name, &args);
        if self.functions.contains_key(&mangled) {
            return Ok(mangled);
        }
        self.check_bounds(&tpl.generics, &args, span)?;
        self.count_instance(span)?;
        let map = type_map(&tpl.generics, &args);
        let mut f = tpl;
        Subst { map: &map }.function(&mut f);
        f.name = mangled.clone();
        f.generics.clear();
        if let Some(t) = self.fn_self.get(name).cloned() {
            self.fn_self.insert(mangled.clone(), t);
        }
        self.declare_function(&f)?;
        self.pending.push(f);
        Ok(mangled)
    }

    pub(super) fn unify(
        &self,
        te: &TypeExpr,
        ty: &Type,
        params: &[String],
        subst: &mut HashMap<String, Type>,
    ) {
        match (&te.kind, ty) {
            (TypeExprKind::Named(n), _) if params.contains(n) => {
                subst.entry(n.clone()).or_insert_with(|| ty.clone());
            }
            (TypeExprKind::Ref { inner, .. }, Type::Ref(t, _))
            | (TypeExprKind::Raw { inner, .. }, Type::Raw(t, _)) => {
                self.unify(inner, t, params, subst)
            }
            (TypeExprKind::Slice { elem, .. }, Type::Slice(t, _)) => {
                self.unify(elem, t, params, subst)
            }
            (TypeExprKind::Slice { elem, .. }, Type::Ref(a, _)) => {
                if let Type::Array(t, _) = &**a {
                    self.unify(elem, t, params, subst);
                }
            }
            (TypeExprKind::Array(elem, _), Type::Array(t, _)) => self.unify(elem, t, params, subst),
            (TypeExprKind::Generic(n, args), Type::Struct(_) | Type::Sum(_)) => {
                if let Some((tn, targs)) = self.instance_of.get(ty) {
                    if tn == n && targs.len() == args.len() {
                        for (a, t) in args.iter().zip(targs) {
                            self.unify(a, t, params, subst);
                        }
                    }
                }
            }
            (
                TypeExprKind::Fn {
                    params: ps, ret, ..
                },
                Type::Fn(ft),
            ) => {
                for (p, t) in ps.iter().zip(&ft.params) {
                    self.unify(p, t, params, subst);
                }
                if let Some(r) = ret {
                    self.unify(r, &ft.ret, params, subst);
                }
            }
            _ => {}
        }
    }

    fn mentions_unbound(
        &self,
        te: &TypeExpr,
        params: &[String],
        subst: &HashMap<String, Type>,
    ) -> bool {
        match &te.kind {
            TypeExprKind::Named(n) => params.contains(n) && !subst.contains_key(n),
            TypeExprKind::Generic(_, args) => {
                args.iter().any(|a| self.mentions_unbound(a, params, subst))
            }
            TypeExprKind::Ref { inner, .. } | TypeExprKind::Raw { inner, .. } => {
                self.mentions_unbound(inner, params, subst)
            }
            TypeExprKind::Slice { elem, .. } | TypeExprKind::Array(elem, _) => {
                self.mentions_unbound(elem, params, subst)
            }
            TypeExprKind::Fn {
                params: ps, ret, ..
            } => {
                ps.iter().any(|p| self.mentions_unbound(p, params, subst))
                    || ret
                        .as_ref()
                        .is_some_and(|r| self.mentions_unbound(r, params, subst))
            }
            TypeExprKind::Unit | TypeExprKind::Resolved(_) => false,
        }
    }

    fn try_resolve(
        &mut self,
        te: &TypeExpr,
        params: &[String],
        subst: &HashMap<String, Type>,
    ) -> TypeResult<Option<Type>> {
        if self.mentions_unbound(te, params, subst) {
            return Ok(None);
        }
        let map: HashMap<String, TypeExpr> = subst
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    TypeExpr {
                        kind: TypeExprKind::Resolved(v.clone()),
                        span: te.span,
                    },
                )
            })
            .collect();
        let mut te = te.clone();
        Subst { map: &map }.ty(&mut te);
        Ok(Some(self.resolve_type(&te)?))
    }

    pub(super) fn try_resolve_pub(
        &mut self,
        te: &TypeExpr,
        params: &[String],
        subst: &HashMap<String, Type>,
    ) -> TypeResult<Option<Type>> {
        self.try_resolve(te, params, subst)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn infer_call(
        &mut self,
        what: &str,
        params: &[String],
        param_tes: &[TypeExpr],
        ret_te: Option<&TypeExpr>,
        args: &mut [Expr],
        expected: Option<&Type>,
        subst: &mut HashMap<String, Type>,
        span: Span,
    ) -> TypeResult<()> {
        if param_tes.len() != args.len() {
            return self.err(
                format!(
                    "'{what}' expects {} argument(s), got {}",
                    param_tes.len(),
                    args.len()
                ),
                span,
            );
        }
        if let (Some(rte), Some(exp)) = (ret_te, expected) {
            self.unify(rte, exp, params, subst);
        }
        let mut done = vec![false; args.len()];
        for i in 0..args.len() {
            if let Some(pt) = self.try_resolve(&param_tes[i], params, subst)? {
                self.coerce(&mut args[i], &pt)?;
                done[i] = true;
            } else if !self.is_flexible(&args[i]) {
                let at = self.check_expr(&mut args[i], None)?;
                self.unify(&param_tes[i], &at, params, subst);
            }
        }
        for i in 0..args.len() {
            if done[i] {
                continue;
            }
            if args[i].r#type.is_none() {
                if let Some(pt) = self.try_resolve(&param_tes[i], params, subst)? {
                    self.coerce(&mut args[i], &pt)?;
                    done[i] = true;
                    continue;
                }
                let at = self.check_expr(&mut args[i], None)?;
                self.unify(&param_tes[i], &at, params, subst);
            }
        }
        if let Some(missing) = params.iter().find(|p| !subst.contains_key(*p)) {
            return self.err(
                format!(
                    "cannot infer the type parameter '{missing}' of '{what}' (give it explicitly: '{what}::<...>')"
                ),
                span,
            );
        }
        for i in 0..args.len() {
            if !done[i] {
                let pt = self.try_resolve(&param_tes[i], params, subst)?.unwrap();
                self.coerce(&mut args[i], &pt)?;
            }
        }
        Ok(())
    }

    pub(super) fn generic_call(
        &mut self,
        name: &str,
        explicit: Vec<Type>,
        mut args: Vec<Expr>,
        expected: Option<&Type>,
        span: Span,
    ) -> TypeResult<(ExprKind, Type)> {
        let tpl = self.fn_tpls[name].clone();
        let params: Vec<String> = tpl.generics.iter().map(|g| g.name.clone()).collect();
        let mut subst = HashMap::new();
        if !explicit.is_empty() {
            self.check_arity(name, &tpl.generics, &explicit, span)?;
            for (p, t) in params.iter().zip(explicit) {
                subst.insert(p.clone(), t);
            }
        }
        let param_tes: Vec<TypeExpr> = tpl.params.iter().map(|p| p.r#type.clone()).collect();
        let saved_self = self.ctx.self_type.clone();
        self.ctx.self_type = self.fn_self.get(name).cloned();
        let r = self.infer_call(
            name,
            &params,
            &param_tes,
            tpl.return_type.as_ref(),
            &mut args,
            expected,
            &mut subst,
            span,
        );
        self.ctx.self_type = saved_self;
        r?;
        let targs: Vec<Type> = params.iter().map(|p| subst[p].clone()).collect();
        let mangled = self.instantiate_fn(name, targs, span)?;
        let sig = self.functions[&mangled].clone();
        let callee = Expr {
            kind: ExprKind::Var(mangled.clone()),
            r#type: Some(sig.fn_type()),
            span,
        };
        Ok((
            ExprKind::Call {
                callee: Box::new(callee),
                args,
                target: CallTarget::Direct(mangled),
            },
            sig.ret,
        ))
    }

    pub(super) fn instantiate_method(
        &mut self,
        ty: &Type,
        method: &str,
        span: Span,
    ) -> TypeResult<Option<String>> {
        let full = format!("{}::{method}", self.type_name(ty));
        if self.functions.contains_key(&full) || self.fn_tpls.contains_key(&full) {
            return Ok(Some(full));
        }
        let mut found: Vec<(Function, Vec<GenericParam>, Vec<Type>, Option<String>)> = Vec::new();
        for i in 0..self.impl_tpls.len() {
            let imp = &self.impl_tpls[i];
            let m = match imp.methods.iter().find(|m| m.name == method) {
                Some(m) => m.clone(),
                None => match &imp.trait_name {
                    Some(tn) => match self.traits[tn]
                        .methods
                        .iter()
                        .find(|(m, d)| *d && m.name == method)
                    {
                        Some((m, _)) => m.clone(),
                        None => continue,
                    },
                    None => continue,
                },
            };
            let imp_generics = imp.generics.clone();
            let imp_self = imp.self_ty.clone();
            let trait_name = imp.trait_name.clone();
            let params: Vec<String> = imp_generics.iter().map(|g| g.name.clone()).collect();
            let mut subst = HashMap::new();
            self.unify(&imp_self, ty, &params, &mut subst);
            if params.iter().any(|p| !subst.contains_key(p)) {
                continue;
            }
            match self.try_resolve(&imp_self, &params, &subst) {
                Ok(Some(t)) if t == *ty => {}
                _ => continue,
            }
            let args: Vec<Type> = params.iter().map(|p| subst[p].clone()).collect();
            if let Err(e) = self.check_bounds(&imp_generics, &args, span) {
                self.unmet_bound = Some(e.message);
                continue;
            }
            found.push((m, imp_generics, args, trait_name));
        }
        let pick = match found.iter().position(|f| f.3.is_none()) {
            Some(i) => i,
            None if found.len() > 1 => {
                let names: Vec<String> = found.iter().filter_map(|f| f.3.clone()).collect();
                return self.err(
                    format!(
                        "method '{method}' of {} is ambiguous: it comes from traits {}",
                        self.type_name(ty),
                        names.join(" and ")
                    ),
                    span,
                );
            }
            None if found.is_empty() => return Ok(None),
            None => 0,
        };
        let (m, imp_generics, args, trait_name) = found.swap_remove(pick);
        self.count_instance(span)?;
        let map = type_map(&imp_generics, &args);
        let mut f = m;
        Subst { map: &map }.function(&mut f);
        f.name = full.clone();
        self.fn_self.insert(full.clone(), ty.clone());
        if f.generics.is_empty() {
            self.declare_function(&f)?;
            if let Some(tn) = &trait_name {
                self.check_trait_signature(tn, method, ty, &full, f.span)?;
            }
            self.pending.push(f);
        } else {
            self.fn_tpls.insert(full.clone(), f);
        }
        Ok(Some(full))
    }

    pub(super) fn infer_assoc_call(
        &mut self,
        owner: &str,
        member: &str,
        args: Vec<Expr>,
        expected: Option<&Type>,
        span: Span,
    ) -> TypeResult<(ExprKind, Type)> {
        let mut args = args;
        if let Some(tpl) = self.sum_tpls.get(owner).cloned() {
            if let Some(v) = tpl.variants.iter().find(|v| v.name == member) {
                let params: Vec<String> = tpl.generics.iter().map(|g| g.name.clone()).collect();
                let fields = v.fields.clone().unwrap_or_default();
                let ret = TypeExpr {
                    kind: TypeExprKind::Generic(
                        owner.to_string(),
                        params
                            .iter()
                            .map(|p| TypeExpr {
                                kind: TypeExprKind::Named(p.clone()),
                                span,
                            })
                            .collect(),
                    ),
                    span,
                };
                let mut subst = HashMap::new();
                let what = format!("{owner}::{member}");
                self.infer_call(
                    &what,
                    &params,
                    &fields,
                    Some(&ret),
                    &mut args,
                    expected,
                    &mut subst,
                    span,
                )?;
                let targs: Vec<Type> = params.iter().map(|p| subst[p].clone()).collect();
                let id = self.instantiate_sum(owner, targs, span)?;
                let vi = self.variant_index(id, member).unwrap();
                return self.build_variant(id, vi, args, span);
            }
        }
        for i in 0..self.impl_tpls.len() {
            let imp = &self.impl_tpls[i];
            let head_matches = match &imp.self_ty.kind {
                TypeExprKind::Generic(n, _) | TypeExprKind::Named(n) => n == owner,
                _ => false,
            };
            if !head_matches {
                continue;
            }
            let Some(m) = imp.methods.iter().find(|m| m.name == member) else {
                continue;
            };
            let mut m = m.clone();
            let mut params: Vec<String> = imp.generics.iter().map(|g| g.name.clone()).collect();
            let self_te = imp.self_ty.clone();
            let mut map = HashMap::new();
            map.insert("Self".to_string(), self_te.clone());
            Subst { map: &map }.function(&mut m);
            let impl_params = params.len();
            params.extend(m.generics.iter().map(|g| g.name.clone()));
            let param_tes: Vec<TypeExpr> = m.params.iter().map(|p| p.r#type.clone()).collect();
            let mut subst = HashMap::new();
            let what = format!("{owner}::{member}");
            self.infer_call(
                &what,
                &params,
                &param_tes,
                m.return_type.as_ref(),
                &mut args,
                expected,
                &mut subst,
                span,
            )?;
            let self_ty = self
                .try_resolve(&self_te, &params[..impl_params], &subst)?
                .unwrap();
            let full = match self.instantiate_method(&self_ty, member, span)? {
                Some(f) => f,
                None => {
                    return self.err(
                        format!("no function '{member}' for {}", self.type_name(&self_ty)),
                        span,
                    );
                }
            };
            let full = if self.fn_tpls.contains_key(&full) {
                let margs: Vec<Type> = params[impl_params..]
                    .iter()
                    .map(|p| subst[p].clone())
                    .collect();
                self.instantiate_fn(&full, margs, span)?
            } else {
                full
            };
            let sig = self.functions[&full].clone();
            let callee = Expr {
                kind: ExprKind::Var(full.clone()),
                r#type: Some(sig.fn_type()),
                span,
            };
            return Ok((
                ExprKind::Call {
                    callee: Box::new(callee),
                    args,
                    target: CallTarget::Direct(full),
                },
                sig.ret,
            ));
        }
        self.err(
            format!("'{owner}::{member}' is not a variant or a function"),
            span,
        )
    }

    pub(super) fn type_member(
        &mut self,
        ty: Type,
        member: &str,
        member_args: Vec<Type>,
        args: Option<Vec<Expr>>,
        expected: Option<&Type>,
        span: Span,
    ) -> TypeResult<(ExprKind, Type)> {
        if let Type::Sum(id) = ty {
            if let Some(vi) = self.variant_index(id, member) {
                return self.build_variant(id, vi, args.unwrap_or_default(), span);
            }
        }
        if let Type::Enum(eid) = ty {
            self.ensure_enum(eid)?;
            if let Some((_, v)) = self
                .enum_info(eid)
                .variants
                .iter()
                .find(|(n, _)| n == member)
            {
                if args.is_some() {
                    return self.err("a C-like enum value cannot be called", span);
                }
                return Ok((ExprKind::Int(*v), Type::Enum(eid)));
            }
        }
        let full = match self.instantiate_method(&ty, member, span)? {
            Some(f) => f,
            None => {
                return self.err(
                    format!("{} has no function '{member}'", self.type_name(&ty)),
                    span,
                );
            }
        };
        if self.fn_tpls.contains_key(&full) {
            return match args {
                Some(args) => self.generic_call(&full, member_args, args, expected, span),
                None => {
                    let f = self.instantiate_fn(&full, member_args, span)?;
                    let t = self.functions[&f].fn_type();
                    Ok((ExprKind::Var(f), t))
                }
            };
        }
        let sig = self.functions[&full].clone();
        match args {
            None => Ok((ExprKind::Var(full), sig.fn_type())),
            Some(mut args) => {
                self.check_args(&sig.params, &mut args, span)?;
                let callee = Expr {
                    kind: ExprKind::Var(full.clone()),
                    r#type: Some(sig.fn_type()),
                    span,
                };
                Ok((
                    ExprKind::Call {
                        callee: Box::new(callee),
                        args,
                        target: CallTarget::Direct(full),
                    },
                    sig.ret,
                ))
            }
        }
    }

    pub(super) fn check_bounds(
        &mut self,
        params: &[GenericParam],
        args: &[Type],
        span: Span,
    ) -> TypeResult<()> {
        for (p, t) in params.iter().zip(args) {
            for b in &p.bounds {
                if !self.implements(t, b, span)? {
                    return self.err(
                        format!(
                            "{} does not implement '{b}' (needed by '{}: {b}')",
                            self.type_name(t),
                            p.name
                        ),
                        span,
                    );
                }
            }
        }
        Ok(())
    }
}
