use crate::{
    ast::{BinOp, Block, Expr, ExprKind, Function, Param, Program, Stmt, TypeExpr, UnaryOp},
    lexer::{
        Span, Token,
        TokenKind::{self},
    },
};

pub struct ParserError {
    pub message: String,
    pub span: Span,
}

type ParserResult<T> = Result<T, ParserError>;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    pub fn parse_program(&mut self) -> ParserResult<Program> {
        let mut functions = Vec::new();
        while !self.check(&TokenKind::Eof) {
            functions.push(self.parse_function()?);
        }
        Ok(Program { functions })
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens[self.pos].clone();
        if token.kind != TokenKind::Eof {
            self.pos += 1;
        }
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
            Err(self.error_here(format!("{what} expected, found: {:?}", self.peek().kind)))
        }
    }

    fn expect_ident(&mut self) -> ParserResult<(String, Span)> {
        let token = self.advance();
        match token.kind {
            TokenKind::Ident(name) => Ok((name, token.span)),
            other => Err(ParserError {
                message: format!("identify expected, found: {:?}", other),
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

    fn parse_function(&mut self) -> ParserResult<Function> {
        let span = self.expect(TokenKind::Fn, "'fn' keyword")?.span;
        let (name, _) = self.expect_ident()?;

        self.expect(TokenKind::LParen, "'('")?;
        let mut params = Vec::new();
        if !self.check(&TokenKind::RParen) {
            loop {
                params.push(self.parse_param()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(TokenKind::RParen, "')'")?;

        let return_type = if self.eat(&TokenKind::Arrow) {
            Some(self.parse_type()?)
        } else {
            None
        };

        let body = self.parse_block()?;
        Ok(Function {
            name,
            params,
            return_type,
            body,
            span,
        })
    }

    fn parse_param(&mut self) -> ParserResult<Param> {
        let (name, span) = self.expect_ident()?;
        self.expect(TokenKind::Colon, "':'")?;
        let r#type = self.parse_type()?;
        Ok(Param { name, r#type, span })
    }

    fn parse_type(&mut self) -> ParserResult<TypeExpr> {
        let (name, span) = self.expect_ident()?;
        Ok(TypeExpr { name, span })
    }

    fn parse_block(&mut self) -> ParserResult<Block> {
        let span = self.expect(TokenKind::LBrace, "'{'")?.span;
        let mut stmts = Vec::new();

        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            stmts.push(self.parse_stmt()?);
            self.eat(&TokenKind::Semicolon);
        }

        self.expect(TokenKind::RBrace, "'}'")?;
        Ok(Block { stmts, span })
    }

    fn parse_stmt(&mut self) -> ParserResult<Stmt> {
        if self.check(&TokenKind::Let) {
            self.parse_let()
        } else if self.check(&TokenKind::Return) {
            self.parse_return()
        } else if self.check(&TokenKind::While) {
            self.parse_while()
        } else if self.check(&TokenKind::If) {
            self.parse_if()
        } else {
            let expr = self.parse_expr()?;
            if self.check(&TokenKind::Eq) {
                let eq_span = self.advance().span;
                let ExprKind::Var(name) = expr.kind else {
                    return Err(ParserError {
                        message: "can only assign to a variable".into(),
                        span: eq_span,
                    });
                };
                let value = self.parse_expr()?;
                return Ok(Stmt::Assign {
                    name,
                    value,
                    span: expr.span,
                });
            }
            Ok(Stmt::Expr(expr))
        }
    }

    fn parse_let(&mut self) -> ParserResult<Stmt> {
        let span = self.advance().span;
        let (name, _) = self.expect_ident()?;
        self.expect(TokenKind::Eq, "'='")?;
        let value = self.parse_expr()?;
        Ok(Stmt::Let { name, value, span })
    }

    fn parse_return(&mut self) -> ParserResult<Stmt> {
        let span = self.advance().span;
        let value = if self.check(&TokenKind::Semicolon) || self.check(&TokenKind::RBrace) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        Ok(Stmt::Return { value, span })
    }

    fn parse_while(&mut self) -> ParserResult<Stmt> {
        let span = self.advance().span;
        let cond = self.parse_expr()?;
        let body = self.parse_block()?;
        Ok(Stmt::While { cond, body, span })
    }

    fn parse_if(&mut self) -> ParserResult<Stmt> {
        let span = self.advance().span;
        let cond = self.parse_expr()?;
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

    fn parse_expr(&mut self) -> ParserResult<Expr> {
        self.parse_expr_bp(0)
    }

    fn parse_expr_bp(&mut self, min_bp: u8) -> ParserResult<Expr> {
        let mut lhs = self.parse_unary()?;

        loop {
            let Some((op, l_bp, r_bp)) = infix_binding_power(&self.peek().kind) else {
                break;
            };
            if l_bp < min_bp {
                break;
            }

            self.advance();
            let rhs = self.parse_expr_bp(r_bp)?;

            let span = lhs.span;
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                r#type: None,
                span,
            };
        }

        Ok(lhs)
    }

    fn parse_unary(&mut self) -> ParserResult<Expr> {
        let op = match &self.peek().kind {
            TokenKind::Minus => Some(UnaryOp::Neg),
            TokenKind::Bang => Some(UnaryOp::Not),
            _ => None,
        };
        let Some(op) = op else {
            return self.parse_primary();
        };

        let span = self.advance().span;
        let operand = self.parse_unary()?;
        Ok(Expr {
            kind: ExprKind::Unary {
                op,
                operand: Box::new(operand),
            },
            r#type: None,
            span,
        })
    }

    fn parse_primary(&mut self) -> ParserResult<Expr> {
        let token = self.advance();
        let kind = match token.kind {
            TokenKind::Int(n) => ExprKind::Int(n),
            TokenKind::Str(n) => ExprKind::Str(n),
            TokenKind::True => ExprKind::Bool(true),
            TokenKind::False => ExprKind::Bool(false),
            TokenKind::Ident(name) => {
                if self.eat(&TokenKind::LParen) {
                    let args = self.parse_args()?;
                    ExprKind::Call { callee: name, args }
                } else {
                    ExprKind::Var(name)
                }
            }
            TokenKind::LParen => {
                let inner = self.parse_expr()?;
                self.expect(TokenKind::RParen, "')'")?;
                return Ok(inner);
            }
            other => {
                return Err(ParserError {
                    message: format!("Expected expression, found: {:?}", other),
                    span: token.span,
                });
            }
        };
        Ok(Expr {
            kind,
            r#type: None,
            span: token.span,
        })
    }

    fn parse_args(&mut self) -> ParserResult<Vec<Expr>> {
        let mut args = Vec::new();
        if !self.check(&TokenKind::RParen) {
            loop {
                args.push(self.parse_expr()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(TokenKind::RParen, "')'")?;
        Ok(args)
    }
}

fn infix_binding_power(kind: &TokenKind) -> Option<(BinOp, u8, u8)> {
    let result = match kind {
        TokenKind::OrOr => (BinOp::Or, 1, 2),
        TokenKind::AndAnd => (BinOp::And, 3, 4),

        TokenKind::EqEq => (BinOp::Eq, 5, 6),
        TokenKind::Ne => (BinOp::Ne, 5, 6),

        TokenKind::Lt => (BinOp::Lt, 7, 8),
        TokenKind::Le => (BinOp::Le, 7, 8),
        TokenKind::Gt => (BinOp::Gt, 7, 8),
        TokenKind::Ge => (BinOp::Ge, 7, 8),

        TokenKind::Plus => (BinOp::Add, 9, 10),
        TokenKind::Minus => (BinOp::Sub, 9, 10),

        TokenKind::Star => (BinOp::Mul, 11, 12),
        TokenKind::Slash => (BinOp::Div, 11, 12),

        TokenKind::Modulo => (BinOp::Mod, 11, 12),

        _ => return None,
    };
    Some(result)
}
