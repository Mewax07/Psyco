#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Keywords
    Let,    // let ...
    Fn,     // fn ...
    If,     // if ...
    Else,   // else ...
    While,  // while ...
    Return, // return ...
    True,   // true
    False,  // false

    // Identifier
    Ident(String), // bob

    // Basic Type
    Str(String), // "alice"
    Int(i64),    // 15

    // Delimiter
    LParen,     // (
    RParen,     // )
    LBrace,     // {
    RBrace,     // }
    Colon,      // :
    Semicolon,  // ;
    Comma,      // ,
    Dot,        // .
    ColonColon, // ::
    DotDot,     // ..

    // Operator
    Plus,   // +
    Minus,  // -
    Star,   // *
    Slash,  // /
    Modulo, // %

    // Arrow
    Arrow,    // ->
    FatArrow, // =>

    // Comparator
    Lt,     // <
    Gt,     // >
    Eq,     // =
    Bang,   // !
    Le,     // <=
    Ge,     // >=
    EqEq,   // ==
    Ne,     // !=
    AndAnd, // &&
    OrOr,   // ||

    Eof, // Eof
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Debug)]
pub struct LexerError {
    pub message: String,
    pub span: Span,
}

type LexerResult<T> = Result<T, LexerError>;

pub struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
}

impl Lexer {
    pub fn new(src: &str) -> Self {
        Self {
            chars: src.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    pub fn tokenize(mut self) -> LexerResult<Vec<Token>> {
        let mut tokens = Vec::new();

        loop {
            let token = self.next_token()?;
            let is_eof = token.kind == TokenKind::Eof;
            tokens.push(token);
            if is_eof {
                break;
            }
        }

        Ok(tokens)
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_next(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn span(&self) -> Span {
        Span {
            line: self.line,
            col: self.col,
        }
    }

    fn error(&self, message: impl Into<String>, span: Span) -> LexerError {
        LexerError {
            message: message.into(),
            span,
        }
    }

    fn skip(&mut self) {
        loop {
            match (self.peek(), self.peek_next()) {
                (Some(c), _) if c.is_whitespace() => {
                    self.advance();
                }
                (Some('/'), Some('/')) => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.advance();
                    }
                }
                _ => break,
            }
        }
    }

    fn next_token(&mut self) -> LexerResult<Token> {
        self.skip();

        let span = self.span();
        let Some(c) = self.advance() else {
            return Ok(Token {
                kind: TokenKind::Eof,
                span,
            });
        };

        let kind = match c {
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '+' => TokenKind::Plus,
            '-' => {
                if self.peek() == Some('>') {
                    self.advance();
                    TokenKind::Arrow
                } else {
                    TokenKind::Minus
                }
            }
            '*' => TokenKind::Star,
            '/' => TokenKind::Slash,
            '%' => TokenKind::Modulo,
            '=' => {
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::EqEq
                } else if self.peek() == Some('>') {
                    self.advance();
                    TokenKind::FatArrow
                } else {
                    TokenKind::Eq
                }
            }
            '<' => {
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::Le
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::Ge
                } else {
                    TokenKind::Gt
                }
            }
            '!' => {
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::Ne
                } else {
                    TokenKind::Bang
                }
            }
            '&' => {
                if self.peek() == Some('&') {
                    self.advance();
                    TokenKind::AndAnd
                } else {
                    return Err(self.error(format!("'&' is unsupported use '&&'"), span));
                }
            }
            '|' => {
                if self.peek() == Some('|') {
                    self.advance();
                    TokenKind::OrOr
                } else {
                    return Err(self.error(format!("'|' is unsupported use '||'"), span));
                }
            }
            ':' => {
                if self.peek() == Some(':') {
                    self.advance();
                    TokenKind::ColonColon
                } else {
                    TokenKind::Colon
                }
            }
            ';' => TokenKind::Semicolon,
            ',' => TokenKind::Comma,
            '.' => {
                if self.peek() == Some('.') {
                    self.advance();
                    TokenKind::DotDot
                } else {
                    TokenKind::Dot
                }
            }
            '"' => self.read_string(span)?,
            c if c.is_ascii_digit() => self.read_number(span, c)?,
            c if c.is_alphabetic() || c == '_' => self.read_ident(c),
            _ => return Err(self.error(format!("Unexpected char: '{c}'"), span)),
        };

        Ok(Token { kind, span })
    }

    fn read_string(&mut self, span: Span) -> LexerResult<TokenKind> {
        let mut text = String::new();
        loop {
            match self.advance() {
                Some('"') => return Ok(TokenKind::Str(text)),
                Some(c) => text.push(c),
                None => return Err(self.error("Unfinished string", span)),
            }
        }
    }

    fn read_number(&mut self, span: Span, ch: char) -> LexerResult<TokenKind> {
        let mut digits = String::from(ch);
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                digits.push(c);
                self.advance();
            } else {
                break;
            }
        }
        digits
            .parse()
            .map(TokenKind::Int)
            .map_err(|_| self.error(format!("Number too big: {digits}"), span))
    }

    fn read_ident(&mut self, ch: char) -> TokenKind {
        let mut word = String::from(ch);
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                word.push(c);
                self.advance();
            } else {
                break;
            }
        }
        match word.as_str() {
            "let" => TokenKind::Let,
            "fn" => TokenKind::Fn,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "return" => TokenKind::Return,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            _ => TokenKind::Ident(word),
        }
    }
}
