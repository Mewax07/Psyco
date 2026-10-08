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
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            no_struct: false,
            trusted: false,
            last_line: 0,
        }
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
        while !self.check(&TokenKind::Eof) {
            let attrs = self.parse_attrs()?;
            let span = self.peek().span;
            match &self.peek().kind {
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
                    self.parse_impl(&mut program)?;
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
                    program.imports.push((path, span));
                }
                TokenKind::Unsafe => {
                    return Err(self.error_here(
                        "'unsafe' does not exist in this language: put low-level code in a file that starts with #![trusted]",
                    ));
                }
                other => {
                    return Err(self.error_here(format!(
                        "expected 'fn', 'struct', 'enum', 'impl', 'static', 'const' or 'import', found {other:?}"
                    )));
                }
            }
            self.eat(&TokenKind::Semicolon);
        }
        Ok(program)
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

        let body = self.parse_block()?;
        let name = match self_type {
            Some(t) => format!("{t}::{name}"),
            None => name,
        };
        Ok(Function {
            name,
            params,
            return_type,
            body,
            attrs,
            trusted: self.trusted,
            span,
        })
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
                    let named = TypeExpr {
                        kind: TypeExprKind::Named(t.to_string()),
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
            TokenKind::Ident(name) => {
                self.advance();
                TypeExprKind::Named(name)
            }
            other => return Err(self.error_here(format!("type expected, found {other:?}"))),
        };
        Ok(TypeExpr { kind, span })
    }

    fn parse_struct(&mut self, attrs: Vec<Attr>) -> ParserResult<StructDef> {
        let span = self.advance().span;
        let (name, _) = self.expect_ident()?;
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
            fields,
            attrs,
            span,
        })
    }

    fn parse_enum(&mut self) -> ParserResult<EnumDef> {
        let span = self.advance().span;
        let (name, _) = self.expect_ident()?;
        let repr = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(TokenKind::LBrace, "'{'")?;
        let mut variants = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            let (vname, vspan) = self.expect_ident()?;
            let value = if self.eat(&TokenKind::Eq) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            variants.push(Variant {
                name: vname,
                value,
                span: vspan,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RBrace, "'}'")?;
        Ok(EnumDef {
            name,
            repr,
            variants,
            span,
        })
    }

    fn parse_impl(&mut self, program: &mut Program) -> ParserResult<()> {
        self.advance();
        let (name, _) = self.expect_ident()?;
        self.expect(TokenKind::LBrace, "'{'")?;
        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let attrs = self.parse_attrs()?;
            program
                .functions
                .push(self.parse_function(attrs, Some(&name))?);
        }
        self.expect(TokenKind::RBrace, "'}'")?;
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

        Ok(Stmt::If {
            cond,
            then_block,
            else_block,
            span,
        })
    }

    fn parse_match(&mut self) -> ParserResult<Stmt> {
        let span = self.advance().span;
        let scrutinee = self.with_no_struct(true, |p| p.parse_expr())?;
        self.expect(TokenKind::LBrace, "'{'")?;
        let mut arms = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let arm_span = self.peek().span;
            let patterns = if matches!(&self.peek().kind, TokenKind::Ident(n) if n == "_") {
                self.advance();
                None
            } else {
                let mut pats = Vec::new();
                loop {
                    pats.push(self.with_no_struct(true, |p| p.parse_expr_bp(9))?);
                    if !self.eat(&TokenKind::Pipe) {
                        break;
                    }
                }
                Some(pats)
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
                patterns,
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
                if self.eat(&TokenKind::ColonColon) {
                    let (member, _) = self.expect_ident()?;
                    ExprKind::Path(name, member)
                } else if self.looks_like_struct_lit() {
                    self.parse_struct_lit(name)?
                } else {
                    ExprKind::Var(name)
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

    fn parse_struct_lit(&mut self, name: String) -> ParserResult<ExprKind> {
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
            Ok(ExprKind::StructLit { name, fields })
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
