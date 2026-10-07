#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Keywords
    Let,      // let ...
    Fn,       // fn ...
    If,       // if ...
    Else,     // else ...
    While,    // while ...
    Loop,     // loop ...
    For,      // for ...
    In,       // in ...
    Break,    // break
    Continue, // continue
    Return,   // return ...
    True,     // true
    False,    // false
    Struct,   // struct ...
    Enum,     // enum ...
    Impl,     // impl ...
    Match,    // match ...
    As,       // as ...
    Static,   // static ...
    Const,    // const ...
    Extern,   // extern ...
    Import,   // import ...
    Sizeof,   // sizeof ...
    Mut,      // mut ...
    Unsafe,   // unsafe ... (I hope to remove that)

    // Identifier
    Ident(String), // bob

    // Literals
    Str(Vec<u8>),           // "alice" -> Str (length + octets)
    CStr(Vec<u8>),          // c"alice" -> *u8 ended with 0
    WStr(Vec<u16>),         // u"alice" -> *u16 ended with 0
    Int(u64),               // 15, 0xFF, 0b1010, 1_000, 'a'
    IntSuffix(u64, String), // 15u8, 0xFFusize

    // Delimiter
    LParen,     // (
    RParen,     // )
    LBrace,     // {
    RBrace,     // }
    LBracket,   // [
    RBracket,   // ]
    Colon,      // :
    Semicolon,  // ;
    Comma,      // ,
    Dot,        // .
    ColonColon, // ::
    DotDot,     // ..
    Hash,       // #

    // Operator
    Plus,         // +
    Minus,        // -
    Star,         // *
    Slash,        // /
    Modulo,       // %
    PlusPercent,  // +%
    MinusPercent, // -%
    StarPercent,  // *%

    // Bitwise
    Amp,   // &
    Pipe,  // |
    Caret, // ^
    Tilde, // ~
    Shl,   // <<
    Shr,   // >>

    // Arrows
    Arrow,    // ->
    FatArrow, // =>

    // Comparison / logic
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

    // Compound assignment
    PlusEq,    // +=
    MinusEq,   // -=
    StarEq,    // *=
    SlashEq,   // /=
    PercentEq, // %=
    AmpEq,     // &=
    PipeEq,    // |=
    CaretEq,   // ^=
    ShlEq,     // <<=
    ShrEq,     // >>=

    Eof, // Eof
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub line: usize,
    pub col: usize,
    // source file index
    pub file: usize,
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
    file: usize,
}

impl Lexer {
    pub fn new(src: &str) -> Self {
        Self::with_file(src, 0)
    }

    pub fn with_file(src: &str, file: usize) -> Self {
        Self {
            chars: src.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
            file,
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

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn span(&self) -> Span {
        Span {
            line: self.line,
            col: self.col,
            file: self.file,
        }
    }

    fn error(&self, message: impl Into<String>, span: Span) -> LexerError {
        LexerError {
            message: message.into(),
            span,
        }
    }

    fn skip(&mut self) -> LexerResult<()> {
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
                (Some('/'), Some('*')) => {
                    let span = self.span();
                    self.advance();
                    self.advance();
                    let mut depth = 1;
                    while depth > 0 {
                        match (self.advance(), self.peek()) {
                            (Some('*'), Some('/')) => {
                                self.advance();
                                depth -= 1;
                            }
                            (Some('/'), Some('*')) => {
                                self.advance();
                                depth += 1;
                            }
                            (None, _) => return Err(self.error("unterminated block comment", span)),
                            _ => {}
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn next_token(&mut self) -> LexerResult<Token> {
        self.skip()?;

        let span = self.span();
        let Some(c) = self.advance() else {
            return Ok(Token {
                kind: TokenKind::Eof,
                span,
            });
        };

        use TokenKind as T;
        let kind = match c {
            '(' => T::LParen,
            ')' => T::RParen,
            '{' => T::LBrace,
            '}' => T::RBrace,
            '[' => T::LBracket,
            ']' => T::RBracket,
            ';' => T::Semicolon,
            ',' => T::Comma,
            '#' => T::Hash,
            '~' => T::Tilde,
            '+' => {
                if self.eat('%') {
                    T::PlusPercent
                } else if self.eat('=') {
                    T::PlusEq
                } else {
                    T::Plus
                }
            }
            '-' => {
                if self.eat('>') {
                    T::Arrow
                } else if self.eat('%') {
                    T::MinusPercent
                } else if self.eat('=') {
                    T::MinusEq
                } else {
                    T::Minus
                }
            }
            '*' => {
                if self.eat('%') {
                    T::StarPercent
                } else if self.eat('=') {
                    T::StarEq
                } else {
                    T::Star
                }
            }
            '/' => {
                if self.eat('=') {
                    T::SlashEq
                } else {
                    T::Slash
                }
            }
            '%' => {
                if self.eat('=') {
                    T::PercentEq
                } else {
                    T::Modulo
                }
            }
            '^' => {
                if self.eat('=') {
                    T::CaretEq
                } else {
                    T::Caret
                }
            }
            '=' => {
                if self.eat('=') {
                    T::EqEq
                } else if self.eat('>') {
                    T::FatArrow
                } else {
                    T::Eq
                }
            }
            '<' => {
                if self.eat('<') {
                    if self.eat('=') { T::ShlEq } else { T::Shl }
                } else if self.eat('=') {
                    T::Le
                } else {
                    T::Lt
                }
            }
            '>' => {
                if self.eat('>') {
                    if self.eat('=') { T::ShrEq } else { T::Shr }
                } else if self.eat('=') {
                    T::Ge
                } else {
                    T::Gt
                }
            }
            '!' => {
                if self.eat('=') {
                    T::Ne
                } else {
                    T::Bang
                }
            }
            '&' => {
                if self.eat('&') {
                    T::AndAnd
                } else if self.eat('=') {
                    T::AmpEq
                } else {
                    T::Amp
                }
            }
            '|' => {
                if self.eat('|') {
                    T::OrOr
                } else if self.eat('=') {
                    T::PipeEq
                } else {
                    T::Pipe
                }
            }
            ':' => {
                if self.eat(':') {
                    T::ColonColon
                } else {
                    T::Colon
                }
            }
            '.' => {
                if self.eat('.') {
                    T::DotDot
                } else {
                    T::Dot
                }
            }
            '"' => T::Str(self.read_string_bytes(span)?),
            'c' if self.peek() == Some('"') => {
                self.advance();
                T::CStr(self.read_string_bytes(span)?)
            }
            'u' if self.peek() == Some('"') => {
                self.advance();
                let s = self.read_string_chars(span)?;
                T::WStr(s.encode_utf16().collect())
            }
            '\'' => self.read_char(span)?,
            c if c.is_ascii_digit() => self.read_number(span, c)?,
            c if c.is_alphabetic() || c == '_' => self.read_ident(c),
            _ => return Err(self.error(format!("unexpected character '{c}'"), span)),
        };

        Ok(Token { kind, span })
    }

    fn read_escape(&mut self, span: Span) -> LexerResult<char> {
        let Some(c) = self.advance() else {
            return Err(self.error("unfinished escape sequence", span));
        };
        Ok(match c {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            '0' => '\0',
            '\\' => '\\',
            '\'' => '\'',
            '"' => '"',
            'x' => {
                let mut v = 0u32;
                for _ in 0..2 {
                    let d = self
                        .advance()
                        .and_then(|d| d.to_digit(16))
                        .ok_or_else(|| self.error("\\x expects two hex digits", span))?;
                    v = v * 16 + d;
                }
                char::from_u32(v).unwrap()
            }
            other => return Err(self.error(format!("unknown escape '\\{other}'"), span)),
        })
    }

    fn read_string_chars(&mut self, span: Span) -> LexerResult<String> {
        let mut text = String::new();
        loop {
            match self.advance() {
                Some('"') => return Ok(text),
                Some('\\') => text.push(self.read_escape(span)?),
                Some(c) => text.push(c),
                None => return Err(self.error("unfinished string", span)),
            }
        }
    }

    fn read_string_bytes(&mut self, span: Span) -> LexerResult<Vec<u8>> {
        let mut bytes = Vec::new();
        loop {
            match self.advance() {
                Some('"') => return Ok(bytes),
                Some('\\') => {
                    let is_hex = self.peek() == Some('x');
                    let c = self.read_escape(span)?;
                    if is_hex {
                        bytes.push(c as u32 as u8);
                    } else {
                        let mut buf = [0; 4];
                        bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    }
                }
                Some(c) => {
                    let mut buf = [0; 4];
                    bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                None => return Err(self.error("unfinished string", span)),
            }
        }
    }

    fn read_char(&mut self, span: Span) -> LexerResult<TokenKind> {
        let c = match self.advance() {
            Some('\\') => self.read_escape(span)?,
            Some('\'') | None => return Err(self.error("empty character literal", span)),
            Some(c) => c,
        };
        if !self.eat('\'') {
            return Err(self.error("character literal must contain one character", span));
        }
        Ok(TokenKind::Int(c as u64))
    }

    fn read_number(&mut self, span: Span, first: char) -> LexerResult<TokenKind> {
        let mut radix = 10;
        let mut digits = String::new();
        if first == '0' && matches!(self.peek(), Some('x' | 'b' | 'o')) {
            radix = match self.advance() {
                Some('x') => 16,
                Some('b') => 2,
                _ => 8,
            };
        } else {
            digits.push(first);
        }
        let mut suffix = String::new();
        while let Some(c) = self.peek() {
            if c == '_' && suffix.is_empty() {
                self.advance();
            } else if c.is_digit(radix) && suffix.is_empty() {
                digits.push(c);
                self.advance();
            } else if (c == 'u' || c == 'i') && suffix.is_empty() {
                suffix.push(c);
                self.advance();
            } else if c.is_ascii_alphanumeric() && !suffix.is_empty() {
                suffix.push(c);
                self.advance();
            } else if c.is_ascii_alphanumeric() {
                return Err(self.error(format!("invalid digit '{c}' in number"), span));
            } else {
                break;
            }
        }
        if digits.is_empty() {
            return Err(self.error("number has no digits", span));
        }
        let value = u64::from_str_radix(&digits, radix)
            .map_err(|_| self.error(format!("number too big: {digits}"), span))?;
        if suffix.is_empty() {
            return Ok(TokenKind::Int(value));
        }
        const SUFFIXES: [&str; 10] = [
            "u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize",
        ];
        if !SUFFIXES.contains(&suffix.as_str()) {
            return Err(self.error(format!("unknown integer suffix '{suffix}'"), span));
        }
        Ok(TokenKind::IntSuffix(value, suffix))
    }

    fn read_ident(&mut self, first: char) -> TokenKind {
        let mut word = String::from(first);
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                word.push(c);
                self.advance();
            } else {
                break;
            }
        }
        use TokenKind as T;
        match word.as_str() {
            "let" => T::Let,
            "fn" => T::Fn,
            "if" => T::If,
            "else" => T::Else,
            "while" => T::While,
            "loop" => T::Loop,
            "for" => T::For,
            "in" => T::In,
            "break" => T::Break,
            "continue" => T::Continue,
            "return" => T::Return,
            "true" => T::True,
            "false" => T::False,
            "struct" => T::Struct,
            "enum" => T::Enum,
            "impl" => T::Impl,
            "match" => T::Match,
            "as" => T::As,
            "static" => T::Static,
            "const" => T::Const,
            "extern" => T::Extern,
            "import" => T::Import,
            "sizeof" => T::Sizeof,
            "mut" => T::Mut,
            "unsafe" => T::Unsafe,
            "not" => T::Bang,
            "and" => T::AndAnd,
            "or" => T::OrOr,
            _ => T::Ident(word),
        }
    }
}
