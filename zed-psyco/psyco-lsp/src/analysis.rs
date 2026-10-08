use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
};

use lsp_types::{Position, Range};
use psycoc::{Lexer, Token, TokenKind};

static EOF: TokenKind = TokenKind::Eof;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefKind {
    Function,
    Method,
    Struct,
    Field,
    Enum,
    Variant,
    Const,
    Static,
    Local,
    Param,
}

impl DefKind {
    pub fn is_item(self) -> bool {
        matches!(
            self,
            DefKind::Function | DefKind::Struct | DefKind::Enum | DefKind::Const | DefKind::Static
        )
    }

    pub fn is_local(self) -> bool {
        matches!(self, DefKind::Local | DefKind::Param)
    }
}

#[derive(Clone, Debug)]
pub struct Def {
    pub name: String,
    pub kind: DefKind,
    pub tok: usize,
    pub start_tok: usize,
    pub end_tok: usize,
    pub container: Option<String>,
    pub detail: String,
    pub params: Vec<String>,
    pub scope: Option<(usize, usize)>,
    pub explicit_type: bool,
}

impl Def {
    pub fn qualified_name(&self) -> String {
        match (&self.container, self.kind) {
            (Some(c), DefKind::Method | DefKind::Variant) => format!("{c}::{}", self.name),
            _ => self.name.clone(),
        }
    }
}

pub struct Analysis {
    pub text: String,
    lines: Vec<String>,
    line_starts: Vec<usize>,
    pub tokens: Vec<Token>,
    pub defs: Vec<Def>,
    pub imports: Vec<(String, usize)>,
    close_of: HashMap<usize, usize>,
    encl: Vec<Option<usize>>,
}

pub fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn lex_tolerant(text: &str) -> Vec<Token> {
    let mut chars: Vec<char> = text.chars().collect();
    let mut line_starts = vec![0];
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' {
            line_starts.push(i + 1);
        }
    }
    for _ in 0..64 {
        let src: String = chars.iter().collect();
        match Lexer::new(&src).tokenize() {
            Ok(tokens) => return tokens,
            Err(e) => {
                let idx = line_starts
                    .get(e.span.line.saturating_sub(1))
                    .map(|s| s + e.span.col.saturating_sub(1));
                let fixable = matches!(idx, Some(i) if i < chars.len() && chars[i] != '\n');
                if !fixable {
                    break;
                }
                chars[idx.unwrap()] = ' ';
            }
        }
    }
    Vec::new()
}

impl Analysis {
    pub fn new(text: String) -> Self {
        let lines = text
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
            .collect();
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        let tokens = lex_tolerant(&text);
        let mut an = Analysis {
            text,
            lines,
            line_starts,
            tokens,
            defs: Vec::new(),
            imports: Vec::new(),
            close_of: HashMap::new(),
            encl: Vec::new(),
        };
        an.match_brackets();
        an.index();
        an
    }

    pub fn line(&self, line: usize) -> &str {
        self.lines
            .get(line.wrapping_sub(1))
            .map_or("", String::as_str)
    }

    pub fn position(&self, line: usize, col: usize) -> Position {
        let character: usize = self
            .line(line)
            .chars()
            .take(col.saturating_sub(1))
            .map(char::len_utf16)
            .sum();
        Position::new(line.saturating_sub(1) as u32, character as u32)
    }

    pub fn line_col(&self, p: Position) -> (usize, usize) {
        let line = p.line as usize + 1;
        let mut units = 0;
        let mut col = 1;
        for c in self.line(line).chars() {
            if units >= p.character as usize {
                break;
            }
            units += c.len_utf16();
            col += 1;
        }
        (line, col)
    }

    fn offset(&self, line: usize, col: usize) -> usize {
        let Some(start) = self.line_starts.get(line.wrapping_sub(1)) else {
            return self.text.len();
        };
        let text = self.line(line);
        let within = text
            .char_indices()
            .nth(col.saturating_sub(1))
            .map_or(text.len(), |(i, _)| i);
        start + within
    }

    pub fn word_range(&self, line: usize, col: usize) -> Range {
        let chars: Vec<char> = self.line(line).chars().collect();
        let start = col.saturating_sub(1);
        let mut end = start;
        while end < chars.len() && is_word(chars[end]) {
            end += 1;
        }
        if end == start && start < chars.len() {
            end += 1;
        }
        Range::new(self.position(line, start + 1), self.position(line, end + 1))
    }

    fn tok_len(&self, i: usize) -> usize {
        let t = &self.tokens[i];
        if let TokenKind::Ident(name) = &t.kind {
            return name.chars().count();
        }
        let rest: Vec<char> = self
            .line(t.span.line)
            .chars()
            .skip(t.span.col.saturating_sub(1))
            .collect();
        if rest.first() == Some(&'\'') {
            // Character literal: up to the closing quote, skipping escapes.
            let mut k = 1;
            while k < rest.len() && rest[k] != '\'' {
                k += if rest[k] == '\\' { 2 } else { 1 };
            }
            return (k + 1).min(rest.len()).max(1);
        }
        rest.iter().take_while(|c| is_word(**c)).count().max(1)
    }

    pub fn tok_range(&self, i: usize) -> Range {
        let s = self.tokens[i].span;
        Range::new(
            self.position(s.line, s.col),
            self.position(s.line, s.col + self.tok_len(i)),
        )
    }

    pub fn token_at(&self, p: Position) -> Option<usize> {
        let (line, col) = self.line_col(p);
        let after = self.cursor_index(line, col + 1);
        (after.saturating_sub(2)..after).rev().find(|&i| {
            let t = &self.tokens[i];
            t.kind != TokenKind::Eof
                && t.span.line == line
                && t.span.col <= col
                && col <= t.span.col + self.tok_len(i)
                && self.line(line).chars().nth(t.span.col - 1).is_some_and(is_word)
        })
    }

    /// `{` minus `}` before token index `upto`.
    pub fn brace_depth(&self, upto: usize) -> i32 {
        self.tokens[..upto.min(self.tokens.len())]
            .iter()
            .map(|t| match t.kind {
                TokenKind::LBrace => 1,
                TokenKind::RBrace => -1,
                _ => 0,
            })
            .sum()
    }

    /// Name inside `#[name]` or `#![name]`.
    pub fn is_attribute_name(&self, tok: usize) -> bool {
        if self.ident(tok).is_none() {
            return false;
        }
        // Walk back over `name(arg),` items of a `#[a, b(1), c]` list.
        let mut k = tok;
        while *self.kind_before(k, 1) == TokenKind::Comma {
            k -= 1;
            if *self.kind_before(k, 1) == TokenKind::RParen {
                k = k.saturating_sub(3);
            }
            if k == 0 || self.ident(k - 1).is_none() {
                return false;
            }
            k -= 1;
        }
        *self.kind_before(k, 1) == TokenKind::LBracket
            && (*self.kind_before(k, 2) == TokenKind::Hash
                || (*self.kind_before(k, 2) == TokenKind::Bang
                    && *self.kind_before(k, 3) == TokenKind::Hash))
    }

    /// Integer or character literal under the cursor.
    pub fn literal_at(&self, p: Position) -> Option<usize> {
        let (line, col) = self.line_col(p);
        let after = self.cursor_index(line, col + 1);
        (after.saturating_sub(2)..after).rev().find(|&i| {
            let t = &self.tokens[i];
            matches!(t.kind, TokenKind::Int(_) | TokenKind::IntSuffix(..))
                && t.span.line == line
                && t.span.col <= col
                && col <= t.span.col + self.tok_len(i)
        })
    }

    pub fn cursor_index(&self, line: usize, col: usize) -> usize {
        self.tokens
            .partition_point(|t| (t.span.line, t.span.col) < (line, col))
    }

    pub fn kind(&self, i: usize) -> &TokenKind {
        self.tokens.get(i).map_or(&EOF, |t| &t.kind)
    }

    fn kind_before(&self, i: usize, back: usize) -> &TokenKind {
        if i >= back {
            self.kind(i - back)
        } else {
            &EOF
        }
    }

    pub fn ident(&self, i: usize) -> Option<&str> {
        match self.kind(i) {
            TokenKind::Ident(name) => Some(name),
            _ => None,
        }
    }

    pub fn is_path_segment(&self, i: usize) -> bool {
        *self.kind_before(i, 1) == TokenKind::ColonColon
    }

    pub fn is_member(&self, i: usize) -> bool {
        *self.kind_before(i, 1) == TokenKind::Dot
    }

    fn text(&self, a: usize, b: usize) -> String {
        let (Some(ta), Some(tb)) = (self.tokens.get(a), self.tokens.get(b)) else {
            return String::new();
        };
        let from = self.offset(ta.span.line, ta.span.col);
        let to = self.offset(tb.span.line, tb.span.col);
        if to <= from {
            return String::new();
        }
        squash(&self.text[from..to])
    }

    fn rest_of_line(&self, a: usize) -> String {
        let s = self.tokens[a].span;
        let line: String = self.line(s.line).chars().skip(s.col - 1).collect();
        let line = match line.find("//") {
            Some(i) => &line[..i],
            None => &line,
        };
        squash(line)
    }

    fn close(&self, open: usize) -> usize {
        self.close_of
            .get(&open)
            .copied()
            .unwrap_or(self.tokens.len().saturating_sub(1))
    }

    fn block_end(&self, i: usize) -> usize {
        match self.encl.get(i).copied().flatten() {
            Some(open) => self.close(open),
            None => self.tokens.len().saturating_sub(1),
        }
    }

    fn body_after(&self, owner: usize, from: usize) -> Option<usize> {
        (from..self.tokens.len().min(from + 96)).find(|&k| {
            *self.kind(k) == TokenKind::LBrace && self.encl[k] == self.encl[owner]
        })
    }

    fn segment_end(&self, from: usize, stop: usize) -> usize {
        let mut depth = 0i32;
        for k in from..stop {
            match self.kind(k) {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => depth -= 1,
                TokenKind::Comma if depth == 0 => return k,
                _ => {}
            }
        }
        stop
    }

    fn match_brackets(&mut self) {
        let n = self.tokens.len();
        let mut stack: Vec<usize> = Vec::new();
        self.encl = vec![None; n];
        for i in 0..n {
            self.encl[i] = stack
                .iter()
                .rev()
                .find(|&&j| self.tokens[j].kind == TokenKind::LBrace)
                .copied();
            let open = match self.tokens[i].kind {
                TokenKind::LBrace | TokenKind::LParen | TokenKind::LBracket => {
                    stack.push(i);
                    continue;
                }
                TokenKind::RBrace => TokenKind::LBrace,
                TokenKind::RParen => TokenKind::LParen,
                TokenKind::RBracket => TokenKind::LBracket,
                _ => continue,
            };
            if let Some(pos) = stack.iter().rposition(|&j| self.tokens[j].kind == open) {
                self.close_of.insert(stack[pos], i);
                stack.truncate(pos);
            }
        }
    }

    fn push(&mut self, def: Def) {
        self.defs.push(def);
    }

    fn index(&mut self) {
        let mut impls: Vec<(String, usize, usize)> = Vec::new();
        for i in 0..self.tokens.len() {
            impls.retain(|(_, _, end)| *end > i);
            match self.kind(i).clone() {
                TokenKind::Impl => {
                    if let (Some(name), TokenKind::LBrace) = (self.ident(i + 1), self.kind(i + 2)) {
                        impls.push((name.to_string(), i + 2, self.close(i + 2)));
                    }
                }
                TokenKind::Fn => {
                    if self.ident(i + 1).is_some() {
                        let container = impls
                            .last()
                            .filter(|(_, open, _)| self.encl[i] == Some(*open))
                            .map(|(name, _, _)| name.clone());
                        self.index_fn(i, container);
                    }
                }
                TokenKind::Struct => self.index_struct(i),
                TokenKind::Enum => self.index_enum(i),
                TokenKind::Const | TokenKind::Static => self.index_global(i),
                TokenKind::Let => self.index_let(i),
                TokenKind::For => self.index_for(i),
                TokenKind::Import => {
                    if let TokenKind::Str(bytes) = self.kind(i + 1) {
                        let path = String::from_utf8_lossy(bytes).into_owned();
                        self.imports.push((path, i + 1));
                    }
                }
                _ => {}
            }
        }
    }

    fn index_fn(&mut self, i: usize, container: Option<String>) {
        let name = self.ident(i + 1).unwrap_or_default().to_string();
        let lp = i + 2;
        let rp = if *self.kind(lp) == TokenKind::LParen {
            self.close(lp)
        } else {
            lp
        };

        let mut params = Vec::new();
        let mut seg = lp + 1;
        while seg < rp {
            let end = self.segment_end(seg, rp);
            params.push((seg, end));
            seg = end + 1;
        }

        let body = self.body_after(i, rp + 1);
        let end_tok = body.map_or(rp, |b| self.close(b));
        let detail = match body {
            Some(b) => self.text(i, b),
            None => self.rest_of_line(i),
        };
        let scope = body.map(|b| (b, self.close(b))).unwrap_or((rp, rp));

        let mut param_texts = Vec::new();
        for (a, b) in params {
            let text = self.text(a, b);
            param_texts.push(text.clone());
            let Some(name_tok) = (a..b).find(|&k| self.ident(k).is_some()) else {
                continue;
            };
            let pname = self.ident(name_tok).unwrap_or_default().to_string();
            let is_param = *self.kind(name_tok + 1) == TokenKind::Colon || pname == "self";
            if !is_param {
                continue;
            }
            let detail = if pname == "self" {
                match &container {
                    Some(c) => format!("{text}: {}", text.replace("self", c)),
                    None => text.clone(),
                }
            } else {
                text.clone()
            };
            self.push(Def {
                name: pname,
                kind: DefKind::Param,
                tok: name_tok,
                start_tok: a,
                end_tok: b.saturating_sub(1),
                container: None,
                detail,
                params: Vec::new(),
                scope: Some(scope),
                explicit_type: true,
            });
        }

        self.push(Def {
            name,
            kind: if container.is_some() {
                DefKind::Method
            } else {
                DefKind::Function
            },
            tok: i + 1,
            start_tok: i,
            end_tok,
            container,
            detail,
            params: param_texts,
            scope: None,
            explicit_type: true,
        });
    }

    fn index_struct(&mut self, i: usize) {
        let Some(name) = self.ident(i + 1).map(str::to_string) else {
            return;
        };
        let lb = i + 2;
        let has_body = *self.kind(lb) == TokenKind::LBrace;
        let end_tok = if has_body { self.close(lb) } else { i + 1 };
        if has_body {
            for j in lb + 1..end_tok {
                if self.encl[j] == Some(lb)
                    && self.ident(j).is_some()
                    && *self.kind(j + 1) == TokenKind::Colon
                    && matches!(self.kind(j - 1), TokenKind::LBrace | TokenKind::Comma)
                {
                    let end = self.segment_end(j, end_tok);
                    self.push(Def {
                        name: self.ident(j).unwrap_or_default().to_string(),
                        kind: DefKind::Field,
                        tok: j,
                        start_tok: j,
                        end_tok: end.saturating_sub(1),
                        container: Some(name.clone()),
                        detail: self.text(j, end),
                        params: Vec::new(),
                        scope: None,
                        explicit_type: true,
                    });
                }
            }
        }
        self.push(Def {
            name: name.clone(),
            kind: DefKind::Struct,
            tok: i + 1,
            start_tok: i,
            end_tok,
            container: None,
            detail: format!("struct {name}"),
            params: Vec::new(),
            scope: None,
            explicit_type: true,
        });
    }

    fn index_enum(&mut self, i: usize) {
        let Some(name) = self.ident(i + 1).map(str::to_string) else {
            return;
        };
        let lb = (i + 2..self.tokens.len().min(i + 8)).find(|&k| *self.kind(k) == TokenKind::LBrace);
        let end_tok = lb.map_or(i + 1, |b| self.close(b));
        let detail = match lb {
            Some(b) => self.text(i, b),
            None => self.rest_of_line(i),
        };
        if let Some(lb) = lb {
            for j in lb + 1..end_tok {
                if self.encl[j] == Some(lb)
                    && self.ident(j).is_some()
                    && matches!(self.kind(j - 1), TokenKind::LBrace | TokenKind::Comma)
                {
                    let end = self.segment_end(j, end_tok);
                    self.push(Def {
                        name: self.ident(j).unwrap_or_default().to_string(),
                        kind: DefKind::Variant,
                        tok: j,
                        start_tok: j,
                        end_tok: end.saturating_sub(1),
                        container: Some(name.clone()),
                        detail: format!("{name}::{}", self.text(j, end)),
                        params: Vec::new(),
                        scope: None,
                        explicit_type: true,
                    });
                }
            }
        }
        self.push(Def {
            name,
            kind: DefKind::Enum,
            tok: i + 1,
            start_tok: i,
            end_tok,
            container: None,
            detail,
            params: Vec::new(),
            scope: None,
            explicit_type: true,
        });
    }

    fn index_global(&mut self, i: usize) {
        let mut j = i + 1;
        if *self.kind(j) == TokenKind::Mut {
            j += 1;
        }
        let Some(name) = self.ident(j).map(str::to_string) else {
            return;
        };
        let line = self.tokens[i].span.line;
        let end_tok = (j..self.tokens.len())
            .take_while(|&k| self.tokens[k].span.line == line && self.tokens[k].kind != TokenKind::Eof)
            .last()
            .unwrap_or(j);
        self.push(Def {
            name,
            kind: if *self.kind(i) == TokenKind::Const {
                DefKind::Const
            } else {
                DefKind::Static
            },
            tok: j,
            start_tok: i,
            end_tok,
            container: None,
            detail: self.rest_of_line(i),
            params: Vec::new(),
            scope: None,
            explicit_type: true,
        });

        // `#[getter]` / `#[setter]` on a static generate `get_NAME()` / `set_NAME(value)`.
        if *self.kind(i) != TokenKind::Static {
            return;
        }
        let attrs = self.attributes_before(i);
        let ty_start = j + 2;
        let ty_end = (ty_start..self.tokens.len())
            .find(|&k| {
                self.tokens[k].span.line != line
                    || matches!(self.tokens[k].kind, TokenKind::Eq | TokenKind::Eof)
            })
            .unwrap_or(self.tokens.len());
        let ty = if *self.kind(j + 1) == TokenKind::Colon {
            self.text(ty_start, ty_end)
        } else {
            "?".into()
        };
        let accessor = |name: String, detail: String, params: Vec<String>| Def {
            name,
            kind: DefKind::Function,
            tok: j,
            start_tok: i,
            end_tok,
            container: Some(self.ident(j).unwrap_or_default().to_string()),
            detail,
            params,
            scope: None,
            explicit_type: true,
        };
        let name = self.ident(j).unwrap_or_default().to_string();
        let mut generated = Vec::new();
        if attrs.iter().any(|a| a == "getter") {
            generated.push(accessor(format!("get_{name}"), format!("fn get_{name}() -> {ty}"), Vec::new()));
        }
        if attrs.iter().any(|a| a == "setter") {
            let param = format!("value: {ty}");
            generated.push(accessor(
                format!("set_{name}"),
                format!("fn set_{name}({param})"),
                vec![param],
            ));
        }
        for def in generated {
            self.push(def);
        }
    }

    /// Names in the `#[...]` attributes right before token `i`.
    fn attributes_before(&self, i: usize) -> Vec<String> {
        let mut names = Vec::new();
        let mut end = i;
        while end >= 1 && *self.kind(end - 1) == TokenKind::RBracket {
            let close = end - 1;
            let Some(open) = (0..close).rev().find(|&k| *self.kind(k) == TokenKind::LBracket) else {
                break;
            };
            if open == 0 || *self.kind(open - 1) != TokenKind::Hash {
                break;
            }
            for k in open + 1..close {
                if let Some(n) = self.ident(k) {
                    if matches!(self.kind(k - 1), TokenKind::LBracket | TokenKind::Comma) {
                        names.push(n.to_string());
                    }
                }
            }
            end = open - 1;
        }
        names
    }

    fn index_let(&mut self, i: usize) {
        let mut j = i + 1;
        if *self.kind(j) == TokenKind::Mut {
            j += 1;
        }
        let Some(name) = self.ident(j).map(str::to_string) else {
            return;
        };
        let line = self.tokens[i].span.line;
        let stop = (j..self.tokens.len())
            .find(|&k| {
                self.tokens[k].span.line != line
                    || matches!(self.tokens[k].kind, TokenKind::Eq | TokenKind::Eof)
            })
            .unwrap_or(self.tokens.len());
        let detail = if stop < self.tokens.len() && self.tokens[stop].span.line == line {
            self.text(i, stop)
        } else {
            self.rest_of_line(i)
        };
        self.push(Def {
            name,
            kind: DefKind::Local,
            tok: j,
            start_tok: i,
            end_tok: stop.saturating_sub(1).max(j),
            container: None,
            detail,
            params: Vec::new(),
            scope: Some((j, self.block_end(i))),
            explicit_type: *self.kind(j + 1) == TokenKind::Colon,
        });
    }

    fn index_for(&mut self, i: usize) {
        let Some(name) = self.ident(i + 1).map(str::to_string) else {
            return;
        };
        if *self.kind(i + 2) != TokenKind::In {
            return;
        }
        let body = self.body_after(i, i + 3);
        let range = body.map_or_else(|| self.rest_of_line(i + 3), |b| self.text(i + 3, b));
        self.push(Def {
            name: name.clone(),
            kind: DefKind::Local,
            tok: i + 1,
            start_tok: i,
            end_tok: i + 1,
            container: None,
            detail: format!("for {name} in {range}"),
            params: Vec::new(),
            scope: Some((i + 1, body.map_or(i + 1, |b| self.close(b)))),
            explicit_type: false,
        });
    }

    pub fn def_at(&self, tok: usize) -> Option<usize> {
        self.defs.iter().position(|d| d.tok == tok)
    }

    pub fn local_at(&self, name: &str, at: usize) -> Option<usize> {
        self.defs
            .iter()
            .enumerate()
            .filter(|(_, d)| d.kind.is_local() && d.name == name)
            .filter(|(_, d)| d.scope.is_some_and(|(s, e)| s <= at && at <= e) && d.tok <= at)
            .max_by_key(|(_, d)| d.tok)
            .map(|(i, _)| i)
    }

    pub fn locals_at(&self, at: usize) -> Vec<&Def> {
        let mut found: Vec<&Def> = self
            .defs
            .iter()
            .filter(|d| d.kind.is_local())
            .filter(|d| d.scope.is_some_and(|(s, e)| s <= at && at <= e) && d.tok <= at)
            .collect();
        found.sort_by_key(|d| std::cmp::Reverse(d.tok));
        let mut seen = std::collections::HashSet::new();
        found.retain(|d| seen.insert(d.name.clone()));
        found
    }

    pub fn doc_comment(&self, def: &Def) -> Option<String> {
        let mut line = self.tokens.get(def.start_tok)?.span.line;
        let mut docs = Vec::new();
        while line > 1 {
            line -= 1;
            let text = self.line(line).trim();
            if text.starts_with("#[") {
                continue;
            }
            let Some(comment) = text.strip_prefix("//") else {
                break;
            };
            if comment.starts_with('>') {
                break;
            }
            let comment = comment.trim_start_matches('/');
            docs.push(comment.strip_prefix(' ').unwrap_or(comment).to_string());
        }
        if docs.is_empty() {
            return None;
        }
        docs.reverse();
        Some(docs.join("\n"))
    }

    pub fn call_at(&self, cursor: usize) -> Option<(usize, u32)> {
        let mut depth = 0;
        let mut active = 0;
        for j in (0..cursor.min(self.tokens.len())).rev() {
            match self.kind(j) {
                TokenKind::RParen | TokenKind::RBracket => depth += 1,
                TokenKind::LParen if depth == 0 => return Some((j, active)),
                TokenKind::LBracket if depth == 0 => return None,
                TokenKind::LParen | TokenKind::LBracket => depth -= 1,
                TokenKind::Comma if depth == 0 => active += 1,
                TokenKind::LBrace | TokenKind::RBrace if depth == 0 => return None,
                _ => {}
            }
        }
        None
    }

    pub fn struct_literal_name(&self, tok: usize) -> Option<&str> {
        if *self.kind(tok + 1) != TokenKind::Colon {
            return None;
        }
        let open = self.encl.get(tok).copied().flatten()?;
        if !matches!(self.kind_before(tok, 1), TokenKind::LBrace | TokenKind::Comma) {
            return None;
        }
        self.ident(open.checked_sub(1)?)
    }
}

pub struct Unit {
    pub uri: String,
    pub path: Option<PathBuf>,
    pub an: Arc<Analysis>,
    pub let_types: Arc<HashMap<(usize, usize), String>>,
}

pub struct World {
    pub units: Vec<Unit>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Target {
    pub unit: usize,
    pub def: usize,
}

impl World {
    pub fn def(&self, t: Target) -> &Def {
        &self.units[t.unit].an.defs[t.def]
    }

    pub fn find_all(&self, mut pred: impl FnMut(&Def) -> bool) -> Vec<Target> {
        let mut out = Vec::new();
        for (unit, u) in self.units.iter().enumerate() {
            for (def, d) in u.an.defs.iter().enumerate() {
                if pred(d) {
                    out.push(Target { unit, def });
                }
            }
        }
        out
    }

    fn find_item(&self, from: usize, name: &str) -> Vec<Target> {
        let order = std::iter::once(from).chain((0..self.units.len()).filter(|&u| u != from));
        for unit in order {
            let found: Vec<Target> = self.units[unit]
                .an
                .defs
                .iter()
                .enumerate()
                .filter(|(_, d)| d.kind.is_item() && d.name == name)
                .map(|(def, _)| Target { unit, def })
                .collect();
            if !found.is_empty() {
                return found;
            }
        }
        Vec::new()
    }

    pub fn resolve(&self, unit: usize, tok: usize) -> Vec<Target> {
        let an = &self.units[unit].an;
        let Some(name) = an.ident(tok) else {
            return Vec::new();
        };
        if let Some(def) = an.def_at(tok) {
            return vec![Target { unit, def }];
        }
        if an.is_path_segment(tok) {
            if let Some(owner) = tok.checked_sub(2).and_then(|t| an.ident(t)) {
                return self.find_all(|d| {
                    d.container.as_deref() == Some(owner)
                        && d.name == name
                        && matches!(d.kind, DefKind::Variant | DefKind::Method)
                });
            }
        }
        if an.is_member(tok) {
            return self.find_all(|d| {
                d.name == name && matches!(d.kind, DefKind::Field | DefKind::Method)
            });
        }
        if let Some(owner) = an.struct_literal_name(tok) {
            let fields = self.find_all(|d| {
                d.kind == DefKind::Field && d.container.as_deref() == Some(owner) && d.name == name
            });
            if !fields.is_empty() {
                return fields;
            }
        }
        if let Some(def) = an.local_at(name, tok) {
            return vec![Target { unit, def }];
        }
        self.find_item(unit, name)
    }

    pub fn references(&self, target: Target, units: impl Iterator<Item = usize>) -> Vec<(usize, usize)> {
        let name = &self.def(target).name;
        let mut out = Vec::new();
        for unit in units {
            let an = &self.units[unit].an;
            for tok in 0..an.tokens.len() {
                if an.ident(tok) == Some(name.as_str()) && self.resolve(unit, tok).contains(&target) {
                    out.push((unit, tok));
                }
            }
        }
        out
    }
}
