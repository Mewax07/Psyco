use crate::lexer::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntType {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    Isize,
    Usize,
}

impl IntType {
    pub fn size(self) -> u64 {
        match self {
            IntType::I8 | IntType::U8 => 1,
            IntType::I16 | IntType::U16 => 2,
            IntType::I32 | IntType::U32 => 4,
            IntType::I64 | IntType::U64 | IntType::Isize | IntType::Usize => 8,
        }
    }

    pub fn signed(self) -> bool {
        matches!(
            self,
            IntType::I8 | IntType::I16 | IntType::I32 | IntType::I64 | IntType::Isize
        )
    }

    pub fn bits(self) -> u32 {
        self.size() as u32 * 8
    }

    pub fn min(self) -> i128 {
        if self.signed() {
            -(1i128 << (self.bits() - 1))
        } else {
            0
        }
    }

    pub fn max(self) -> i128 {
        if self.signed() {
            (1i128 << (self.bits() - 1)) - 1
        } else {
            (1i128 << self.bits()) - 1
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            IntType::I8 => "i8",
            IntType::I16 => "i16",
            IntType::I32 => "i32",
            IntType::I64 => "i64",
            IntType::U8 => "u8",
            IntType::U16 => "u16",
            IntType::U32 => "u32",
            IntType::U64 => "u64",
            IntType::Isize => "isize",
            IntType::Usize => "usize",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Abi {
    Native,
    Efi,
    Interrupt,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FnType {
    pub params: Vec<Type>,
    pub ret: Type,
    pub abi: Abi,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    Unit,
    Bool,
    Str,
    Int(IntType),
    Ref(Box<Type>, bool),
    Raw(Box<Type>, bool),
    Slice(Box<Type>, bool),
    Array(Box<Type>, u64),
    Struct(usize),
    Enum(usize),
    Sum(usize),
    Fn(Box<FnType>),
}

impl Type {
    pub const I64: Type = Type::Int(IntType::I64);
    pub const U64: Type = Type::Int(IntType::U64);
    pub const U8: Type = Type::Int(IntType::U8);
    pub const USIZE: Type = Type::Int(IntType::Usize);

    pub fn raw(to: Type, mutable: bool) -> Type {
        Type::Raw(Box::new(to), mutable)
    }

    pub fn reference(to: Type, mutable: bool) -> Type {
        Type::Ref(Box::new(to), mutable)
    }

    pub fn int(&self) -> Option<IntType> {
        match self {
            Type::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn is_int(&self) -> bool {
        matches!(self, Type::Int(_))
    }

    pub fn pointee(&self) -> Option<&Type> {
        match self {
            Type::Ref(t, _) | Type::Raw(t, _) => Some(t),
            _ => None,
        }
    }

    pub fn is_ref_like(&self) -> bool {
        matches!(self, Type::Ref(..) | Type::Slice(..))
    }

    pub fn is_aggregate(&self) -> bool {
        matches!(
            self,
            Type::Struct(_) | Type::Array(..) | Type::Slice(..) | Type::Sum(_)
        )
    }
}

#[derive(Debug, Clone)]
pub struct Attr {
    pub name: String,
    pub arg: Option<u64>,
    pub span: Span,
}

pub fn has_attr(attrs: &[Attr], name: &str) -> bool {
    attrs.iter().any(|a| a.name == name)
}

pub fn attr_arg(attrs: &[Attr], name: &str) -> Option<u64> {
    attrs.iter().find(|a| a.name == name).and_then(|a| a.arg)
}

#[derive(Debug, Clone)]
pub struct GenericParam {
    pub name: String,
    pub bounds: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum TypeExprKind {
    Named(String),
    Generic(String, Vec<TypeExpr>),
    Resolved(Type),
    Unit,
    Ref {
        inner: Box<TypeExpr>,
        mutable: bool,
    },
    Raw {
        inner: Box<TypeExpr>,
        mutable: bool,
    },
    Slice {
        elem: Box<TypeExpr>,
        mutable: bool,
    },
    Array(Box<TypeExpr>, Box<Expr>),
    Fn {
        params: Vec<TypeExpr>,
        ret: Option<Box<TypeExpr>>,
        efi: bool,
    },
}

#[derive(Debug, Clone)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    AddWrap,
    SubWrap,
    MulWrap,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        )
    }

    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::AddWrap => "+%",
            BinOp::SubWrap => "-%",
            BinOp::MulWrap => "*%",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
    Ref(bool),
    Deref,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CallTarget {
    Unresolved,
    Direct(String),
    Indirect,
    Builtin(String),
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub r#type: Option<Type>,
    pub span: Span,
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Self {
        Expr {
            kind,
            r#type: None,
            span,
        }
    }

    pub fn r#type(&self) -> &Type {
        self.r#type.as_ref().expect("ICE: expression not typed")
    }
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int(i128),
    TypedInt(i128, IntType),
    Bool(bool),
    Str(Vec<u8>),
    CStr(Vec<u8>),
    Bytes(Vec<u8>),
    WStr(Vec<u16>),
    Var(String),
    Path(String, String),
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Cast {
        expr: Box<Expr>,
        r#type: TypeExpr,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        target: CallTarget,
    },
    Field {
        base: Box<Expr>,
        name: String,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    StructLit {
        name: String,
        args: Vec<TypeExpr>,
        fields: Vec<(String, Expr, Span)>,
    },
    Turbofish {
        owner: String,
        owner_args: Vec<TypeExpr>,
        member: Option<String>,
        member_args: Vec<TypeExpr>,
    },
    ArrayLit(Vec<Expr>),
    ArrayRepeat {
        value: Box<Expr>,
        count: Box<Expr>,
    },
    Sizeof(TypeExpr),
    Range {
        base: Box<Expr>,
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
    },
    Unsize(Box<Expr>),
    TypePath {
        ty: TypeExpr,
        member: String,
        member_args: Vec<TypeExpr>,
    },
    Try(Box<Expr>),
    TryOp {
        inner: Box<Expr>,
        ok: usize,
        fail: usize,
        ret_sum: usize,
        ret_fail: usize,
    },
    VariantLit {
        sum: usize,
        variant: usize,
        args: Vec<Expr>,
    },
}

#[derive(Debug, Clone)]
pub enum PatKind {
    Wild,
    Ident(String),
    MutBind(String),
    Path(String, String),
    Variant {
        owner: Option<String>,
        name: String,
        args: Vec<Pattern>,
    },
    Lit(Box<Expr>),
    Or(Vec<Pattern>),
    Bind {
        name: String,
        mutable: bool,
        by_ref: Option<bool>,
    },
    Const(i128),
    Ctor {
        sum: usize,
        variant: usize,
        args: Vec<Pattern>,
    },
}

#[derive(Debug, Clone)]
pub struct Pattern {
    pub kind: PatKind,
    pub span: Span,
    pub r#type: Option<Type>,
    pub deref: bool,
}

impl Pattern {
    pub fn new(kind: PatKind, span: Span) -> Self {
        Pattern {
            kind,
            span,
            r#type: None,
            deref: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pats: Vec<Pattern>,
    pub guard: Option<Expr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        name: String,
        mutable: bool,
        expr_type: Option<TypeExpr>,
        value: Option<Expr>,
        span: Span,
        r#type: Option<Type>,
    },
    Assign {
        target: Expr,
        op: Option<BinOp>,
        value: Expr,
        span: Span,
    },
    Expr(Expr),
    Return {
        value: Option<Expr>,
        span: Span,
    },
    If {
        cond: Expr,
        then_block: Block,
        else_block: Option<Block>,
        span: Span,
    },
    While {
        cond: Expr,
        body: Block,
        span: Span,
    },
    Loop {
        body: Block,
        span: Span,
    },
    For {
        var: String,
        start: Expr,
        end: Expr,
        body: Block,
        span: Span,
    },
    Break(Span),
    Continue(Span),
    Match {
        scrutinee: Expr,
        arms: Vec<MatchArm>,
        span: Span,
    },
    IfLet {
        pat: Pattern,
        value: Expr,
        then_block: Block,
        else_block: Option<Block>,
        span: Span,
    },
    WhileLet {
        pat: Pattern,
        value: Expr,
        body: Block,
        span: Span,
    },
    LetElse {
        pat: Pattern,
        value: Expr,
        else_block: Block,
        span: Span,
    },
    Block(Block),
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub mutable: bool,
    pub r#type: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub module: String,
    pub generics: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    pub body: Block,
    pub attrs: Vec<Attr>,
    pub trusted: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: String,
    pub r#type: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    pub module: String,
    pub generics: Vec<GenericParam>,
    pub fields: Vec<FieldDef>,
    pub attrs: Vec<Attr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Variant {
    pub name: String,
    pub value: Option<Expr>,
    pub fields: Option<Vec<TypeExpr>>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    pub module: String,
    pub generics: Vec<GenericParam>,
    pub repr: Option<TypeExpr>,
    pub variants: Vec<Variant>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct StaticDef {
    pub name: String,
    pub module: String,
    pub mutable: bool,
    pub r#type: TypeExpr,
    pub init: Option<Expr>,
    pub attrs: Vec<Attr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ConstDef {
    pub name: String,
    pub module: String,
    pub r#type: Option<TypeExpr>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ImplDef {
    pub module: String,
    pub generics: Vec<GenericParam>,
    pub self_ty: TypeExpr,
    pub trait_name: Option<String>,
    pub methods: Vec<Function>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TraitDef {
    pub name: String,
    pub module: String,
    pub methods: Vec<(Function, bool)>,
    pub span: Span,
}

#[derive(Debug, Clone, Default)]
pub struct Program {
    pub traits: Vec<TraitDef>,
    pub imports: Vec<(String, Option<String>, Span)>,
    pub modules: Vec<String>,
    pub functions: Vec<Function>,
    pub impls: Vec<ImplDef>,
    pub structs: Vec<StructDef>,
    pub enums: Vec<EnumDef>,
    pub statics: Vec<StaticDef>,
    pub consts: Vec<ConstDef>,
}

impl Program {
    pub fn merge(&mut self, other: Program) {
        self.functions.extend(other.functions);
        self.impls.extend(other.impls);
        self.traits.extend(other.traits);
        for m in other.modules {
            if !self.modules.contains(&m) {
                self.modules.push(m);
            }
        }
        self.structs.extend(other.structs);
        self.enums.extend(other.enums);
        self.statics.extend(other.statics);
        self.consts.extend(other.consts);
    }
}
