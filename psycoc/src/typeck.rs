use std::{
    collections::{HashMap, HashSet},
    mem::replace,
};

use crate::*;

#[derive(Debug)]
pub struct TypeError {
    pub message: String,
    pub span: Span,
}

type TypeResult<T> = Result<T, TypeError>;

#[derive(Debug, Clone)]
pub struct FieldLayout {
    pub name: String,
    pub r#type: Type,
    pub offset: u64,
}

#[derive(Debug, Clone)]
pub struct StructLayout {
    pub name: String,
    pub fields: Vec<FieldLayout>,
    pub size: u64,
    pub align: u64,
}

#[derive(Debug, Clone)]
pub struct EnumInfo {
    pub name: String,
    pub repr: IntType,
    pub variants: Vec<(String, i128)>,
}

#[derive(Debug, Clone)]
pub struct StaticInfo {
    pub name: String,
    pub r#type: Type,
    pub mutable: bool,
    pub init: Option<Vec<u8>>,
    pub align: u64,
}

#[derive(Debug, Clone)]
pub struct FnSig {
    pub params: Vec<Type>,
    pub ret: Type,
    pub abi: Abi,
}

impl FnSig {
    fn fn_type(&self) -> Type {
        Type::Fn(Box::new(FnType {
            params: self.params.clone(),
            ret: self.ret.clone(),
            abi: self.abi,
        }))
    }
}

#[derive(Debug, Default)]
pub struct ProgramInfo {
    pub structs: Vec<StructLayout>,
    pub enums: Vec<EnumInfo>,
    pub statics: Vec<StaticInfo>,
    pub functions: HashMap<String, FnSig>,
}

impl ProgramInfo {
    pub fn size_of(&self, t: &Type) -> u64 {
        match t {
            Type::Unit => 0,
            Type::Bool => 1,
            Type::Int(i) => i.size(),
            Type::Str | Type::Ref(..) | Type::Raw(..) | Type::Fn(_) => 8,
            Type::Slice(..) => 16,
            Type::Array(e, n) => self.size_of(e) * n,
            Type::Struct(id) => self.structs[*id].size,
            Type::Enum(id) => self.enums[*id].repr.size(),
        }
    }

    pub fn align_of(&self, t: &Type) -> u64 {
        match t {
            Type::Unit | Type::Bool => 1,
            Type::Int(i) => i.size(),
            Type::Array(e, _) => self.align_of(e),
            Type::Struct(id) => self.structs[*id].align,
            Type::Enum(id) => self.enums[*id].repr.size(),
            _ => 8,
        }
    }

    pub fn static_index(&self, name: &str) -> Option<usize> {
        self.statics.iter().position(|s| s.name == name)
    }

    pub fn field(&self, sid: usize, name: &str) -> &FieldLayout {
        self.structs[sid]
            .fields
            .iter()
            .find(|f| f.name == name)
            .expect("ICE: unknown field reached codegen")
    }

    pub fn int_repr(&self, t: &Type) -> Option<IntType> {
        match t {
            Type::Int(i) => Some(*i),
            Type::Enum(id) => Some(self.enums[*id].repr),
            Type::Bool => Some(IntType::U8),
            _ => None,
        }
    }
}

enum State<T> {
    Todo,
    Busy,
    Done(T),
}

#[derive(Clone)]
struct ConstVal {
    value: i128,
    r#type: Type,
    flexible: bool,
}

struct ConstSlot {
    def: ConstDef,
    state: State<ConstVal>,
}

#[derive(Clone)]
struct Local {
    r#type: Type,
    mutable: bool,
    origin: u8,
}

#[derive(Default)]
struct FnCtx {
    scopes: Vec<HashMap<String, Local>>,
    ret: Option<Type>,
    trusted: bool,
    loops: Vec<bool>,
}

pub struct TypeChecker {
    struct_ids: HashMap<String, usize>,
    struct_defs: Vec<StructDef>,
    layouts: Vec<State<StructLayout>>,
    enum_ids: HashMap<String, usize>,
    enum_defs: Vec<EnumDef>,
    enums: Vec<State<EnumInfo>>,
    consts: HashMap<String, ConstSlot>,
    statics: Vec<StaticInfo>,
    static_ids: HashMap<String, usize>,
    functions: HashMap<String, FnSig>,
    trusted_fns: HashSet<String>,
    ctx: FnCtx,
}

impl TypeChecker {
    pub fn new() -> Self {
        TypeChecker {
            struct_ids: HashMap::new(),
            struct_defs: Vec::new(),
            layouts: Vec::new(),
            enum_ids: HashMap::new(),
            enum_defs: Vec::new(),
            enums: Vec::new(),
            consts: HashMap::new(),
            statics: Vec::new(),
            static_ids: HashMap::new(),
            functions: HashMap::new(),
            trusted_fns: HashSet::new(),
            ctx: FnCtx::default(),
        }
    }

    pub fn check_program(&mut self, program: &mut Program) -> TypeResult<ProgramInfo> {
        for s in &program.structs {
            if self.type_name_taken(&s.name) {
                return self.err(format!("type '{}' is already defined", s.name), s.span);
            }
            self.struct_ids
                .insert(s.name.clone(), self.struct_defs.len());
            self.struct_defs.push(s.clone());
            self.layouts.push(State::Todo);
        }
        for e in &program.enums {
            if self.type_name_taken(&e.name) {
                return self.err(format!("type '{}' is already defined", e.name), e.span);
            }
            self.enum_ids.insert(e.name.clone(), self.enum_defs.len());
            self.enum_defs.push(e.clone());
            self.enums.push(State::Todo);
        }

        for c in &program.consts {
            if self.consts.contains_key(&c.name) {
                return self.err(format!("constant '{}' is already defined", c.name), c.span);
            }
            self.consts.insert(
                c.name.clone(),
                ConstSlot {
                    def: c.clone(),
                    state: State::Todo,
                },
            );
        }

        for sid in 0..self.struct_defs.len() {
            self.ensure_layout(sid)?;
        }
        for eid in 0..self.enum_defs.len() {
            self.ensure_enum(eid)?;
        }
        let names: Vec<String> = self.consts.keys().cloned().collect();
        for name in names {
            let span = self.consts[&name].def.span;
            self.ensure_const(&name, span)?;
        }

        for f in &program.functions {
            self.declare_function(f)?;
        }
        match program.functions.iter().find(|f| f.name == "main") {
            None => {
                return self.err(
                    "no 'main' function",
                    Span {
                        line: 1,
                        col: 1,
                        file: 0,
                    },
                );
            }
            Some(main) => {
                let sig = &self.functions["main"];
                if !sig.params.is_empty() || sig.ret != Type::Unit || sig.abi != Abi::Native {
                    return self.err(
                        "'main' must take no parameters and return nothing",
                        main.span,
                    );
                }
            }
        }

        for s in &mut program.statics {
            self.declare_static(s)?;
        }

        for f in &mut program.functions {
            self.check_function(f)?;
        }

        let structs = std::mem::take(&mut self.layouts)
            .into_iter()
            .map(|s| match s {
                State::Done(l) => l,
                _ => unreachable!(),
            })
            .collect();
        let enums = std::mem::take(&mut self.enums)
            .into_iter()
            .map(|s| match s {
                State::Done(e) => e,
                _ => unreachable!(),
            })
            .collect();
        Ok(ProgramInfo {
            structs,
            enums,
            statics: std::mem::take(&mut self.statics),
            functions: std::mem::take(&mut self.functions),
        })
    }

    // Utils

    fn err<T>(&self, message: impl Into<String>, span: Span) -> TypeResult<T> {
        Err(TypeError {
            message: message.into(),
            span,
        })
    }

    fn primitive_type(&self, name: &str) -> Option<Type> {
        use IntType::*;
        Some(match name {
            "Int" | "i64" => Type::Int(I64),
            "i8" => Type::Int(I8),
            "i16" => Type::Int(I16),
            "i32" => Type::Int(I32),
            "u8" => Type::Int(U8),
            "u16" => Type::Int(U16),
            "u32" => Type::Int(U32),
            "u64" => Type::Int(U64),
            "usize" => Type::Int(Usize),
            "isize" => Type::Int(Isize),
            "Bool" | "bool" => Type::Bool,
            "Str" => Type::Str,
            _ => return None,
        })
    }

    fn wrap_int(&self, v: i128, t: IntType) -> i128 {
        let bits = t.bits();
        let mask = (1i128 << bits) - 1;
        let m = v & mask;
        if t.signed() && m >= (1i128 << (bits - 1)) {
            m - (1i128 << bits)
        } else {
            m
        }
    }

    fn contains_ref(&self, t: &Type) -> bool {
        match t {
            Type::Ref(..) | Type::Slice(..) => true,
            Type::Raw(inner, _) | Type::Array(inner, _) => self.contains_ref(inner),
            _ => false,
        }
    }

    // Checker

    fn type_name_taken(&self, name: &str) -> bool {
        self.struct_ids.contains_key(name)
            || self.enum_ids.contains_key(name)
            || self.primitive_type(name).is_some()
    }

    fn value_name_taken(&self, name: &str) -> bool {
        self.functions.contains_key(name)
            || self.static_ids.contains_key(name)
            || self.consts.contains_key(name)
            || is_builtin(name)
    }

    fn declare_function(&mut self, f: &Function) -> TypeResult<()> {
        if self.functions.contains_key(&f.name) || self.static_ids.contains_key(&f.name) {
            return self.err(format!("'{}' is already defined", f.name), f.span);
        }
        if is_builtin(&f.name) {
            return self.err(format!("'{}' is a built-in function name", f.name), f.span);
        }
        if let Some((owner, _)) = f.name.split_once("::") {
            if !self.struct_ids.contains_key(owner) && !self.enum_ids.contains_key(owner) {
                return self.err(format!("impl for unknown type '{owner}'"), f.span);
            }
        }
        let mut params = Vec::new();
        for p in &f.params {
            let t = self.resolve_type(&p.r#type)?;
            self.check_storable(&t, true, p.span)?;
            params.push(t);
        }
        let ret = match &f.return_type {
            Some(t) => {
                let ret = self.resolve_type(t)?;
                if ret != Type::Unit {
                    self.check_storable(&ret, true, t.span)?;
                }
                ret
            }
            None => Type::Unit,
        };

        let interrupt = has_attr(&f.attrs, "interrupt");
        for a in &f.attrs {
            if a.name != "interrupt" {
                return self.err(format!("unknown function attribute '{}'", a.name), a.span);
            }
        }
        let abi = if interrupt {
            let ok_frame = matches!(params.first(), Some(Type::Ref(t, false)) | Some(Type::Raw(t, _)) if matches!(**t, Type::Struct(_)));
            let ok_code = params.len() == 1 || (params.len() == 2 && params[1] == Type::U64);
            if !ok_frame || !ok_code || ret != Type::Unit {
                return self.err(
                    "#[interrupt] functions take (frame: &Frame) or (frame: &Frame, code: u64) and return nothing",
                    f.span,
                );
            }
            Abi::Interrupt
        } else {
            Abi::Native
        };

        if f.trusted {
            self.trusted_fns.insert(f.name.clone());
        }
        self.functions
            .insert(f.name.clone(), FnSig { params, ret, abi });
        Ok(())
    }

    fn declare_static(&mut self, s: &mut StaticDef) -> TypeResult<()> {
        if self.value_name_taken(&s.name) {
            return self.err(format!("'{}' is already defined", s.name), s.span);
        }
        let r#type = self.resolve_type(&s.r#type)?;
        self.check_storable(&r#type, false, s.r#type.span)?;
        let mut align = self.align_of(&r#type)?;
        for a in &s.attrs {
            match (a.name.as_str(), a.arg) {
                ("align", Some(n)) if n.is_power_of_two() && n <= 4096 => align = align.max(n),
                ("align", _) => {
                    return self.err("#[align(N)] needs a power of two <= 4096", a.span);
                }
                _ => return self.err(format!("unknown static attribute '{}'", a.name), a.span),
            }
        }
        let size = self.size_of(&r#type)?;
        let init = match &mut s.init {
            None => None,
            Some(e) => {
                let saved = std::mem::take(&mut self.ctx);
                let r = self.coerce(e, &r#type);
                self.ctx = saved;
                r?;
                let mut bytes = vec![0u8; size as usize];
                self.const_bytes(e, &mut bytes, 0)?;
                if bytes.iter().all(|&b| b == 0) {
                    None
                } else {
                    Some(bytes)
                }
            }
        };
        self.static_ids.insert(s.name.clone(), self.statics.len());
        self.statics.push(StaticInfo {
            name: s.name.clone(),
            r#type,
            mutable: s.mutable,
            init,
            align,
        });
        Ok(())
    }

    fn type_name(&self, t: &Type) -> String {
        match t {
            Type::Unit => "()".into(),
            Type::Bool => "bool".into(),
            Type::Str => "Str".into(),
            Type::Int(i) => i.name().into(),
            Type::Ref(t, m) => format!("&{}{}", if *m { "mut " } else { "" }, self.type_name(t)),
            Type::Raw(t, m) => format!(
                "*{} {}",
                if *m { "mut" } else { "const" },
                self.type_name(t)
            ),
            Type::Slice(t, m) => {
                format!("&{}[{}]", if *m { "mut " } else { "" }, self.type_name(t))
            }
            Type::Array(t, n) => format!("[{}; {n}]", self.type_name(t)),
            Type::Struct(id) => self.struct_defs[*id].name.clone(),
            Type::Enum(id) => self.enum_defs[*id].name.clone(),
            Type::Fn(f) => {
                let params: Vec<_> = f.params.iter().map(|p| self.type_name(p)).collect();
                let prefix = match f.abi {
                    Abi::Efi => "extern fn",
                    Abi::Interrupt => "interrupt fn",
                    Abi::Native => "fn",
                };
                if f.ret == Type::Unit {
                    format!("{prefix}({})", params.join(", "))
                } else {
                    format!(
                        "{prefix}({}) -> {}",
                        params.join(", "),
                        self.type_name(&f.ret)
                    )
                }
            }
        }
    }

    fn resolve_type(&mut self, te: &TypeExpr) -> TypeResult<Type> {
        Ok(match &te.kind {
            TypeExprKind::Unit => Type::Unit,
            TypeExprKind::Named(name) => {
                if let Some(t) = self.primitive_type(name) {
                    t
                } else if let Some(&id) = self.struct_ids.get(name) {
                    Type::Struct(id)
                } else if let Some(&id) = self.enum_ids.get(name) {
                    Type::Enum(id)
                } else {
                    return self.err(format!("unknown type '{name}'"), te.span);
                }
            }
            TypeExprKind::Ref { inner, mutable } => {
                Type::Ref(Box::new(self.resolve_type(inner)?), *mutable)
            }
            TypeExprKind::Raw { inner, mutable } => {
                Type::Raw(Box::new(self.resolve_type(inner)?), *mutable)
            }
            TypeExprKind::Slice { elem, mutable } => {
                Type::Slice(Box::new(self.resolve_type(elem)?), *mutable)
            }
            TypeExprKind::Array(elem, count) => {
                let elem = self.resolve_type(elem)?;
                let mut count = (**count).clone();
                let n = self.const_usize(&mut count)?;
                Type::Array(Box::new(elem), n)
            }
            TypeExprKind::Fn { params, ret, efi } => {
                let mut ps = Vec::new();
                for p in params {
                    ps.push(self.resolve_type(p)?);
                }
                let ret = match ret {
                    Some(r) => self.resolve_type(r)?,
                    None => Type::Unit,
                };
                if *efi && (ps.iter().chain([&ret]).any(|t| t.is_aggregate())) {
                    return self.err(
                        "extern fn types only take and return scalars (use pointers)",
                        te.span,
                    );
                }
                Type::Fn(Box::new(FnType {
                    params: ps,
                    ret,
                    abi: if *efi { Abi::Efi } else { Abi::Native },
                }))
            }
        })
    }

    fn const_usize(&mut self, e: &mut Expr) -> TypeResult<u64> {
        let saved = std::mem::take(&mut self.ctx);
        let r = self.coerce(e, &Type::USIZE);
        self.ctx = saved;
        r?;
        match self.const_eval(e)? {
            Some(v) => Ok(v as u64),
            None => self.err("expected a constant expression", e.span),
        }
    }

    fn check_storable(&self, t: &Type, allow_top_ref: bool, span: Span) -> TypeResult<()> {
        if *t == Type::Unit {
            return self.err("a value of type () cannot be stored", span);
        }
        match t {
            Type::Ref(inner, _) | Type::Slice(inner, _) => {
                if !allow_top_ref {
                    return self.err(
                        "references cannot be stored in structs, arrays or statics (use a raw pointer)",
                        span,
                    );
                }
                if self.contains_ref(inner) {
                    return self.err("references to references are not supported", span);
                }
            }
            other => {
                if self.contains_ref(other) {
                    return self.err(
                        "references cannot be stored in structs, arrays, statics or behind pointers (use a raw pointer)",
                        span,
                    );
                }
            }
        }
        Ok(())
    }

    fn ensure_layout(&mut self, sid: usize) -> TypeResult<()> {
        match self.layouts[sid] {
            State::Done(_) => return Ok(()),
            State::Busy => {
                return self.err(
                    format!(
                        "struct '{}' contains itself (use a pointer)",
                        self.struct_defs[sid].name
                    ),
                    self.struct_defs[sid].span,
                );
            }
            State::Todo => {}
        }
        self.layouts[sid] = State::Busy;
        let def = self.struct_defs[sid].clone();
        let packed = has_attr(&def.attrs, "packed");
        for a in &def.attrs {
            if a.name != "packed" {
                return self.err(format!("unknown struct attribute '{}'", a.name), a.span);
            }
        }
        let mut fields: Vec<FieldLayout> = Vec::new();
        let mut offset = 0u64;
        let mut align = 1u64;
        for f in &def.fields {
            if fields.iter().any(|g| g.name == f.name) {
                return self.err(format!("field '{}' declared twice", f.name), f.span);
            }
            let r#type = self.resolve_type(&f.r#type)?;
            self.check_storable(&r#type, false, f.span)?;
            let size = self.size_of(&r#type)?;
            let falign = if packed { 1 } else { self.align_of(&r#type)? };
            offset = offset.next_multiple_of(falign);
            fields.push(FieldLayout {
                name: f.name.clone(),
                r#type,
                offset,
            });
            offset += size;
            align = align.max(falign);
        }
        let size = offset.next_multiple_of(align);
        self.layouts[sid] = State::Done(StructLayout {
            name: def.name.clone(),
            fields,
            size,
            align,
        });
        Ok(())
    }

    fn layout(&self, sid: usize) -> &StructLayout {
        match &self.layouts[sid] {
            State::Done(l) => l,
            _ => panic!("ICE: layout not computed"),
        }
    }

    fn ensure_enum(&mut self, eid: usize) -> TypeResult<()> {
        match self.enums[eid] {
            State::Done(_) => return Ok(()),
            State::Busy => {
                return self.err(
                    "enum discriminants depend on themselves",
                    self.enum_defs[eid].span,
                );
            }
            State::Todo => {}
        }
        self.enums[eid] = State::Busy;
        let def = self.enum_defs[eid].clone();
        let repr = match &def.repr {
            None => IntType::I64,
            Some(te) => match self.resolve_type(te)? {
                Type::Int(i) => i,
                _ => return self.err("enum representation must be an integer type", te.span),
            },
        };
        let mut variants: Vec<(String, i128)> = Vec::new();
        let mut next = 0i128;
        for v in &def.variants {
            if variants.iter().any(|(n, _)| *n == v.name) {
                return self.err(format!("variant '{}' declared twice", v.name), v.span);
            }
            let value = match &v.value {
                None => next,
                Some(e) => {
                    let mut e = e.clone();
                    let saved = std::mem::take(&mut self.ctx);
                    let r = self.coerce(&mut e, &Type::Int(repr));
                    self.ctx = saved;
                    r?;
                    match self.const_eval(&e)? {
                        Some(v) => v,
                        None => return self.err("discriminant must be a constant", e.span),
                    }
                }
            };
            if value < repr.min() || value > repr.max() {
                return self.err(
                    format!("discriminant {value} out of range for {}", repr.name()),
                    v.span,
                );
            }
            variants.push((v.name.clone(), value));
            next = value + 1;
        }
        self.enums[eid] = State::Done(EnumInfo {
            name: def.name,
            repr,
            variants,
        });
        Ok(())
    }

    fn enum_info(&self, eid: usize) -> &EnumInfo {
        match &self.enums[eid] {
            State::Done(e) => e,
            _ => panic!("ICE: enum not resolved"),
        }
    }

    fn ensure_const(&mut self, name: &str, use_span: Span) -> TypeResult<ConstVal> {
        let slot = self.consts.get_mut(name).unwrap();
        match &slot.state {
            State::Done(v) => return Ok(v.clone()),
            State::Busy => {
                return self.err(format!("constant '{name}' depends on itself"), use_span);
            }
            State::Todo => {}
        }
        slot.state = State::Busy;
        let def = slot.def.clone();
        let mut value = def.value.clone();
        let flexible = def.r#type.is_none() && matches!(value.kind, ExprKind::Int(_));
        let saved = std::mem::take(&mut self.ctx);
        let r = (|| -> TypeResult<Type> {
            match &def.r#type {
                Some(te) => {
                    let t = self.resolve_type(te)?;
                    self.coerce(&mut value, &t)?;
                    Ok(t)
                }
                None => self.check_expr(&mut value, None),
            }
        })();
        self.ctx = saved;
        let r#type = r?;
        if !matches!(r#type, Type::Int(_) | Type::Bool | Type::Enum(_)) {
            return self.err(
                "constants must have an integer, bool or enum type",
                def.span,
            );
        }
        let Some(v) = self.const_eval(&value)? else {
            return self.err("constant value must be a constant expression", value.span);
        };
        let val = ConstVal {
            value: v,
            r#type,
            flexible,
        };
        self.consts.get_mut(name).unwrap().state = State::Done(val.clone());
        Ok(val)
    }

    fn size_of(&mut self, t: &Type) -> TypeResult<u64> {
        Ok(match t {
            Type::Unit => 0,
            Type::Bool => 1,
            Type::Int(i) => i.size(),
            Type::Str | Type::Ref(..) | Type::Raw(..) | Type::Fn(_) => 8,
            Type::Slice(..) => 16,
            Type::Array(e, n) => self.size_of(e)? * n,
            Type::Struct(id) => {
                self.ensure_layout(*id)?;
                self.layout(*id).size
            }
            Type::Enum(id) => {
                self.ensure_enum(*id)?;
                self.enum_info(*id).repr.size()
            }
        })
    }

    fn align_of(&mut self, t: &Type) -> TypeResult<u64> {
        Ok(match t {
            Type::Unit | Type::Bool => 1,
            Type::Int(i) => i.size(),
            Type::Array(e, _) => self.align_of(e)?,
            Type::Struct(id) => {
                self.ensure_layout(*id)?;
                self.layout(*id).align
            }
            Type::Enum(id) => {
                self.ensure_enum(*id)?;
                self.enum_info(*id).repr.size()
            }
            _ => 8,
        })
    }

    fn fn_trusted(&self, name: &str) -> bool {
        self.trusted_fns.contains(name)
    }

    fn need_trusted(&self, what: &str, span: Span) -> TypeResult<()> {
        if self.ctx.trusted {
            Ok(())
        } else {
            self.err(
                format!("{what} is only allowed in a file that starts with #![trusted]"),
                span,
            )
        }
    }

    fn lookup_local(&self, name: &str) -> Option<&Local> {
        self.ctx.scopes.iter().rev().find_map(|s| s.get(name))
    }

    fn lookup_local_mut(&mut self, name: &str) -> Option<&mut Local> {
        self.ctx
            .scopes
            .iter_mut()
            .rev()
            .find_map(|s| s.get_mut(name))
    }

    fn declare_local(&mut self, name: &str, local: Local) {
        self.ctx
            .scopes
            .last_mut()
            .expect("ICE: no scope")
            .insert(name.to_string(), local);
    }

    fn check_function(&mut self, f: &mut Function) -> TypeResult<()> {
        let sig = self.functions[&f.name].clone();
        let mut frame = HashMap::new();
        for (param, r#type) in f.params.iter().zip(&sig.params) {
            let local = Local {
                r#type: r#type.clone(),
                mutable: param.mutable,
                origin: if r#type.is_ref_like() { 2u8 } else { 0 },
            };
            if frame.insert(param.name.clone(), local).is_some() {
                return self.err(
                    format!("parameter '{}' declared twice", param.name),
                    param.span,
                );
            }
        }
        self.ctx = FnCtx {
            scopes: vec![frame],
            ret: Some(sig.ret.clone()),
            trusted: self.fn_trusted(&f.name),
            loops: Vec::new(),
        };
        let diverges = self.check_block(&mut f.body)?;
        self.ctx = FnCtx::default();

        if sig.ret != Type::Unit && !diverges {
            return self.err(
                format!(
                    "function '{}' must return a {} on every path",
                    f.name,
                    self.type_name(&sig.ret)
                ),
                f.span,
            );
        }
        Ok(())
    }

    fn check_block(&mut self, block: &mut Block) -> TypeResult<bool> {
        self.ctx.scopes.push(HashMap::new());
        let r = self.check_stmts(&mut block.stmts, None);
        self.ctx.scopes.pop();
        r.map(|(d, _, _)| d)
    }

    fn check_stmts(
        &mut self,
        stmts: &mut [Stmt],
        last_hint: Option<&Type>,
    ) -> TypeResult<(bool, Type, u8)> {
        let mut diverges = false;
        let n = stmts.len();
        let mut last = (Type::Unit, 1u8);
        for (i, stmt) in stmts.iter_mut().enumerate() {
            if i + 1 == n {
                if let Stmt::Expr(e) = stmt {
                    let t = self.check_expr(e, last_hint)?;
                    let o = if t.is_ref_like() { self.origin(e) } else { 0 };
                    diverges |= self.expr_diverges(e);
                    last = (t, o);
                    continue;
                }
            }
            diverges |= self.check_stmt(stmt)?;
        }
        Ok((diverges, last.0, last.1))
    }

    fn expr_diverges(&self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Call {
                target: CallTarget::Builtin(name),
                ..
            } => diverging_builtin(name),
            _ => false,
        }
    }

    fn check_stmt(&mut self, stmt: &mut Stmt) -> TypeResult<bool> {
        match stmt {
            Stmt::Let {
                name,
                mutable,
                expr_type,
                value,
                span,
                r#type,
            } => {
                let t = match (expr_type, value.as_mut()) {
                    (Some(te), v) => {
                        let t = self.resolve_type(te)?;
                        self.check_storable(&t, true, te.span)?;
                        match v {
                            Some(v) => self.coerce(v, &t)?,
                            None if t.is_ref_like() => {
                                return self.err(
                                    "a reference must be initialized (references are never null)",
                                    *span,
                                );
                            }
                            None => {}
                        }
                        t
                    }
                    (None, Some(v)) => {
                        let t = self.check_expr(v, None)?;
                        if t == Type::Unit {
                            return self.err("cannot store a value of type ()", v.span);
                        }
                        self.check_storable(&t, true, v.span)?;
                        t
                    }
                    (None, None) => unreachable!("parser requires a type or a value"),
                };
                let origin = match value {
                    Some(v) if t.is_ref_like() => self.origin(v),
                    _ => 0,
                };
                *r#type = Some(t.clone());
                self.declare_local(
                    name,
                    Local {
                        r#type: t,
                        mutable: *mutable,
                        origin,
                    },
                );
                Ok(false)
            }
            Stmt::Assign {
                target,
                op,
                value,
                span,
            } => {
                let tt = self.check_expr(target, None)?;
                match self.place_mut(target) {
                    Some(true) => {}
                    Some(false) => {
                        let msg = match &target.kind {
                                ExprKind::Var(n) => format!(
                                    "cannot assign to immutable variable '{n}' (declare it with 'let mut')"
                                ),
                                _ => "cannot assign through a '&' reference or '*const' pointer, or to a field of an immutable variable".into(),
                            };
                        return self.err(msg, *span);
                    }
                    None => return self.err("this expression cannot be assigned to", target.span),
                }
                match op {
                    None => self.coerce(value, &tt)?,
                    Some(op) => {
                        let vt = self.check_expr(value, Some(&tt))?;
                        self.binary_result(*op, &tt, &vt, value, *span)?;
                    }
                }
                if tt.is_ref_like() {
                    let o = self.origin(value);
                    if let ExprKind::Var(n) = &target.kind {
                        let n = n.clone();
                        if let Some(l) = self.lookup_local_mut(&n) {
                            l.origin |= o;
                        }
                    }
                }
                Ok(false)
            }
            Stmt::Expr(e) => {
                self.check_expr(e, None)?;
                Ok(self.expr_diverges(e))
            }
            Stmt::Return { value, span } => {
                let ret = self.ctx.ret.clone().expect("ICE: return outside function");
                match value {
                    Some(e) => {
                        self.coerce(e, &ret)?;
                        if ret.is_ref_like() && self.origin(e) & 1u8 != 0 {
                            return self.err(
                                "cannot return a reference to a local variable (it is freed when the function returns)",
                                e.span,
                            );
                        }
                    }
                    None if ret != Type::Unit => {
                        return self.err(
                            format!("expected a {} return value", self.type_name(&ret)),
                            *span,
                        );
                    }
                    None => {}
                }
                Ok(true)
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                self.coerce(cond, &Type::Bool)?;
                let t = self.check_block(then_block)?;
                let e = match else_block {
                    Some(b) => self.check_block(b)?,
                    None => false,
                };
                Ok(t && e)
            }
            Stmt::While { cond, body, .. } => {
                self.coerce(cond, &Type::Bool)?;
                self.ctx.loops.push(false);
                let r = self.check_block(body);
                self.ctx.loops.pop();
                r?;
                Ok(false)
            }
            Stmt::Loop { body, .. } => {
                self.ctx.loops.push(false);
                let r = self.check_block(body);
                let broke = self.ctx.loops.pop().unwrap();
                r?;
                Ok(!broke)
            }
            Stmt::For {
                var,
                start,
                end,
                body,
                span,
            } => {
                let (st, et) = self.check_operands(start, end, None)?;
                if st != et || !st.is_int() {
                    return self.err(
                        format!(
                            "for range bounds must have the same integer type, found {} and {}",
                            self.type_name(&st),
                            self.type_name(&et)
                        ),
                        *span,
                    );
                }
                self.ctx.scopes.push(HashMap::new());
                self.declare_local(
                    var,
                    Local {
                        r#type: st,
                        mutable: false,
                        origin: 0,
                    },
                );
                self.ctx.loops.push(false);
                let r = self.check_block(body);
                self.ctx.loops.pop();
                self.ctx.scopes.pop();
                r?;
                Ok(false)
            }
            Stmt::Break(span) => match self.ctx.loops.last_mut() {
                Some(b) => {
                    *b = true;
                    Ok(true)
                }
                None => self.err("'break' outside of a loop", *span),
            },
            Stmt::Continue(span) => {
                if self.ctx.loops.is_empty() {
                    return self.err("'continue' outside of a loop", *span);
                }
                Ok(true)
            }
            Stmt::Match {
                scrutinee,
                arms,
                span,
            } => {
                let st = self.check_expr(scrutinee, None)?;
                if !matches!(st, Type::Int(_) | Type::Enum(_) | Type::Bool) {
                    return self.err(
                        format!("cannot match on a value of type {}", self.type_name(&st)),
                        scrutinee.span,
                    );
                }
                let mut seen: Vec<i128> = Vec::new();
                let mut has_wildcard = false;
                let mut all_diverge = true;
                for arm in arms.iter_mut() {
                    match &mut arm.patterns {
                        None => {
                            if has_wildcard {
                                return self.err("'_' appears twice", arm.span);
                            }
                            has_wildcard = true;
                        }
                        Some(pats) => {
                            for p in pats.iter_mut() {
                                self.coerce(p, &st)?;
                                let Some(v) = self.const_eval(p)? else {
                                    return self.err("match patterns must be constants", p.span);
                                };
                                if seen.contains(&v) {
                                    return self.err("this pattern is already covered", p.span);
                                }
                                seen.push(v);
                                p.kind = ExprKind::Int(v);
                            }
                        }
                    }
                    all_diverge &= self.check_block(&mut arm.body)?;
                }
                let exhaustive = has_wildcard
                    || (st == Type::Bool && seen.len() == 2)
                    || matches!(st, Type::Enum(id) if self.enum_info(id).variants.iter().all(|(_, v)| seen.contains(v)));
                let _ = span;
                Ok(exhaustive && all_diverge && !arms.is_empty())
            }
            Stmt::Block(b) => self.check_block(b),
        }
    }

    fn place_mut(&self, e: &Expr) -> Option<bool> {
        match &e.kind {
            ExprKind::Var(n) => {
                if let Some(l) = self.lookup_local(n) {
                    Some(l.mutable)
                } else {
                    self.static_ids.get(n).map(|&i| self.statics[i].mutable)
                }
            }
            ExprKind::Field { base, .. } => match base.r#type() {
                Type::Struct(_) => self.place_mut(base),
                _ => None,
            },
            ExprKind::Index { base, .. } => match base.r#type() {
                Type::Array(..) => self.place_mut(base),
                Type::Slice(_, m) => Some(*m),
                _ => None,
            },
            ExprKind::Unary {
                op: UnaryOp::Deref,
                operand,
            } => match operand.r#type() {
                Type::Ref(_, m) | Type::Raw(_, m) => Some(*m),
                _ => None,
            },
            _ => None,
        }
    }

    fn origin(&self, e: &Expr) -> u8 {
        match &e.kind {
            ExprKind::Var(n) => self.lookup_local(n).map_or(1u8, |l| l.origin),
            ExprKind::Unary {
                op: UnaryOp::Ref(_),
                operand,
            } => self.place_origin(operand),
            ExprKind::Range { base, .. } => match base.r#type() {
                Type::Slice(..) => self.origin(base),
                _ => self.place_origin(base),
            },
            ExprKind::Unsize(inner) => self.origin(inner),
            ExprKind::Call { args, .. } => {
                let o = args
                    .iter()
                    .filter(|a| a.r#type().is_ref_like())
                    .fold(0, |acc, a| acc | self.origin(a));
                if o == 0 { 4u8 } else { o }
            }
            _ => 1u8,
        }
    }

    fn place_origin(&self, p: &Expr) -> u8 {
        match &p.kind {
            ExprKind::Var(n) => {
                if self.lookup_local(n).is_some() {
                    1u8
                } else {
                    4u8
                }
            }
            ExprKind::Field { base, .. } => self.place_origin(base),
            ExprKind::Index { base, .. } => match base.r#type() {
                Type::Slice(..) => self.origin(base),
                _ => self.place_origin(base),
            },
            ExprKind::Unary {
                op: UnaryOp::Deref,
                operand,
            } => match operand.r#type() {
                Type::Ref(..) => self.origin(operand),
                _ => 4u8,
            },
            _ => 1u8,
        }
    }

    fn coerce(&mut self, e: &mut Expr, expected: &Type) -> TypeResult<()> {
        let t = self.check_expr(e, Some(expected))?;
        if t == *expected {
            return Ok(());
        }
        match (expected, &t) {
            (Type::Slice(et, m), Type::Ref(inner, m2))
                if matches!(&**inner, Type::Array(at, _) if at == et) && (*m2 || !*m) =>
            {
                let inner_expr = take(e);
                let span = inner_expr.span;
                *e = Expr {
                    kind: ExprKind::Unsize(Box::new(inner_expr)),
                    r#type: Some(expected.clone()),
                    span,
                };
                Ok(())
            }
            (Type::Ref(a, false), Type::Ref(b, true))
            | (Type::Slice(a, false), Type::Slice(b, true))
            | (Type::Raw(a, false), Type::Raw(b, true))
                if a == b =>
            {
                e.r#type = Some(expected.clone());
                Ok(())
            }
            _ => {
                let hint = match (expected, &t) {
                    (Type::Int(_), Type::Int(_)) => " (use 'as' to convert)",
                    _ => "",
                };
                self.err(
                    format!(
                        "expected {}, found {}{hint}",
                        self.type_name(expected),
                        self.type_name(&t)
                    ),
                    e.span,
                )
            }
        }
    }

    fn is_flexible(&self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Int(_) => e.r#type.is_none(),
            ExprKind::Var(n) => {
                self.lookup_local(n).is_none()
                    && matches!(self.consts.get(n), Some(ConstSlot { def, .. }) if def.r#type.is_none() && matches!(def.value.kind, ExprKind::Int(_)))
            }
            ExprKind::Unary {
                op: UnaryOp::Neg | UnaryOp::BitNot,
                operand,
            } => self.is_flexible(operand),
            _ => false,
        }
    }

    fn check_operands(
        &mut self,
        lhs: &mut Expr,
        rhs: &mut Expr,
        hint: Option<&Type>,
    ) -> TypeResult<(Type, Type)> {
        if self.is_flexible(lhs) && !self.is_flexible(rhs) {
            let rt = self.check_expr(rhs, hint)?;
            let lt = self.check_expr(lhs, Some(&rt))?;
            Ok((lt, rt))
        } else {
            let lt = self.check_expr(lhs, hint)?;
            let rt = self.check_expr(rhs, Some(&lt))?;
            Ok((lt, rt))
        }
    }

    pub fn check_expr(&mut self, e: &mut Expr, expected: Option<&Type>) -> TypeResult<Type> {
        if let Some(t) = &e.r#type {
            if matches!(e.kind, ExprKind::Unsize(_)) {
                return Ok(t.clone());
            }
        }
        let span = e.span;
        let kind = std::mem::replace(&mut e.kind, ExprKind::Bool(false));
        let (kind, r#type) = self.check_kind(kind, span, expected)?;
        e.kind = kind;
        e.r#type = Some(r#type.clone());
        Ok(r#type)
    }

    fn check_kind(
        &mut self,
        kind: ExprKind,
        span: Span,
        expected: Option<&Type>,
    ) -> TypeResult<(ExprKind, Type)> {
        match kind {
            ExprKind::Int(v) => {
                let it = match expected {
                    Some(Type::Int(i)) => *i,
                    _ if v > i64::MAX as i128 => IntType::U64,
                    _ => IntType::I64,
                };
                if v < it.min() || v > it.max() {
                    return self.err(format!("literal {v} out of range for {}", it.name()), span);
                }
                Ok((ExprKind::Int(v), Type::Int(it)))
            }
            ExprKind::TypedInt(v, it) => {
                if v < it.min() || v > it.max() {
                    return self.err(format!("literal {v} out of range for {}", it.name()), span);
                }
                Ok((ExprKind::Int(v), Type::Int(it)))
            }
            ExprKind::Bool(b) => Ok((ExprKind::Bool(b), Type::Bool)),
            ExprKind::Str(s) => Ok((ExprKind::Str(s), Type::Str)),
            ExprKind::CStr(s) => Ok((ExprKind::CStr(s), Type::raw(Type::U8, false))),
            ExprKind::WStr(s) => Ok((ExprKind::WStr(s), Type::raw(Type::Int(IntType::U16), false))),

            ExprKind::Var(name) => {
                if let Some(l) = self.lookup_local(&name) {
                    let t = l.r#type.clone();
                    return Ok((ExprKind::Var(name), t));
                }
                if let Some(&i) = self.static_ids.get(&name) {
                    let t = self.statics[i].r#type.clone();
                    return Ok((ExprKind::Var(name), t));
                }
                if self.consts.contains_key(&name) {
                    let c = self.ensure_const(&name, span)?;
                    if c.r#type == Type::Bool {
                        return Ok((ExprKind::Bool(c.value != 0), Type::Bool));
                    }
                    if c.flexible {
                        return self.check_kind(ExprKind::Int(c.value), span, expected);
                    }
                    return Ok((ExprKind::Int(c.value), c.r#type));
                }
                if let Some(sig) = self.functions.get(&name) {
                    let t = sig.fn_type();
                    return Ok((ExprKind::Var(name), t));
                }
                if is_builtin(&name) {
                    return self.err(format!("built-in '{name}' can only be called"), span);
                }
                self.err(format!("unknown variable '{name}'"), span)
            }

            ExprKind::Path(owner, member) => {
                if let Some(&eid) = self.enum_ids.get(&owner) {
                    self.ensure_enum(eid)?;
                    if let Some((_, v)) = self
                        .enum_info(eid)
                        .variants
                        .iter()
                        .find(|(n, _)| *n == member)
                    {
                        return Ok((ExprKind::Int(*v), Type::Enum(eid)));
                    }
                }
                let full = format!("{owner}::{member}");
                if let Some(sig) = self.functions.get(&full) {
                    let t = sig.fn_type();
                    return Ok((ExprKind::Var(full), t));
                }
                self.err(format!("'{full}' is not a variant or a function"), span)
            }

            ExprKind::Unary { op, mut operand } => {
                self.check_unary(op, &mut operand, span, expected)
            }

            ExprKind::Binary {
                op,
                mut lhs,
                mut rhs,
            } => {
                let r#type = match op {
                    BinOp::And | BinOp::Or => {
                        self.coerce(&mut lhs, &Type::Bool)?;
                        self.coerce(&mut rhs, &Type::Bool)?;
                        Type::Bool
                    }
                    _ if op.is_comparison() => {
                        let (lt, rt) = self.check_operands(&mut lhs, &mut rhs, None)?;
                        self.binary_result(op, &lt, &rt, &rhs, span)?
                    }
                    BinOp::Shl | BinOp::Shr => {
                        let lt = self.check_expr(&mut lhs, expected)?;
                        let rt = self.check_expr(&mut rhs, Some(&Type::Int(IntType::U32)))?;
                        self.binary_result(op, &lt, &rt, &rhs, span)?
                    }
                    _ => {
                        let (lt, rt) = self.check_operands(&mut lhs, &mut rhs, expected)?;
                        self.binary_result(op, &lt, &rt, &rhs, span)?
                    }
                };
                Ok((ExprKind::Binary { op, lhs, rhs }, r#type))
            }

            ExprKind::Cast { mut expr, r#type } => {
                let to = self.resolve_type(&r#type)?;
                let from = self.check_expr(&mut expr, None)?;
                if !self.cast_ok(&from, &to) {
                    return self.err(
                        format!(
                            "cannot cast {} as {}",
                            self.type_name(&from),
                            self.type_name(&to)
                        ),
                        span,
                    );
                }
                Ok((ExprKind::Cast { expr, r#type }, to))
            }

            ExprKind::Call {
                callee,
                args,
                target: _,
            } => self.check_call(*callee, args, span),

            ExprKind::Field { base, name } => {
                let mut base = *base;
                self.check_expr(&mut base, None)?;
                let (base, r#type) = self.make_field(base, &name, span)?;
                Ok((
                    ExprKind::Field {
                        base: Box::new(base),
                        name,
                    },
                    r#type,
                ))
            }

            ExprKind::Index { base, mut index } => {
                let mut base = *base;
                self.check_expr(&mut base, None)?;
                let base = self.auto_deref(base);
                let elem = match base.r#type() {
                    Type::Array(t, _) | Type::Slice(t, _) => (**t).clone(),
                    Type::Raw(..) => {
                        return self.err(
                            "raw pointers cannot be indexed; use 'p.add(i).read()' in a #![trusted] file",
                            span,
                        );
                    }
                    other => {
                        return self.err(
                            format!("cannot index a value of type {}", self.type_name(other)),
                            span,
                        );
                    }
                };
                self.coerce(&mut index, &Type::USIZE)?;
                if let (Type::Array(_, n), Some(i)) = (base.r#type(), self.const_eval(&index)?) {
                    if i as u64 >= *n {
                        return self.err(
                            format!("index {i} out of bounds for an array of length {n}"),
                            index.span,
                        );
                    }
                }
                Ok((
                    ExprKind::Index {
                        base: Box::new(base),
                        index,
                    },
                    elem,
                ))
            }

            ExprKind::Range { .. } => self.err(
                "slicing needs a reference: write '&a[i..j]' or '&mut a[i..j]'",
                span,
            ),

            ExprKind::StructLit { name, mut fields } => {
                let Some(&sid) = self.struct_ids.get(&name) else {
                    return self.err(format!("unknown struct '{name}'"), span);
                };
                let layout = self.layout(sid).clone();
                let mut seen: Vec<String> = Vec::new();
                for (fname, value, fspan) in fields.iter_mut() {
                    let Some(fl) = layout.fields.iter().find(|f| f.name == *fname) else {
                        return self.err(format!("struct '{name}' has no field '{fname}'"), *fspan);
                    };
                    if seen.contains(fname) {
                        return self.err(format!("field '{fname}' given twice"), *fspan);
                    }
                    seen.push(fname.clone());
                    self.coerce(value, &fl.r#type)?;
                }
                let missing: Vec<_> = layout
                    .fields
                    .iter()
                    .filter(|f| !seen.contains(&f.name))
                    .map(|f| f.name.as_str())
                    .collect();
                if !missing.is_empty() {
                    return self.err(
                        format!("missing field(s) in '{name}': {}", missing.join(", ")),
                        span,
                    );
                }
                Ok((ExprKind::StructLit { name, fields }, Type::Struct(sid)))
            }

            ExprKind::ArrayLit(mut items) => {
                let elem_hint = match expected {
                    Some(Type::Array(t, _)) => Some((**t).clone()),
                    _ => None,
                };
                let elem = match items.first_mut() {
                    None => match elem_hint {
                        Some(t) => t,
                        None => return self.err("cannot infer the type of an empty array", span),
                    },
                    Some(first) => match &elem_hint {
                        Some(t) => {
                            self.coerce(first, t)?;
                            t.clone()
                        }
                        None => self.check_expr(first, None)?,
                    },
                };
                self.check_storable(&elem, false, span)?;
                for item in items.iter_mut().skip(1) {
                    self.coerce(item, &elem)?;
                }
                let n = items.len() as u64;
                Ok((ExprKind::ArrayLit(items), Type::Array(Box::new(elem), n)))
            }

            ExprKind::ArrayRepeat {
                mut value,
                mut count,
            } => {
                let n = self.const_usize(&mut count)?;
                let elem = match expected {
                    Some(Type::Array(t, _)) => {
                        let t = (**t).clone();
                        self.coerce(&mut value, &t)?;
                        t
                    }
                    _ => self.check_expr(&mut value, None)?,
                };
                self.check_storable(&elem, false, span)?;
                Ok((
                    ExprKind::ArrayRepeat { value, count },
                    Type::Array(Box::new(elem), n),
                ))
            }

            ExprKind::Sizeof(te) => {
                let t = self.resolve_type(&te)?;
                let size = self.size_of(&t)?;
                Ok((ExprKind::Int(size as i128), Type::USIZE))
            }

            ExprKind::Unsize(_) => unreachable!("handled in check_expr"),
        }
    }

    fn auto_deref(&self, base: Expr) -> Expr {
        match base.r#type().clone() {
            Type::Ref(inner, _) => {
                let span = base.span;
                Expr {
                    kind: ExprKind::Unary {
                        op: UnaryOp::Deref,
                        operand: Box::new(base),
                    },
                    r#type: Some(*inner),
                    span,
                }
            }
            _ => base,
        }
    }

    fn make_field(&mut self, base: Expr, name: &str, span: Span) -> TypeResult<(Expr, Type)> {
        let base = self.auto_deref(base);
        match base.r#type().clone() {
            Type::Struct(sid) => {
                let layout = self.layout(sid);
                match layout.fields.iter().find(|f| f.name == name) {
                    Some(f) => {
                        let t = f.r#type.clone();
                        Ok((base, t))
                    }
                    None => self.err(
                        format!("struct '{}' has no field '{name}'", layout.name),
                        span,
                    ),
                }
            }
            Type::Raw(..) => self.err(
                format!(
                    "cannot access a field through a raw pointer; write '(*p).{name}' in a #![trusted] file"
                ),
                span,
            ),
            Type::Array(..) | Type::Slice(..) | Type::Str if name == "len" => {
                self.err("use the method '.len()'", span)
            }
            other => self.err(
                format!("type {} has no field '{name}'", self.type_name(&other)),
                span,
            ),
        }
    }

    fn check_unary(
        &mut self,
        op: UnaryOp,
        operand: &mut Box<Expr>,
        span: Span,
        expected: Option<&Type>,
    ) -> TypeResult<(ExprKind, Type)> {
        let rebuild = |operand: &mut Box<Expr>| ExprKind::Unary {
            op,
            operand: std::mem::replace(operand, Box::new(Expr::new(ExprKind::Bool(false), span))),
        };
        match op {
            UnaryOp::Neg => {
                let t = self.check_expr(operand, expected)?;
                match t {
                    Type::Int(i) if i.signed() => Ok((rebuild(operand), t)),
                    _ => self.err(
                        format!("cannot negate a value of type {}", self.type_name(&t)),
                        span,
                    ),
                }
            }
            UnaryOp::Not => {
                let t = self.check_expr(operand, expected)?;
                match t {
                    Type::Bool => Ok((rebuild(operand), t)),
                    Type::Int(_) => Ok((
                        ExprKind::Unary {
                            op: UnaryOp::BitNot,
                            operand: std::mem::replace(
                                operand,
                                Box::new(Expr::new(ExprKind::Bool(false), span)),
                            ),
                        },
                        t,
                    )),
                    _ => self.err(format!("cannot apply '!' to {}", self.type_name(&t)), span),
                }
            }
            UnaryOp::BitNot => {
                let t = self.check_expr(operand, expected)?;
                match t {
                    Type::Int(_) => Ok((rebuild(operand), t)),
                    _ => self.err(format!("cannot apply '~' to {}", self.type_name(&t)), span),
                }
            }
            UnaryOp::Deref => {
                let t = self.check_expr(operand, None)?;
                match t {
                    Type::Ref(inner, _) => Ok((rebuild(operand), *inner)),
                    Type::Raw(inner, _) => {
                        self.need_trusted("dereferencing a raw pointer", span)?;
                        if *inner == Type::Unit {
                            return self.err("cannot dereference a pointer to ()", span);
                        }
                        Ok((rebuild(operand), *inner))
                    }
                    _ => self.err(
                        format!("cannot dereference a value of type {}", self.type_name(&t)),
                        span,
                    ),
                }
            }
            UnaryOp::Ref(mutable) => {
                if let ExprKind::Range { .. } = operand.kind {
                    return self.check_slice_range(take(operand), mutable, span);
                }
                let inner_hint = match expected {
                    Some(Type::Ref(t, _)) => Some((**t).clone()),
                    _ => None,
                };
                let t = self.check_expr(operand, inner_hint.as_ref())?;
                if t.is_ref_like() {
                    return self.err("references to references are not supported", span);
                }
                if t == Type::Unit || matches!(t, Type::Fn(_)) {
                    return self.err(
                        format!("cannot take a reference to {}", self.type_name(&t)),
                        span,
                    );
                }
                if self.contains_ref(&t) {
                    return self.err(
                        "references to values containing references are not supported",
                        span,
                    );
                }
                if mutable {
                    match self.place_mut(operand) {
                        Some(false) => {
                            let msg = match &operand.kind {
                                ExprKind::Var(n) => format!(
                                    "cannot borrow '{n}' as mutable (declare it with 'let mut')"
                                ),
                                _ => {
                                    "cannot borrow as mutable: the place is behind '&' or not 'mut'"
                                        .into()
                                }
                            };
                            return self.err(msg, span);
                        }
                        Some(true) | None => {}
                    }
                }
                Ok((rebuild(operand), Type::Ref(Box::new(t), mutable)))
            }
        }
    }

    fn check_slice_range(
        &mut self,
        range: Expr,
        mutable: bool,
        span: Span,
    ) -> TypeResult<(ExprKind, Type)> {
        let ExprKind::Range {
            base,
            mut start,
            mut end,
        } = range.kind
        else {
            unreachable!()
        };
        let mut base = *base;
        self.check_expr(&mut base, None)?;
        let base = self.auto_deref(base);
        let elem = match base.r#type() {
            Type::Array(t, _) => {
                if mutable && self.place_mut(&base) == Some(false) {
                    return self.err("cannot borrow as mutable: the array is not 'mut'", span);
                }
                (**t).clone()
            }
            Type::Slice(t, m) => {
                if mutable && !m {
                    return self.err("cannot reborrow a '&[T]' slice as '&mut'", span);
                }
                (**t).clone()
            }
            other => {
                return self.err(
                    format!("cannot slice a value of type {}", self.type_name(other)),
                    span,
                );
            }
        };
        if let Some(s) = &mut start {
            self.coerce(s, &Type::USIZE)?;
        }
        if let Some(e) = &mut end {
            self.coerce(e, &Type::USIZE)?;
        }
        if let Type::Array(_, n) = base.r#type() {
            let s = match &start {
                Some(s) => self.const_eval(s)?,
                None => Some(0),
            };
            let e = match &end {
                Some(e) => self.const_eval(e)?,
                None => Some(*n as i128),
            };
            if let (Some(s), Some(e)) = (s, e) {
                if s > e || e as u64 > *n {
                    return self.err(
                        format!("range {s}..{e} out of bounds for an array of length {n}"),
                        span,
                    );
                }
            }
        }
        Ok((
            ExprKind::Range {
                base: Box::new(base),
                start,
                end,
            },
            Type::Slice(Box::new(elem), mutable),
        ))
    }

    fn binary_result(
        &self,
        op: BinOp,
        lt: &Type,
        rt: &Type,
        rhs: &Expr,
        span: Span,
    ) -> TypeResult<Type> {
        let mismatch = || {
            self.err(
                format!(
                    "cannot apply '{}' to {} and {}{}",
                    op.symbol(),
                    self.type_name(lt),
                    self.type_name(rt),
                    if lt.is_int() && rt.is_int() {
                        " (use 'as' to convert)"
                    } else {
                        ""
                    }
                ),
                span,
            )
        };
        match op {
            BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Mod
            | BinOp::AddWrap
            | BinOp::SubWrap
            | BinOp::MulWrap => {
                if lt == rt && lt.is_int() {
                    Ok(lt.clone())
                } else if matches!(lt, Type::Raw(..)) {
                    self.err(
                        "pointer arithmetic is not allowed; use 'p.add(n)' in a #![trusted] file",
                        span,
                    )
                } else {
                    mismatch()
                }
            }
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                if lt == rt && (lt.is_int() || *lt == Type::Bool) {
                    Ok(lt.clone())
                } else {
                    mismatch()
                }
            }
            BinOp::Shl | BinOp::Shr => {
                let (Some(li), true) = (lt.int(), rt.is_int()) else {
                    return mismatch();
                };
                if let ExprKind::Int(n) = rhs.kind {
                    if n < 0 || n >= li.bits() as i128 {
                        return self.err(
                            format!("shift by {n} is out of range for {}", li.name()),
                            rhs.span,
                        );
                    }
                }
                Ok(lt.clone())
            }
            BinOp::Eq | BinOp::Ne => {
                if lt == rt
                    && matches!(
                        lt,
                        Type::Int(_) | Type::Bool | Type::Enum(_) | Type::Raw(..) | Type::Fn(_)
                    )
                {
                    Ok(Type::Bool)
                } else if lt == rt && lt.is_ref_like() {
                    self.err(
                        "cannot compare references; compare the values with '*a == *b'",
                        span,
                    )
                } else {
                    mismatch()
                }
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                if lt == rt && matches!(lt, Type::Int(_) | Type::Raw(..)) {
                    Ok(Type::Bool)
                } else {
                    mismatch()
                }
            }
            BinOp::And | BinOp::Or => unreachable!(),
        }
    }

    fn cast_ok(&self, from: &Type, to: &Type) -> bool {
        use Type::*;
        let ptr_int = |t: &Type| {
            matches!(
                t,
                Int(IntType::Usize | IntType::Isize | IntType::U64 | IntType::I64)
            )
        };
        match (from, to) {
            _ if from == to => true,
            (Int(_) | Bool | Enum(_), Int(_)) => true,
            (Raw(..), Raw(..)) => true,
            (Raw(..), t) | (t, Raw(..)) if ptr_int(t) => true,
            (Ref(a, m), Raw(b, m2)) => a == b && (*m || !*m2),
            (Fn(_), t) if ptr_int(t) => true,
            (Fn(_), Raw(..)) => true,
            (Str, Raw(t, false)) => **t == Type::U8,
            _ => false,
        }
    }

    fn check_call(
        &mut self,
        mut callee: Expr,
        mut args: Vec<Expr>,
        span: Span,
    ) -> TypeResult<(ExprKind, Type)> {
        if let ExprKind::Field { .. } = callee.kind {
            let ExprKind::Field { base, name } = callee.kind else {
                unreachable!()
            };
            return self.check_method_call(*base, name, args, span);
        }

        if let ExprKind::Var(name) = &callee.kind {
            if self.lookup_local(name).is_none() && is_builtin(name) {
                let name = name.clone();
                let ret = self.check_builtin(&name, &mut args, span)?;
                callee.r#type = Some(Type::Unit);
                return Ok((
                    ExprKind::Call {
                        callee: Box::new(callee),
                        args,
                        target: CallTarget::Builtin(name),
                    },
                    ret,
                ));
            }
        }

        let ct = self.check_expr(&mut callee, None)?;
        let direct = match &callee.kind {
            ExprKind::Var(n)
                if self.lookup_local(n).is_none() && self.functions.contains_key(n) =>
            {
                Some(n.clone())
            }
            _ => None,
        };
        let Type::Fn(ft) = ct else {
            return self.err(
                format!("cannot call a value of type {}", self.type_name(&ct)),
                span,
            );
        };
        match ft.abi {
            Abi::Interrupt => {
                return self.err("interrupt handlers cannot be called directly", span);
            }
            Abi::Efi => self.need_trusted("calling an 'extern fn' (firmware code)", span)?,
            _ => {}
        }
        self.check_args(&ft.params, &mut args, span)?;
        let target = match direct {
            Some(name) => CallTarget::Direct(name),
            None => CallTarget::Indirect,
        };
        Ok((
            ExprKind::Call {
                callee: Box::new(callee),
                args,
                target,
            },
            ft.ret.clone(),
        ))
    }

    fn check_args(&mut self, params: &[Type], args: &mut [Expr], span: Span) -> TypeResult<()> {
        if params.len() != args.len() {
            return self.err(
                format!("expected {} argument(s), got {}", params.len(), args.len()),
                span,
            );
        }
        for (p, a) in params.iter().zip(args.iter_mut()) {
            self.coerce(a, p)?;
        }
        Ok(())
    }

    fn check_method_call(
        &mut self,
        mut base: Expr,
        name: String,
        mut args: Vec<Expr>,
        span: Span,
    ) -> TypeResult<(ExprKind, Type)> {
        if let ExprKind::Range { .. } = base.kind {
            let bspan = base.span;
            base = Expr::new(
                ExprKind::Unary {
                    op: UnaryOp::Ref(false),
                    operand: Box::new(base),
                },
                bspan,
            );
        }
        let bt = self.check_expr(&mut base, None)?;
        let target_ty = match &bt {
            Type::Ref(inner, _) => (**inner).clone(),
            t => t.clone(),
        };

        if let Type::Struct(sid) = target_ty {
            let field = self
                .layout(sid)
                .fields
                .iter()
                .find(|f| f.name == name)
                .cloned();
            if let Some(f) = field {
                if let Type::Fn(_) = f.r#type {
                    let (fbase, fty) = self.make_field(base, &name, span)?;
                    let callee = Expr {
                        kind: ExprKind::Field {
                            base: Box::new(fbase),
                            name,
                        },
                        r#type: Some(fty),
                        span,
                    };
                    return self.finish_indirect(callee, args, span);
                }
            }
        }

        let owner = match &target_ty {
            Type::Struct(id) => Some(self.struct_defs[*id].name.clone()),
            Type::Enum(id) => Some(self.enum_defs[*id].name.clone()),
            _ => None,
        };
        if let Some(owner) = owner {
            let full = format!("{owner}::{name}");
            if let Some(sig) = self.functions.get(&full).cloned() {
                let Some(recv_ty) = sig.params.first() else {
                    return self.err(
                        format!("'{full}' has no 'self' parameter; call it as '{full}(...)'"),
                        span,
                    );
                };
                let recv = self.adjust_receiver(base, &bt, recv_ty, &full, span)?;
                args.insert(0, recv);
                if sig.params.len() != args.len() {
                    return self.err(
                        format!(
                            "'{full}' expects {} argument(s), got {}",
                            sig.params.len() - 1,
                            args.len() - 1
                        ),
                        span,
                    );
                }
                for (p, a) in sig.params.iter().zip(args.iter_mut()).skip(1) {
                    self.coerce(a, p)?;
                }
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
                    sig.ret.clone(),
                ));
            }
        }

        self.check_builtin_method(base, bt, name, args, span)
    }

    fn finish_indirect(
        &mut self,
        callee: Expr,
        mut args: Vec<Expr>,
        span: Span,
    ) -> TypeResult<(ExprKind, Type)> {
        let Type::Fn(ft) = callee.r#type().clone() else {
            unreachable!()
        };
        match ft.abi {
            Abi::Interrupt => {
                return self.err("interrupt handlers cannot be called directly", span);
            }
            Abi::Efi => self.need_trusted("calling an 'extern fn' (firmware code)", span)?,
            _ => {}
        }
        self.check_args(&ft.params, &mut args, span)?;
        Ok((
            ExprKind::Call {
                callee: Box::new(callee),
                args,
                target: CallTarget::Indirect,
            },
            ft.ret,
        ))
    }

    fn adjust_receiver(
        &mut self,
        base: Expr,
        bt: &Type,
        recv_ty: &Type,
        full: &str,
        span: Span,
    ) -> TypeResult<Expr> {
        match recv_ty {
            Type::Ref(t, m) => {
                if let Type::Ref(bi, bm) = bt {
                    if bi == t {
                        if *m && !bm {
                            return self.err(
                                format!(
                                    "'{full}' needs '&mut self' but the receiver is a '&' reference"
                                ),
                                span,
                            );
                        }
                        let mut base = base;
                        base.r#type = Some(recv_ty.clone());
                        return Ok(base);
                    }
                }
                if bt == &**t {
                    if *m && self.place_mut(&base) == Some(false) {
                        let msg = match &base.kind {
                            ExprKind::Var(n) => {
                                format!("'{full}' needs '&mut self': declare '{n}' with 'let mut'")
                            }
                            _ => format!(
                                "'{full}' needs '&mut self' but the receiver is not mutable"
                            ),
                        };
                        return self.err(msg, span);
                    }
                    let span = base.span;
                    return Ok(Expr {
                        kind: ExprKind::Unary {
                            op: UnaryOp::Ref(*m),
                            operand: Box::new(base),
                        },
                        r#type: Some(recv_ty.clone()),
                        span,
                    });
                }
                self.err(format!("wrong receiver type for '{full}'"), span)
            }
            t if t == bt => Ok(base),
            t if matches!(bt, Type::Ref(inner, _) if &**inner == t) => Ok(self.auto_deref(base)),
            _ => self.err(format!("wrong receiver type for '{full}'"), span),
        }
    }

    fn check_builtin_method(
        &mut self,
        base: Expr,
        bt: Type,
        name: String,
        mut args: Vec<Expr>,
        span: Span,
    ) -> TypeResult<(ExprKind, Type)> {
        let base = self.auto_deref(base);
        let bt_inner = base.r#type().clone();
        let need_args = |n: usize, args: &Vec<Expr>| -> TypeResult<()> {
            if args.len() != n {
                return self.err(
                    format!("'.{name}()' expects {n} argument(s), got {}", args.len()),
                    span,
                );
            }
            Ok(())
        };
        let ret = match (&bt_inner, name.as_str()) {
            (Type::Array(..) | Type::Slice(..) | Type::Str, "len") => {
                need_args(0, &args)?;
                Type::USIZE
            }
            (Type::Array(t, _) | Type::Slice(t, _), "as_ptr") => {
                need_args(0, &args)?;
                Type::raw((**t).clone(), false)
            }
            (Type::Str, "as_ptr") => {
                need_args(0, &args)?;
                Type::raw(Type::U8, false)
            }
            (Type::Array(t, _), "as_mut_ptr") => {
                need_args(0, &args)?;
                let mutable = match &bt {
                    Type::Ref(_, m) => *m,
                    _ => self.place_mut(&base) != Some(false),
                };
                if !mutable {
                    return self.err("'.as_mut_ptr()' needs a mutable array", span);
                }
                Type::raw((**t).clone(), true)
            }
            (Type::Slice(t, m), "as_mut_ptr") => {
                need_args(0, &args)?;
                if !m {
                    return self.err("'.as_mut_ptr()' needs a '&mut [T]' slice", span);
                }
                Type::raw((**t).clone(), true)
            }
            (Type::Raw(..), "is_null") => {
                need_args(0, &args)?;
                Type::Bool
            }
            (Type::Raw(t, m), "add" | "sub" | "offset") => {
                need_args(1, &args)?;
                self.need_trusted(&format!("'.{name}()' on a raw pointer"), span)?;
                let it = if name == "offset" {
                    Type::Int(IntType::Isize)
                } else {
                    Type::USIZE
                };
                self.coerce(&mut args[0], &it)?;
                Type::Raw(t.clone(), *m)
            }
            (Type::Raw(t, _), "read") => {
                need_args(0, &args)?;
                self.need_trusted("'.read()' on a raw pointer", span)?;
                if **t == Type::Unit {
                    return self.err("cannot read through a pointer to ()", span);
                }
                (**t).clone()
            }
            (Type::Raw(t, m), "write") => {
                need_args(1, &args)?;
                self.need_trusted("'.write()' on a raw pointer", span)?;
                if !m {
                    return self.err("cannot write through a '*const' pointer", span);
                }
                let t = (**t).clone();
                self.coerce(&mut args[0], &t)?;
                Type::Unit
            }
            _ => {
                return self.err(
                    format!("no method '{name}' on type {}", self.type_name(&bt)),
                    span,
                );
            }
        };
        let mut all = vec![base];
        all.extend(args);
        let callee = Expr {
            kind: ExprKind::Var(format!(".{name}")),
            r#type: Some(Type::Unit),
            span,
        };
        Ok((
            ExprKind::Call {
                callee: Box::new(callee),
                args: all,
                target: CallTarget::Builtin(format!(".{name}")),
            },
            ret,
        ))
    }

    fn check_builtin(&mut self, name: &str, args: &mut [Expr], span: Span) -> TypeResult<Type> {
        if name == "print" {
            for a in args.iter_mut() {
                let t = self.check_expr(a, None)?;
                if !matches!(t, Type::Int(_) | Type::Bool | Type::Str | Type::Enum(_)) {
                    return self.err(
                        format!("cannot print a value of type {}", self.type_name(&t)),
                        a.span,
                    );
                }
            }
            return Ok(Type::Unit);
        }
        let (params, ret, trusted_only) = builtin_sig(name).unwrap();
        if trusted_only {
            self.need_trusted(&format!("'{name}'"), span)?;
        }
        if name == "lgdt" || name == "lidt" {
            if args.len() != 1 {
                return self.err(format!("'{name}' expects 1 argument"), span);
            }
            let t = self.check_expr(&mut args[0], None)?;
            if !matches!(t, Type::Ref(..) | Type::Raw(..)) {
                return self.err(
                    format!("'{name}' expects a pointer to the descriptor table register"),
                    args[0].span,
                );
            }
            return Ok(Type::Unit);
        }
        self.check_args(&params, args, span)?;
        Ok(ret)
    }

    fn const_eval(&self, e: &Expr) -> TypeResult<Option<i128>> {
        let it = |t: &Type| match t {
            Type::Int(i) => Some(*i),
            Type::Enum(id) => Some(self.enum_info(*id).repr),
            _ => None,
        };
        let check = |v: i128, t: &Type, span: Span| -> TypeResult<Option<i128>> {
            match it(t) {
                Some(i) if v < i.min() || v > i.max() => self.err(
                    format!(
                        "arithmetic overflow in constant expression: {v} is out of range for {}",
                        i.name()
                    ),
                    span,
                ),
                _ => Ok(Some(v)),
            }
        };
        Ok(match &e.kind {
            ExprKind::Int(v) => Some(*v),
            ExprKind::Bool(b) => Some(*b as i128),
            ExprKind::Unary { op, operand } => {
                let Some(v) = self.const_eval(operand)? else {
                    return Ok(None);
                };
                match op {
                    UnaryOp::Neg => return check(-v, e.r#type(), e.span),
                    UnaryOp::Not => Some((v == 0) as i128),
                    UnaryOp::BitNot => Some(self.wrap_int(!v, it(e.r#type()).unwrap())),
                    _ => None,
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (Some(a), Some(b)) = (self.const_eval(lhs)?, self.const_eval(rhs)?) else {
                    return Ok(None);
                };
                let lt = lhs.r#type();
                match op {
                    BinOp::Add => return check(a + b, lt, e.span),
                    BinOp::Sub => return check(a - b, lt, e.span),
                    BinOp::Mul => return check(a.checked_mul(b).unwrap_or(i128::MAX), lt, e.span),
                    BinOp::Div | BinOp::Mod => {
                        if b == 0 {
                            return self.err("division by zero in constant expression", e.span);
                        }
                        let v = if *op == BinOp::Div { a / b } else { a % b };
                        return check(v, lt, e.span);
                    }
                    BinOp::AddWrap => Some(self.wrap_int(a + b, it(lt).unwrap())),
                    BinOp::SubWrap => Some(self.wrap_int(a - b, it(lt).unwrap())),
                    BinOp::MulWrap => Some(self.wrap_int(a.wrapping_mul(b), it(lt).unwrap())),
                    BinOp::BitAnd => Some(a & b),
                    BinOp::BitOr => Some(a | b),
                    BinOp::BitXor => Some(a ^ b),
                    BinOp::Shl => Some(self.wrap_int(a << b, it(lt).unwrap())),
                    BinOp::Shr => Some(a >> b),
                    BinOp::Eq => Some((a == b) as i128),
                    BinOp::Ne => Some((a != b) as i128),
                    BinOp::Lt => Some((a < b) as i128),
                    BinOp::Le => Some((a <= b) as i128),
                    BinOp::Gt => Some((a > b) as i128),
                    BinOp::Ge => Some((a >= b) as i128),
                    BinOp::And => Some((a != 0 && b != 0) as i128),
                    BinOp::Or => Some((a != 0 || b != 0) as i128),
                }
            }
            ExprKind::Cast { expr, .. } => {
                let Some(v) = self.const_eval(expr)? else {
                    return Ok(None);
                };
                match e.r#type() {
                    Type::Int(i) => Some(self.wrap_int(v, *i)),
                    Type::Raw(..) => Some(self.wrap_int(v, IntType::U64)),
                    _ => None,
                }
            }
            _ => None,
        })
    }

    fn const_bytes(&self, e: &Expr, out: &mut [u8], off: usize) -> TypeResult<()> {
        let size_of = |t: &Type| -> u64 {
            match t {
                Type::Bool => 1,
                Type::Int(i) => i.size(),
                Type::Enum(id) => self.enum_info(*id).repr.size(),
                Type::Struct(id) => self.layout(*id).size,
                Type::Array(..) => unreachable!(),
                _ => 8,
            }
        };
        match &e.kind {
            ExprKind::StructLit { name, fields } => {
                let sid = self.struct_ids[name];
                for (fname, value, _) in fields {
                    let fl = self
                        .layout(sid)
                        .fields
                        .iter()
                        .find(|f| f.name == *fname)
                        .unwrap();
                    self.const_bytes(value, out, off + fl.offset as usize)?;
                }
                Ok(())
            }
            ExprKind::ArrayLit(items) => {
                let stride = self.elem_size(e.r#type());
                for (i, item) in items.iter().enumerate() {
                    self.const_bytes(item, out, off + i * stride)?;
                }
                Ok(())
            }
            ExprKind::ArrayRepeat { value, .. } => {
                let Type::Array(_, n) = e.r#type() else {
                    unreachable!()
                };
                let stride = self.elem_size(e.r#type());
                self.const_bytes(value, out, off)?;
                for i in 1..*n as usize {
                    out.copy_within(off..off + stride, off + i * stride);
                }
                Ok(())
            }
            ExprKind::Str(_) | ExprKind::CStr(_) | ExprKind::WStr(_) => self.err(
                "string literals cannot initialize statics yet (their address needs relocation); assign it at runtime",
                e.span,
            ),
            _ => match self.const_eval(e)? {
                Some(v) => {
                    let size = size_of(e.r#type()) as usize;
                    let bytes = (v as i64 as u64).to_le_bytes();
                    out[off..off + size].copy_from_slice(&bytes[..size]);
                    Ok(())
                }
                None => self.err("static initializers must be constant expressions", e.span),
            },
        }
    }

    fn elem_size(&self, t: &Type) -> usize {
        let Type::Array(elem, _) = t else {
            unreachable!()
        };
        fn size(tc: &TypeChecker, t: &Type) -> u64 {
            match t {
                Type::Unit => 0,
                Type::Bool => 1,
                Type::Int(i) => i.size(),
                Type::Enum(id) => tc.enum_info(*id).repr.size(),
                Type::Struct(id) => tc.layout(*id).size,
                Type::Array(e, n) => size(tc, e) * n,
                Type::Slice(..) => 16,
                _ => 8,
            }
        }
        size(self, elem) as usize
    }
}

fn builtin_sig(name: &str) -> Option<(Vec<Type>, Type, bool)> {
    use IntType::*;
    let u = |i| Type::Int(i);
    let raw_mut_u8 = Type::raw(Type::U8, true);
    let raw_u8 = Type::raw(Type::U8, false);
    Some(match name {
        "panic" => (vec![Type::Str], Type::Unit, false),
        "exit" => (vec![u(I32)], Type::Unit, false),
        "outb" => (vec![u(U16), u(U8)], Type::Unit, true),
        "outw" => (vec![u(U16), u(U16)], Type::Unit, true),
        "outl" => (vec![u(U16), u(U32)], Type::Unit, true),
        "inb" => (vec![u(U16)], u(U8), true),
        "inw" => (vec![u(U16)], u(U16), true),
        "inl" => (vec![u(U16)], u(U32), true),
        "cli" | "sti" | "hlt" | "pause" | "int3" => (vec![], Type::Unit, false),
        "read_cr0" | "read_cr2" | "read_cr3" | "read_cr4" | "rdtsc" => (vec![], u(U64), false),
        "write_cr0" | "write_cr3" | "write_cr4" => (vec![u(U64)], Type::Unit, true),
        "rdmsr" => (vec![u(U32)], u(U64), true),
        "wrmsr" => (vec![u(U32), u(U64)], Type::Unit, true),
        "invlpg" => (vec![u(Usize)], Type::Unit, false),
        "load_cs" | "load_ds" | "ltr" => (vec![u(U16)], Type::Unit, true),
        "memcpy" => (vec![raw_mut_u8, raw_u8, u(Usize)], Type::Unit, true),
        "memset" => (vec![raw_mut_u8, u(U8), u(Usize)], Type::Unit, true),
        "efi_image_handle" | "efi_system_table" => (vec![], Type::raw(Type::U8, true), false),
        "switch_stack" => (
            vec![
                u(Usize),
                Type::Fn(Box::new(FnType {
                    params: vec![],
                    ret: Type::Unit,
                    abi: Abi::Native,
                })),
            ],
            Type::Unit,
            true,
        ),
        "lgdt" | "lidt" => (vec![], Type::Unit, true),
        _ => return None,
    })
}

pub fn is_builtin(name: &str) -> bool {
    name == "print" || builtin_sig(name).is_some()
}

fn diverging_builtin(name: &str) -> bool {
    matches!(name, "panic" | "exit" | "switch_stack")
}

fn take(e: &mut Expr) -> Expr {
    replace(e, Expr::new(ExprKind::Bool(false), e.span))
}
