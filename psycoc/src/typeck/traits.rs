use std::collections::HashMap;

use super::*;

impl TypeChecker {
    pub(super) fn implements(&mut self, t: &Type, tr: &str, span: Span) -> TypeResult<bool> {
        if tr == "Copy" && !self.traits.contains_key(tr) {
            self.size_of(t)?;
            return Ok(!self.is_move_type(t));
        }
        if !self.traits.contains_key(tr) {
            return self.err(format!("unknown trait '{tr}'"), span);
        }
        for i in 0..self.impl_tpls.len() {
            if self.impl_tpls[i].trait_name.as_deref() != Some(tr) {
                continue;
            }
            let generics = self.impl_tpls[i].generics.clone();
            let self_ty = self.impl_tpls[i].self_ty.clone();
            let params: Vec<String> = generics.iter().map(|g| g.name.clone()).collect();
            let mut subst = HashMap::new();
            self.unify(&self_ty, t, &params, &mut subst);
            if params.iter().any(|p| !subst.contains_key(p)) {
                continue;
            }
            if !matches!(self.try_resolve_pub(&self_ty, &params, &subst), Ok(Some(ref r)) if r == t)
            {
                continue;
            }
            let args: Vec<Type> = params.iter().map(|p| subst[p].clone()).collect();
            if self.check_bounds(&generics, &args, span).is_ok() {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl TypeChecker {
    pub(super) fn register_traits(&mut self, traits: Vec<TraitDef>) -> TypeResult<()> {
        for t in traits {
            if self.traits.contains_key(&t.name) || self.type_name_taken(&t.name) {
                return self.err(format!("'{}' is already defined", t.name), t.span);
            }
            let mut seen: Vec<&str> = Vec::new();
            for (m, _) in &t.methods {
                if seen.contains(&m.name.as_str()) {
                    return self.err(
                        format!("method '{}' declared twice in trait '{}'", m.name, t.name),
                        m.span,
                    );
                }
                seen.push(&m.name);
            }
            self.traits.insert(t.name.clone(), t);
        }
        Ok(())
    }

    pub(super) fn check_trait_impls(&mut self) -> TypeResult<()> {
        let mut concrete: Vec<(String, Type)> = Vec::new();
        for i in 0..self.impl_tpls.len() {
            let imp = self.impl_tpls[i].clone();
            let Some(tn) = &imp.trait_name else {
                continue;
            };
            let tr = self.traits[tn].clone();
            for m in &imp.methods {
                if !tr.methods.iter().any(|(t, _)| t.name == m.name) {
                    return self.err(
                        format!("'{}' is not a method of trait '{tn}'", m.name),
                        m.span,
                    );
                }
                if imp.methods.iter().filter(|x| x.name == m.name).count() > 1 {
                    return self.err(format!("method '{}' defined twice", m.name), m.span);
                }
            }
            for (tm, has_body) in &tr.methods {
                if !has_body && !imp.methods.iter().any(|m| m.name == tm.name) {
                    return self.err(
                        format!("missing method '{}' in impl {tn} for ..", tm.name),
                        imp.span,
                    );
                }
            }
            if imp.generics.is_empty() {
                let ty = self.resolve_type(&imp.self_ty)?;
                if concrete.iter().any(|(t, x)| t == tn && *x == ty) {
                    return self.err(
                        format!(
                            "trait '{tn}' is implemented twice for {}",
                            self.type_name(&ty)
                        ),
                        imp.span,
                    );
                }
                concrete.push((tn.clone(), ty));
            }
        }
        for (tn, ty) in concrete {
            let names: Vec<String> = self.traits[&tn]
                .methods
                .iter()
                .map(|(m, _)| m.name.clone())
                .collect();
            for n in names {
                self.instantiate_method(&ty, &n, self.traits[&tn].span)?;
            }
        }
        Ok(())
    }

    pub(super) fn check_trait_signature(
        &mut self,
        tn: &str,
        name: &str,
        ty: &Type,
        full: &str,
        span: Span,
    ) -> TypeResult<()> {
        let Some((tm, _)) = self.traits[tn]
            .methods
            .iter()
            .find(|(m, _)| m.name == name)
            .cloned()
        else {
            return Ok(());
        };
        if !tm.generics.is_empty() {
            return Ok(());
        }
        let saved = std::mem::replace(&mut self.ctx.self_type, Some(ty.clone()));
        let r = (|| -> TypeResult<FnSig> {
            let mut params = Vec::new();
            for p in &tm.params {
                params.push(self.resolve_type(&p.r#type)?);
            }
            let ret = match &tm.return_type {
                Some(t) => self.resolve_type(t)?,
                None => Type::Unit,
            };
            Ok(FnSig {
                params,
                ret,
                abi: Abi::Native,
            })
        })();
        self.ctx.self_type = saved;
        let want = r?;
        let have = self.functions[full].clone();
        if want.params != have.params || want.ret != have.ret {
            return self.err(
                format!(
                    "'{full}' does not match trait '{tn}': expected {}, found {}",
                    self.type_name(&want.fn_type()),
                    self.type_name(&have.fn_type())
                ),
                span,
            );
        }
        Ok(())
    }
}
