use std::mem::take;

use crate::*;

pub struct ParserError {
    pub message: String,
    pub span: Span,
}

type ParserResult<T> = Result<T, ParserError>;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    no_struct: bool,
    trusted: bool,
    last_line: usize,
    trait_sig: bool,
    module: String,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            no_struct: false,
            trusted: false,
            last_line: 0,
            trait_sig: false,
            module: String::new(),
        }
    }

    pub fn with_module(mut self, module: String) -> Self {
        self.module = module;
        self
    }

    pub fn parse_program(&mut self) -> ParserResult<Program> {
        let mut program = Program::default();
        if self.check(&TokenKind::Hash) && *self.peek_at(1) == TokenKind::Bang {
            self.advance();
            self.advance();
            self.expect(TokenKind::LBracket, "'['")?;
            let (name, span) = self.expect_ident()?;
            if name != "trusted" {
                return Err(ParserError {
                    message: format!("unknown file attribute '{name}' (only #![trusted] exists)"),
                    span,
                });
            }
            self.expect(TokenKind::RBracket, "']'")?;
            self.trusted = true;
        }
        if !self.module.is_empty() {
            program.modules.push(self.module.clone());
        }
        self.parse_items(&mut program, false)?;
        Ok(program)
    }

    fn parse_items(&mut self, program: &mut Program, in_block: bool) -> ParserResult<()> {
        loop {
            if self.check(&TokenKind::Eof) {
                if in_block {
                    return Err(self.error_here("'}' expected at the end of the module"));
                }
                return Ok(());
            }
            if in_block && self.eat(&TokenKind::RBrace) {
                return Ok(());
            }
            let attrs = self.parse_attrs()?;
            let span = self.peek().span;
            match &self.peek().kind {
                TokenKind::Ident(m)
                    if m == "mod"
                        && matches!(self.peek_at(1), TokenKind::Ident(_))
                        && *self.peek_at(2) == TokenKind::LBrace =>
                {
                    self.no_attrs(&attrs, "mod")?;
                    self.advance();
                    let (name, _) = self.expect_ident()?;
                    self.advance();
                    let saved = self.module.clone();
                    self.module = if saved.is_empty() {
                        name
                    } else {
                        format!("{saved}::{name}")
                    };
                    if !program.modules.contains(&self.module) {
                        program.modules.push(self.module.clone());
                    }
                    let r = self.parse_items(program, true);
                    self.module = saved;
                    r?;
                }
                TokenKind::Fn => {
                    let f = self.parse_function(attrs, None)?;
                    program.functions.push(f);
                }
                TokenKind::Struct => program.structs.push(self.parse_struct(attrs)?),
                TokenKind::Enum => {
                    self.no_attrs(&attrs, "enum")?;
                    program.enums.push(self.parse_enum()?);
                }
                TokenKind::Impl => {
                    self.no_attrs(&attrs, "impl")?;
                    self.parse_impl(program)?;
                }
                TokenKind::Trait => {
                    self.no_attrs(&attrs, "trait")?;
                    program.traits.push(self.parse_trait()?);
                }
                TokenKind::Static => {
                    let s = self.parse_static(attrs)?;
                    program.functions.extend(self.static_accessors(&s));
                    program.statics.push(s);
                }
                TokenKind::Const => {
                    self.no_attrs(&attrs, "const")?;
                    program.consts.push(self.parse_const()?);
                }
                TokenKind::Import => {
                    self.no_attrs(&attrs, "import")?;
                    self.advance();
                    let token = self.advance();
                    let TokenKind::Str(path) = token.kind else {
                        return Err(ParserError {
                            message: "import expects a string path".into(),
                            span: token.span,
                        });
                    };
                    let path = String::from_utf8(path).map_err(|_| ParserError {
                        message: "import path must be UTF-8".into(),
                        span: token.span,
                    })?;
                    let alias = if self.eat(&TokenKind::As) {
                        Some(self.expect_ident()?.0)
                    } else {
                        None
                    };
                    program.imports.push((path, alias, span));
                }
                TokenKind::Unsafe => {
                    return Err(self.error_here(
                        "'unsafe' does not exist in this language: put low-level code in a file that starts with #![trusted]",
                    ));
                }
                other => {
                    return Err(self.error_here(format!(
                        "expected 'fn', 'struct', 'enum', 'impl', 'trait', 'mod', 'static', 'const' or 'import', found {other:?}"
                    )));
                }
            }
            self.eat(&TokenKind::Semicolon);
        }
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn peek_at(&self, n: usize) -> &TokenKind {
        let i = (self.pos + n).min(self.tokens.len() - 1);
        &self.tokens[i].kind
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens[self.pos].clone();
        if token.kind != TokenKind::Eof {
            self.pos += 1;
        }
        self.last_line = token.span.line;
        token
    }

    fn check(&self, kind: &TokenKind) -> bool {
        &self.peek().kind == kind
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: TokenKind, what: &str) -> ParserResult<Token> {
        if self.check(&kind) {
            Ok(self.advance())
        } else {
            Err(self.error_here(format!("{what} expected, found {:?}", self.peek().kind)))
        }
    }

    fn expect_ident(&mut self) -> ParserResult<(String, Span)> {
        let token = self.advance();
        match token.kind {
            TokenKind::Ident(name) => Ok((name, token.span)),
            other => Err(ParserError {
                message: format!("identifier expected, found {other:?}"),
                span: token.span,
            }),
        }
    }

    fn error_here(&self, message: impl Into<String>) -> ParserError {
        ParserError {
            message: message.into(),
            span: self.peek().span,
        }
    }

    fn no_attrs(&self, attrs: &[Attr], what: &str) -> ParserResult<()> {
        match attrs.first() {
            Some(a) => Err(ParserError {
                message: format!("attributes are not allowed on {what}"),
                span: a.span,
            }),
            None => Ok(()),
        }
    }

    fn continues_expr(&self) -> bool {
        let ambiguous = matches!(
            self.peek().kind,
            TokenKind::Star
                | TokenKind::Minus
                | TokenKind::Amp
                | TokenKind::AndAnd
                | TokenKind::LParen
                | TokenKind::LBracket
        );
        !ambiguous || self.peek().span.line == self.last_line
    }

    fn with_no_struct<T>(
        &mut self,
        value: bool,
        f: impl FnOnce(&mut Self) -> ParserResult<T>,
    ) -> ParserResult<T> {
        let saved = self.no_struct;
        self.no_struct = value;
        let result = f(self);
        self.no_struct = saved;
        result
    }

    fn parse_attrs(&mut self) -> ParserResult<Vec<Attr>> {
        let mut attrs = Vec::new();
        while self.check(&TokenKind::Hash) {
            self.advance();
            self.expect(TokenKind::LBracket, "'['")?;
            loop {
                let (name, span) = self.expect_ident()?;
                let arg = if self.eat(&TokenKind::LParen) {
                    let token = self.advance();
                    let TokenKind::Int(n) = token.kind else {
                        return Err(ParserError {
                            message: "attribute argument must be an integer".into(),
                            span: token.span,
                        });
                    };
                    self.expect(TokenKind::RParen, "')'")?;
                    Some(n)
                } else {
                    None
                };
                attrs.push(Attr { name, arg, span });
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(TokenKind::RBracket, "']'")?;
        }
        Ok(attrs)
    }

    fn parse_function(
        &mut self,
        attrs: Vec<Attr>,
        self_type: Option<&str>,
    ) -> ParserResult<Function> {
        let span = self.peek().span;
        self.expect(TokenKind::Fn, "'fn'")?;
        let (name, _) = self.expect_ident()?;
        let generics = self.parse_generic_params()?;

        self.expect(TokenKind::LParen, "'('")?;
        let mut params = Vec::new();
        while !self.check(&TokenKind::RParen) {
            params.push(self.parse_param(self_type, params.is_empty())?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RParen, "')'")?;

        let return_type = if self.eat(&TokenKind::Arrow) {
            Some(self.parse_type()?)
        } else {
            None
        };

        let body = if self.trait_sig && !self.check(&TokenKind::LBrace) {
            self.eat(&TokenKind::Semicolon);
            Block {
                stmts: Vec::new(),
                span,
            }
        } else {
            self.parse_block()?
        };
        Ok(Function {
            name,
            module: self.module.clone(),
            generics,
            params,
            return_type,
            body,
            attrs,
            trusted: self.trusted,
            span,
        })
    }

    fn parse_generic_params(&mut self) -> ParserResult<Vec<GenericParam>> {
        let mut out = Vec::new();
        if !self.eat(&TokenKind::Lt) {
            return Ok(out);
        }
        while !self.check(&TokenKind::Gt) {
            let (name, span) = self.expect_ident()?;
            let mut bounds = Vec::new();
            if self.eat(&TokenKind::Colon) {
                loop {
                    let (b, _) = self.expect_ident()?;
                    bounds.push(b);
                    if !self.eat(&TokenKind::Plus) {
                        break;
                    }
                }
            }
            if out.iter().any(|g: &GenericParam| g.name == name) {
                return Err(ParserError {
                    message: format!("type parameter '{name}' declared twice"),
                    span,
                });
            }
            out.push(GenericParam { name, bounds, span });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect_gt()?;
        Ok(out)
    }

    fn expect_gt(&mut self) -> ParserResult<()> {
        if self.eat(&TokenKind::Gt) {
            return Ok(());
        }
        if self.check(&TokenKind::Shr) {
            self.tokens[self.pos].kind = TokenKind::Gt;
            return Ok(());
        }
        Err(self.error_here(format!("'>' expected, found {:?}", self.peek().kind)))
    }

    fn parse_type_args(&mut self) -> ParserResult<Vec<TypeExpr>> {
        let mut args = Vec::new();
        while !self.check(&TokenKind::Gt) && !self.check(&TokenKind::Shr) {
            args.push(self.parse_type()?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect_gt()?;
        Ok(args)
    }

    fn parse_param(&mut self, self_type: Option<&str>, first: bool) -> ParserResult<Param> {
        if first {
            if let Some(t) = self_type {
                let span = self.peek().span;
                let (by_ref, mutable) = match (self.peek_at(0), self.peek_at(1), self.peek_at(2)) {
                    (TokenKind::Amp, TokenKind::Mut, TokenKind::Ident(n)) if n == "self" => {
                        (true, true)
                    }
                    (TokenKind::Amp, TokenKind::Ident(n), _) if n == "self" => (true, false),
                    (TokenKind::Ident(n), next, _) if n == "self" && *next != TokenKind::Colon => {
                        (false, false)
                    }
                    _ => (false, false),
                };
                let is_receiver = by_ref
                    || matches!((self.peek_at(0), self.peek_at(1)), (TokenKind::Ident(n), next) if n == "self" && *next != TokenKind::Colon);
                if is_receiver {
                    if by_ref {
                        self.advance();
                        if mutable {
                            self.advance();
                        }
                    }
                    self.advance();
                    let _ = t;
                    let named = TypeExpr {
                        kind: TypeExprKind::Named("Self".to_string()),
                        span,
                    };
                    let r#type = if by_ref {
                        TypeExpr {
                            kind: TypeExprKind::Ref {
                                inner: Box::new(named),
                                mutable,
                            },
                            span,
                        }
                    } else {
                        named
                    };
                    return Ok(Param {
                        name: "self".into(),
                        mutable: false,
                        r#type,
                        span,
                    });
                }
            }
        }
        let mutable = self.eat(&TokenKind::Mut);
        let (name, span) = self.expect_ident()?;
        self.expect(TokenKind::Colon, "':'")?;
        let r#type = self.parse_type()?;
        Ok(Param {
            name,
            mutable,
            r#type,
            span,
        })
    }

    fn parse_type(&mut self) -> ParserResult<TypeExpr> {
        let span = self.peek().span;
        let kind = match self.peek().kind.clone() {
            TokenKind::Star => {
                self.advance();
                let mutable = if self.eat(&TokenKind::Mut) {
                    true
                } else if self.eat(&TokenKind::Const) {
                    false
                } else {
                    return Err(self.error_here(
                        "raw pointer type needs 'const' or 'mut' (`*const T` / `*mut T`)",
                    ));
                };
                TypeExprKind::Raw {
                    inner: Box::new(self.parse_type()?),
                    mutable,
                }
            }
            TokenKind::Amp => {
                self.advance();
                let mutable = self.eat(&TokenKind::Mut);
                if self.check(&TokenKind::LBracket) {
                    let save = self.pos;
                    self.advance();
                    let elem = self.parse_type()?;
                    if self.eat(&TokenKind::RBracket) {
                        return Ok(TypeExpr {
                            kind: TypeExprKind::Slice {
                                elem: Box::new(elem),
                                mutable,
                            },
                            span,
                        });
                    }
                    self.pos = save;
                }
                TypeExprKind::Ref {
                    inner: Box::new(self.parse_type()?),
                    mutable,
                }
            }
            TokenKind::LBracket => {
                self.advance();
                let elem = self.parse_type()?;
                self.expect(TokenKind::Semicolon, "';' in array type")?;
                let count = self.with_no_struct(false, |p| p.parse_expr())?;
                self.expect(TokenKind::RBracket, "']'")?;
                TypeExprKind::Array(Box::new(elem), Box::new(count))
            }
            TokenKind::LParen => {
                self.advance();
                self.expect(TokenKind::RParen, "')' (unit type)")?;
                TypeExprKind::Unit
            }
            TokenKind::Fn | TokenKind::Extern => {
                let efi = self.eat(&TokenKind::Extern);
                self.expect(TokenKind::Fn, "'fn'")?;
                self.expect(TokenKind::LParen, "'('")?;
                let mut params = Vec::new();
                while !self.check(&TokenKind::RParen) {
                    params.push(self.parse_type()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(TokenKind::RParen, "')'")?;
                let ret = if self.eat(&TokenKind::Arrow) {
                    Some(Box::new(self.parse_type()?))
                } else {
                    None
                };
                TypeExprKind::Fn { params, ret, efi }
            }
            TokenKind::Ident(first) => {
                self.advance();
                let mut name = first;
                while *self.peek_at(0) == TokenKind::ColonColon
                    && matches!(self.peek_at(1), TokenKind::Ident(_))
                {
                    self.advance();
                    name = format!("{name}::{}", self.expect_ident()?.0);
                }
                let primitive = matches!(
                    name.as_str(),
                    "u8" | "u16"
                        | "u32"
                        | "u64"
                        | "usize"
                        | "i8"
                        | "i16"
                        | "i32"
                        | "i64"
                        | "isize"
                        | "bool"
                        | "Bool"
                        | "Int"
                        | "Str"
                );
                if !primitive && self.eat(&TokenKind::Lt) {
                    TypeExprKind::Generic(name, self.parse_type_args()?)
                } else {
                    TypeExprKind::Named(name)
                }
            }
            other => return Err(self.error_here(format!("type expected, found {other:?}"))),
        };
        Ok(TypeExpr { kind, span })
    }

    fn parse_struct(&mut self, attrs: Vec<Attr>) -> ParserResult<StructDef> {
        let span = self.advance().span;
        let (name, _) = self.expect_ident()?;
        let generics = self.parse_generic_params()?;
        self.expect(TokenKind::LBrace, "'{'")?;
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            let (fname, fspan) = self.expect_ident()?;
            self.expect(TokenKind::Colon, "':'")?;
            let r#type = self.parse_type()?;
            fields.push(FieldDef {
                name: fname,
                r#type,
                span: fspan,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RBrace, "'}'")?;
        Ok(StructDef {
            name,
            module: self.module.clone(),
            generics,
            fields,
            attrs,
            span,
        })
    }

    fn parse_enum(&mut self) -> ParserResult<EnumDef> {
        let span = self.advance().span;
        let (name, _) = self.expect_ident()?;
        let generics = self.parse_generic_params()?;
        let repr = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(TokenKind::LBrace, "'{'")?;
        let mut variants = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            let (vname, vspan) = self.expect_ident()?;
            let fields = if self.eat(&TokenKind::LParen) {
                let mut types = Vec::new();
                while !self.check(&TokenKind::RParen) {
                    types.push(self.parse_type()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(TokenKind::RParen, "')'")?;
                if types.is_empty() {
                    return Err(ParserError {
                        message: format!(
                            "variant '{vname}()' needs at least one type (write '{vname}' for no data)"
                        ),
                        span: vspan,
                    });
                }
                Some(types)
            } else {
                None
            };
            let value = if self.eat(&TokenKind::Eq) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            variants.push(Variant {
                name: vname,
                value,
                fields,
                span: vspan,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RBrace, "'}'")?;
        Ok(EnumDef {
            name,
            module: self.module.clone(),
            generics,
            repr,
            variants,
            span,
        })
    }

    fn parse_trait(&mut self) -> ParserResult<TraitDef> {
        let span = self.advance().span;
        let (name, _) = self.expect_ident()?;
        self.expect(TokenKind::LBrace, "'{'")?;
        let mut methods = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let attrs = self.parse_attrs()?;
            self.trait_sig = true;
            let f = self.parse_function(attrs, Some("Self"));
            self.trait_sig = false;
            let f = f?;
            let has_body =
                !f.body.stmts.is_empty() || self.tokens[self.pos - 1].kind == TokenKind::RBrace;
            methods.push((f, has_body));
        }
        self.expect(TokenKind::RBrace, "'}'")?;
        Ok(TraitDef {
            name,
            module: self.module.clone(),
            methods,
            span,
        })
    }

    fn parse_impl(&mut self, program: &mut Program) -> ParserResult<()> {
        let span = self.advance().span;
        let generics = self.parse_generic_params()?;
        let first = self.parse_type()?;
        let (trait_name, self_ty) = if self.eat(&TokenKind::For) {
            let TypeExprKind::Named(t) = &first.kind else {
                return Err(ParserError {
                    message: "trait name expected before 'for'".into(),
                    span: first.span,
                });
            };
            (Some(t.clone()), self.parse_type()?)
        } else {
            (None, first)
        };
        self.expect(TokenKind::LBrace, "'{'")?;
        let mut methods = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let attrs = self.parse_attrs()?;
            methods.push(self.parse_function(attrs, Some("Self"))?);
        }
        self.expect(TokenKind::RBrace, "'}'")?;
        match (&self_ty.kind, generics.is_empty(), &trait_name) {
            (TypeExprKind::Named(name), true, None) => {
                for mut m in methods {
                    m.name = format!("{name}::{}", m.name);
                    program.functions.push(m);
                }
            }
            _ => program.impls.push(ImplDef {
                module: self.module.clone(),
                generics,
                self_ty,
                trait_name,
                methods,
                span,
            }),
        }
        Ok(())
    }

    fn parse_static(&mut self, attrs: Vec<Attr>) -> ParserResult<StaticDef> {
        let span = self.advance().span;
        let mutable = self.eat(&TokenKind::Mut);
        let (name, _) = self.expect_ident()?;
        self.expect(TokenKind::Colon, "':' (statics need a type)")?;
        let r#type = self.parse_type()?;
        let init = if self.eat(&TokenKind::Eq) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(StaticDef {
            name,
            module: self.module.clone(),
            mutable,
            r#type,
            init,
            attrs,
            span,
        })
    }

    fn static_accessors(&self, s: &StaticDef) -> Vec<Function> {
        let mut out = Vec::new();
        let span = s.span;
        let var = |name: &str| Expr::new(ExprKind::Var(name.to_string()), span);
        if has_attr(&s.attrs, "getter") {
            out.push(Function {
                name: format!("get_{}", s.name),
                module: s.module.clone(),
                generics: Vec::new(),
                params: Vec::new(),
                return_type: Some(s.r#type.clone()),
                body: Block {
                    stmts: vec![Stmt::Return {
                        value: Some(var(&s.name)),
                        span,
                    }],
                    span,
                },
                attrs: Vec::new(),
                trusted: self.trusted,
                span,
            });
        }
        if has_attr(&s.attrs, "setter") {
            const PARAM: &str = "value$";
            out.push(Function {
                name: format!("set_{}", s.name),
                module: s.module.clone(),
                generics: Vec::new(),
                params: vec![Param {
                    name: PARAM.into(),
                    mutable: false,
                    r#type: s.r#type.clone(),
                    span,
                }],
                return_type: None,
                body: Block {
                    stmts: vec![Stmt::Assign {
                        target: var(&s.name),
                        op: None,
                        value: var(PARAM),
                        span,
                    }],
                    span,
                },
                attrs: Vec::new(),
                trusted: self.trusted,
                span,
            });
        }
        out
    }

    fn parse_const(&mut self) -> ParserResult<ConstDef> {
        let span = self.advance().span;
        let (name, _) = self.expect_ident()?;
        let r#type = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(TokenKind::Eq, "'='")?;
        let value = self.parse_expr()?;
        Ok(ConstDef {
            name,
            module: self.module.clone(),
            r#type,
            value,
            span,
        })
    }

    fn parse_block(&mut self) -> ParserResult<Block> {
        let span = self.expect(TokenKind::LBrace, "'{'")?.span;
        self.with_no_struct(false, |p| {
            let mut stmts = Vec::new();
            while !p.check(&TokenKind::RBrace) && !p.check(&TokenKind::Eof) {
                stmts.push(p.parse_stmt()?);
                p.eat(&TokenKind::Semicolon);
            }
            p.expect(TokenKind::RBrace, "'}'")?;
            Ok(Block { stmts, span })
        })
    }

    fn parse_stmt(&mut self) -> ParserResult<Stmt> {
        let span = self.peek().span;
        match self.peek().kind {
            TokenKind::Let => self.parse_let(),
            TokenKind::Return => {
                self.advance();
                let value = if matches!(
                    self.peek().kind,
                    TokenKind::Semicolon | TokenKind::RBrace | TokenKind::Comma
                ) {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                Ok(Stmt::Return { value, span })
            }
            TokenKind::While => {
                self.advance();
                if self.eat(&TokenKind::Let) {
                    let pat = self.parse_or_pattern()?;
                    self.expect(TokenKind::Eq, "'=' after the pattern")?;
                    let value = self.with_no_struct(true, |p| p.parse_expr())?;
                    let body = self.parse_block()?;
                    return Ok(Stmt::WhileLet {
                        pat,
                        value,
                        body,
                        span,
                    });
                }
                let cond = self.with_no_struct(true, |p| p.parse_expr())?;
                let body = self.parse_block()?;
                Ok(Stmt::While { cond, body, span })
            }
            TokenKind::Loop => {
                self.advance();
                let body = self.parse_block()?;
                Ok(Stmt::Loop { body, span })
            }
            TokenKind::For => {
                self.advance();
                let (var, _) = self.expect_ident()?;
                self.expect(TokenKind::In, "'in'")?;
                let (start, end) = self.with_no_struct(true, |p| {
                    let start = p.parse_expr()?;
                    p.expect(TokenKind::DotDot, "'..'")?;
                    let end = p.parse_expr()?;
                    Ok((start, end))
                })?;
                let body = self.parse_block()?;
                Ok(Stmt::For {
                    var,
                    start,
                    end,
                    body,
                    span,
                })
            }
            TokenKind::Break => {
                self.advance();
                Ok(Stmt::Break(span))
            }
            TokenKind::Continue => {
                self.advance();
                Ok(Stmt::Continue(span))
            }
            TokenKind::If => self.parse_if(),
            TokenKind::Match => self.parse_match(),
            TokenKind::LBrace => Ok(Stmt::Block(self.parse_block()?)),
            _ => {
                let expr = self.parse_expr()?;
                let op = match self.peek().kind {
                    TokenKind::Eq => Some(None),
                    TokenKind::PlusEq => Some(Some(BinOp::Add)),
                    TokenKind::MinusEq => Some(Some(BinOp::Sub)),
                    TokenKind::StarEq => Some(Some(BinOp::Mul)),
                    TokenKind::SlashEq => Some(Some(BinOp::Div)),
                    TokenKind::PercentEq => Some(Some(BinOp::Mod)),
                    TokenKind::AmpEq => Some(Some(BinOp::BitAnd)),
                    TokenKind::PipeEq => Some(Some(BinOp::BitOr)),
                    TokenKind::CaretEq => Some(Some(BinOp::BitXor)),
                    TokenKind::ShlEq => Some(Some(BinOp::Shl)),
                    TokenKind::ShrEq => Some(Some(BinOp::Shr)),
                    _ => None,
                };
                match op {
                    Some(op) => {
                        self.advance();
                        let value = self.parse_expr()?;
                        Ok(Stmt::Assign {
                            target: expr,
                            op,
                            value,
                            span,
                        })
                    }
                    None => Ok(Stmt::Expr(expr)),
                }
            }
        }
    }

    fn parse_let(&mut self) -> ParserResult<Stmt> {
        let span = self.advance().span;
        let is_pattern = matches!(self.peek().kind, TokenKind::Ident(_))
            && matches!(self.peek_at(1), TokenKind::LParen | TokenKind::ColonColon);
        if is_pattern {
            let pat = self.parse_or_pattern()?;
            self.expect(TokenKind::Eq, "'=' after the pattern")?;
            let value = self.parse_expr()?;
            self.expect(TokenKind::Else, "'else' after 'let PATTERN = VALUE'")?;
            let else_block = self.parse_block()?;
            return Ok(Stmt::LetElse {
                pat,
                value,
                else_block,
                span,
            });
        }
        let mutable = self.eat(&TokenKind::Mut);
        let (name, _) = self.expect_ident()?;
        let expr_type = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        let value = if self.eat(&TokenKind::Eq) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        if expr_type.is_none() && value.is_none() {
            return Err(ParserError {
                message: format!("'let {name}' needs a type or a value"),
                span,
            });
        }
        Ok(Stmt::Let {
            name,
            mutable,
            expr_type,
            value,
            span,
            r#type: None,
        })
    }

    fn parse_if(&mut self) -> ParserResult<Stmt> {
        let span = self.advance().span;
        let let_pat = if self.eat(&TokenKind::Let) {
            let pat = self.parse_or_pattern()?;
            self.expect(TokenKind::Eq, "'=' after the pattern")?;
            Some(pat)
        } else {
            None
        };
        let cond = self.with_no_struct(true, |p| p.parse_expr())?;
        let then_block = self.parse_block()?;

        let else_block = if self.eat(&TokenKind::Else) {
            if self.check(&TokenKind::If) {
                let else_span = self.peek().span;
                let nested = self.parse_if()?;
                Some(Block {
                    stmts: vec![nested],
                    span: else_span,
                })
            } else {
                Some(self.parse_block()?)
            }
        } else {
            None
        };

        if let Some(pat) = let_pat {
            return Ok(Stmt::IfLet {
                pat,
                value: cond,
                then_block,
                else_block,
                span,
            });
        }

        Ok(Stmt::If {
            cond,
            then_block,
            else_block,
            span,
        })
    }

    fn parse_pattern(&mut self) -> ParserResult<Pattern> {
        let span = self.peek().span;
        match self.peek().kind.clone() {
            TokenKind::Ident(n) if n == "_" => {
                self.advance();
                return Ok(Pattern::new(PatKind::Wild, span));
            }
            TokenKind::Mut => {
                self.advance();
                let (name, _) = self.expect_ident()?;
                return Ok(Pattern::new(PatKind::MutBind(name), span));
            }
            TokenKind::Ident(_) => {
                let mut k = 1;
                while *self.peek_at(k) == TokenKind::ColonColon
                    && matches!(self.peek_at(k + 1), TokenKind::Ident(_))
                {
                    k += 2;
                }
                if *self.peek_at(k) == TokenKind::LParen {
                    let mut segs = Vec::new();
                    segs.push(self.expect_ident()?.0);
                    while self.eat(&TokenKind::ColonColon) {
                        segs.push(self.expect_ident()?.0);
                    }
                    self.advance();
                    let args = self.parse_pattern_list()?;
                    let name = segs.pop().unwrap();
                    let owner = if segs.is_empty() {
                        None
                    } else {
                        Some(segs.join("::"))
                    };
                    return Ok(Pattern::new(PatKind::Variant { owner, name, args }, span));
                }
            }
            _ => {}
        }
        let e = self.with_no_struct(true, |p| p.parse_expr_bp(9))?;
        let kind = match e.kind {
            ExprKind::Var(n) => PatKind::Ident(n),
            ExprKind::Path(o, m) => PatKind::Path(o, m),
            _ => PatKind::Lit(Box::new(e)),
        };
        Ok(Pattern::new(kind, span))
    }

    fn parse_or_pattern(&mut self) -> ParserResult<Pattern> {
        let span = self.peek().span;
        let mut alts = vec![self.parse_pattern()?];
        while self.eat(&TokenKind::Pipe) {
            alts.push(self.parse_pattern()?);
        }
        Ok(if alts.len() == 1 {
            alts.pop().unwrap()
        } else {
            Pattern::new(PatKind::Or(alts), span)
        })
    }

    fn parse_pattern_list(&mut self) -> ParserResult<Vec<Pattern>> {
        let mut pats = Vec::new();
        while !self.check(&TokenKind::RParen) {
            pats.push(self.parse_or_pattern()?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RParen, "')'")?;
        Ok(pats)
    }

    fn parse_match(&mut self) -> ParserResult<Stmt> {
        let span = self.advance().span;
        let scrutinee = self.with_no_struct(true, |p| p.parse_expr())?;
        self.expect(TokenKind::LBrace, "'{'")?;
        let mut arms = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let arm_span = self.peek().span;
            let mut pats = Vec::new();
            loop {
                pats.push(self.parse_pattern()?);
                if !self.eat(&TokenKind::Pipe) {
                    break;
                }
            }
            let guard = if self.eat(&TokenKind::If) {
                Some(self.with_no_struct(true, |p| p.parse_expr())?)
            } else {
                None
            };
            self.expect(TokenKind::FatArrow, "'=>'")?;
            let body = if self.check(&TokenKind::LBrace) {
                self.parse_block()?
            } else {
                let stmt_span = self.peek().span;
                let stmt = self.with_no_struct(false, |p| p.parse_stmt())?;
                Block {
                    stmts: vec![stmt],
                    span: stmt_span,
                }
            };
            self.eat(&TokenKind::Comma);
            arms.push(MatchArm {
                pats,
                guard,
                body,
                span: arm_span,
            });
        }
        self.expect(TokenKind::RBrace, "'}'")?;
        Ok(Stmt::Match {
            scrutinee,
            arms,
            span,
        })
    }

    pub fn parse_expr(&mut self) -> ParserResult<Expr> {
        self.parse_expr_bp(0)
    }

    fn parse_expr_bp(&mut self, min_bp: u8) -> ParserResult<Expr> {
        let mut lhs = self.parse_unary()?;

        loop {
            if self.check(&TokenKind::As) {
                // NOTE: important here 19u8 is the max of binding_power
                if 19u8 < min_bp {
                    break;
                }
                self.advance();
                let r#type = self.parse_type()?;
                let span = lhs.span;
                lhs = Expr::new(
                    ExprKind::Cast {
                        expr: Box::new(lhs),
                        r#type,
                    },
                    span,
                );
                continue;
            }

            let Some((op, l_bp, r_bp)) = infix_binding_power(&self.peek().kind) else {
                break;
            };
            if !self.continues_expr() {
                break;
            }
            if l_bp < min_bp {
                break;
            }

            self.advance();
            let rhs = self.parse_expr_bp(r_bp)?;
            let span = lhs.span;
            lhs = Expr::new(
                ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            );
        }

        Ok(lhs)
    }

    fn parse_unary(&mut self) -> ParserResult<Expr> {
        let span = self.peek().span;
        let op = match &self.peek().kind {
            TokenKind::Minus => UnaryOp::Neg,
            TokenKind::Bang => UnaryOp::Not,
            TokenKind::Tilde => UnaryOp::BitNot,
            TokenKind::Star => UnaryOp::Deref,
            TokenKind::Amp => {
                self.advance();
                let mutable = self.eat(&TokenKind::Mut);
                let operand = self.parse_unary()?;
                return Ok(Expr::new(
                    ExprKind::Unary {
                        op: UnaryOp::Ref(mutable),
                        operand: Box::new(operand),
                    },
                    span,
                ));
            }
            TokenKind::AndAnd => {
                self.advance();
                let mutable = self.eat(&TokenKind::Mut);
                let inner = self.parse_unary()?;
                let once = Expr::new(
                    ExprKind::Unary {
                        op: UnaryOp::Ref(mutable),
                        operand: Box::new(inner),
                    },
                    span,
                );
                return Ok(Expr::new(
                    ExprKind::Unary {
                        op: UnaryOp::Ref(false),
                        operand: Box::new(once),
                    },
                    span,
                ));
            }
            _ => return self.parse_postfix(),
        };

        self.advance();
        let operand = self.parse_unary()?;

        if op == UnaryOp::Neg {
            match operand.kind {
                ExprKind::Int(n) => return Ok(Expr::new(ExprKind::Int(-n), span)),
                ExprKind::TypedInt(n, t) => return Ok(Expr::new(ExprKind::TypedInt(-n, t), span)),
                _ => {}
            }
        }

        Ok(Expr::new(
            ExprKind::Unary {
                op,
                operand: Box::new(operand),
            },
            span,
        ))
    }

    fn parse_postfix(&mut self) -> ParserResult<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            let span = expr.span;
            if !self.continues_expr() {
                return Ok(expr);
            }
            match self.peek().kind {
                TokenKind::Question => {
                    self.advance();
                    expr = Expr::new(ExprKind::Try(Box::new(expr)), span);
                }
                TokenKind::LParen => {
                    self.advance();
                    let args = self.with_no_struct(false, |p| p.parse_args(TokenKind::RParen))?;
                    expr = Expr::new(
                        ExprKind::Call {
                            callee: Box::new(expr),
                            args,
                            target: CallTarget::Unresolved,
                        },
                        span,
                    );
                }
                TokenKind::Dot => {
                    self.advance();
                    let (name, _) = self.expect_ident()?;
                    expr = Expr::new(
                        ExprKind::Field {
                            base: Box::new(expr),
                            name,
                        },
                        span,
                    );
                }
                TokenKind::LBracket => {
                    self.advance();
                    expr = self.with_no_struct(false, |p| {
                        let start = if p.check(&TokenKind::DotDot) {
                            None
                        } else {
                            Some(Box::new(p.parse_expr()?))
                        };
                        if p.eat(&TokenKind::DotDot) {
                            let end = if p.check(&TokenKind::RBracket) {
                                None
                            } else {
                                Some(Box::new(p.parse_expr()?))
                            };
                            p.expect(TokenKind::RBracket, "']'")?;
                            return Ok(Expr::new(
                                ExprKind::Range {
                                    base: Box::new(expr),
                                    start,
                                    end,
                                },
                                span,
                            ));
                        }
                        p.expect(TokenKind::RBracket, "']'")?;
                        Ok(Expr::new(
                            ExprKind::Index {
                                base: Box::new(expr),
                                index: start.unwrap(),
                            },
                            span,
                        ))
                    })?;
                }
                _ => return Ok(expr),
            }
        }
    }

    fn looks_like_struct_lit(&self) -> bool {
        !self.no_struct
            && self.check(&TokenKind::LBrace)
            && match self.peek_at(1) {
                TokenKind::RBrace => true,
                TokenKind::Ident(_) => *self.peek_at(2) == TokenKind::Colon,
                _ => false,
            }
    }

    fn parse_primary(&mut self) -> ParserResult<Expr> {
        let token = self.advance();
        let span = token.span;
        let kind = match token.kind {
            TokenKind::Int(n) => ExprKind::Int(n as i128),
            TokenKind::IntSuffix(n, suffix) => {
                use IntType::*;
                let t = match suffix.as_str() {
                    "u8" => U8,
                    "u16" => U16,
                    "u32" => U32,
                    "u64" => U64,
                    "usize" => Usize,
                    "i8" => I8,
                    "i16" => I16,
                    "i32" => I32,
                    "i64" => I64,
                    _ => Isize,
                };
                ExprKind::TypedInt(n as i128, t)
            }
            TokenKind::Str(s) => ExprKind::Str(s),
            TokenKind::CStr(s) => ExprKind::CStr(s),
            TokenKind::WStr(s) => ExprKind::WStr(s),
            TokenKind::True => ExprKind::Bool(true),
            TokenKind::False => ExprKind::Bool(false),
            TokenKind::Unsafe => {
                return Err(ParserError {
                            message: "'unsafe' does not exist in this language: put low-level code in a file that starts with #![trusted]".into(),
                            span,
                        });
            }
            TokenKind::Sizeof => {
                self.expect(TokenKind::LParen, "'('")?;
                let r#type = self.parse_type()?;
                self.expect(TokenKind::RParen, "')'")?;
                ExprKind::Sizeof(r#type)
            }
            TokenKind::Ident(name) => {
                let mut segs: Vec<(String, Vec<TypeExpr>)> = vec![(name, Vec::new())];
                while self.eat(&TokenKind::ColonColon) {
                    if self.eat(&TokenKind::Lt) {
                        let args = self.parse_type_args()?;
                        let last = segs.last_mut().unwrap();
                        if !last.1.is_empty() {
                            return Err(self.error_here("type arguments given twice"));
                        }
                        last.1 = args;
                    } else {
                        segs.push((self.expect_ident()?.0, Vec::new()));
                    }
                }
                let n = segs.len();
                if self.looks_like_struct_lit() {
                    if segs[..n - 1].iter().any(|s| !s.1.is_empty()) {
                        return Err(ParserError {
                            message: "type arguments go on the last name of a struct literal"
                                .into(),
                            span,
                        });
                    }
                    let args = take(&mut segs[n - 1].1);
                    let name = segs
                        .iter()
                        .map(|s| s.0.as_str())
                        .collect::<Vec<_>>()
                        .join("::");
                    self.parse_struct_lit(name, args)?
                } else if n == 1 {
                    let (name, args) = segs.pop().unwrap();
                    if args.is_empty() {
                        ExprKind::Var(name)
                    } else {
                        ExprKind::Turbofish {
                            owner: name,
                            owner_args: args,
                            member: None,
                            member_args: Vec::new(),
                        }
                    }
                } else {
                    if segs[..n - 2].iter().any(|s| !s.1.is_empty()) {
                        return Err(ParserError {
                            message: "type arguments go on the type or on the function of a path"
                                .into(),
                            span,
                        });
                    }
                    let (member, member_args) = segs.pop().unwrap();
                    let (last_owner, owner_args) = segs.pop().unwrap();
                    let mut owner_parts: Vec<String> = segs.into_iter().map(|s| s.0).collect();
                    owner_parts.push(last_owner);
                    let owner = owner_parts.join("::");
                    if owner_args.is_empty() && member_args.is_empty() {
                        ExprKind::Path(owner, member)
                    } else {
                        ExprKind::Turbofish {
                            owner,
                            owner_args,
                            member: Some(member),
                            member_args,
                        }
                    }
                }
            }
            TokenKind::LParen => {
                let inner = self.with_no_struct(false, |p| p.parse_expr())?;
                self.expect(TokenKind::RParen, "')'")?;
                return Ok(inner);
            }
            TokenKind::LBracket => self.with_no_struct(false, |p| {
                if p.eat(&TokenKind::RBracket) {
                    return Ok(ExprKind::ArrayLit(Vec::new()));
                }
                let first = p.parse_expr()?;
                if p.eat(&TokenKind::Semicolon) {
                    let count = p.parse_expr()?;
                    p.expect(TokenKind::RBracket, "']'")?;
                    return Ok(ExprKind::ArrayRepeat {
                        value: Box::new(first),
                        count: Box::new(count),
                    });
                }
                let mut items = vec![first];
                while p.eat(&TokenKind::Comma) {
                    if p.check(&TokenKind::RBracket) {
                        break;
                    }
                    items.push(p.parse_expr()?);
                }
                p.expect(TokenKind::RBracket, "']'")?;
                Ok(ExprKind::ArrayLit(items))
            })?,
            other => {
                return Err(ParserError {
                    message: format!("expected expression, found {other:?}"),
                    span,
                });
            }
        };
        Ok(Expr::new(kind, span))
    }

    fn parse_struct_lit(&mut self, name: String, args: Vec<TypeExpr>) -> ParserResult<ExprKind> {
        self.expect(TokenKind::LBrace, "'{'")?;
        self.with_no_struct(false, |p| {
            let mut fields = Vec::new();
            while !p.check(&TokenKind::RBrace) {
                let (fname, fspan) = p.expect_ident()?;
                p.expect(TokenKind::Colon, "':'")?;
                let value = p.parse_expr()?;
                fields.push((fname, value, fspan));
                if !p.eat(&TokenKind::Comma) {
                    break;
                }
            }
            p.expect(TokenKind::RBrace, "'}'")?;
            Ok(ExprKind::StructLit { name, args, fields })
        })
    }

    fn parse_args(&mut self, close: TokenKind) -> ParserResult<Vec<Expr>> {
        let mut args = Vec::new();
        while !self.check(&close) {
            args.push(self.parse_expr()?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(close, "')'")?;
        Ok(args)
    }
}

fn infix_binding_power(kind: &TokenKind) -> Option<(BinOp, u8, u8)> {
    let result = match kind {
        TokenKind::OrOr => (BinOp::Or, 1, 2),
        TokenKind::AndAnd => (BinOp::And, 3, 4),

        TokenKind::EqEq => (BinOp::Eq, 5, 6),
        TokenKind::Ne => (BinOp::Ne, 5, 6),
        TokenKind::Lt => (BinOp::Lt, 5, 6),
        TokenKind::Le => (BinOp::Le, 5, 6),
        TokenKind::Gt => (BinOp::Gt, 5, 6),
        TokenKind::Ge => (BinOp::Ge, 5, 6),

        TokenKind::Pipe => (BinOp::BitOr, 7, 8),
        TokenKind::Caret => (BinOp::BitXor, 9, 10),
        TokenKind::Amp => (BinOp::BitAnd, 11, 12),
        TokenKind::Shl => (BinOp::Shl, 13, 14),
        TokenKind::Shr => (BinOp::Shr, 13, 14),

        TokenKind::Plus => (BinOp::Add, 15, 16),
        TokenKind::Minus => (BinOp::Sub, 15, 16),
        TokenKind::PlusPercent => (BinOp::AddWrap, 15, 16),
        TokenKind::MinusPercent => (BinOp::SubWrap, 15, 16),

        TokenKind::Star => (BinOp::Mul, 17, 18),
        TokenKind::Slash => (BinOp::Div, 17, 18),
        TokenKind::Modulo => (BinOp::Mod, 17, 18),
        TokenKind::StarPercent => (BinOp::MulWrap, 17, 18),

        _ => return None,
    };
    Some(result)
}
