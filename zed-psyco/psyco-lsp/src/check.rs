use std::{
    collections::{HashMap, HashSet},
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
};

use lsp_types::{Diagnostic, DiagnosticSeverity};
use psycoc::{
    Abi, Block, Expr, ExprKind, Function, Lexer, Parser, Program, Span, Stmt, Type, TypeChecker,
};

use crate::analysis::Analysis;

/// `include_program` nesting limit, the same as the compiler's.
const MAX_INCLUDE_DEPTH: usize = 8;

type LetTypes = HashMap<(usize, usize), String>;

pub struct CheckResult {
    pub diagnostics: Vec<Diagnostic>,
    pub let_types: LetTypes,
}

pub struct Loader<'a> {
    pub read: &'a dyn Fn(&Path) -> Option<String>,
    pub resolve: &'a dyn Fn(Option<&Path>, &str) -> PathBuf,
    pub key: &'a dyn Fn(&Path) -> String,
}

pub fn check(an: &Analysis, path: Option<&Path>, loader: &Loader) -> CheckResult {
    let mut let_types = HashMap::new();
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        let mut checker = Checker {
            loader,
            programs: Vec::new(),
        };
        checker.check(&an.text, path, false, Some(&mut let_types))
    }));
    let diagnostics = match outcome {
        Ok(errors) => {
            let mut seen = HashSet::new();
            errors
                .into_iter()
                .map(|(span, message)| (an.word_range(span.line, span.col), message))
                .filter(|(range, message)| seen.insert((range.start, message.clone())))
                .map(|(range, message)| Diagnostic {
                    range,
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("psycoc".into()),
                    message,
                    ..Default::default()
                })
                .collect()
        }
        Err(_) => {
            eprintln!("psyco-lsp: the compiler panicked while checking the document");
            Vec::new()
        }
    };
    CheckResult {
        diagnostics,
        let_types,
    }
}

struct Checker<'a> {
    loader: &'a Loader<'a>,
    /// Programs being checked: the document, then the chain of
    /// `include_program` that leads to the current one.
    programs: Vec<String>,
}

impl Checker<'_> {
    /// Errors of the program whose main file is `text`, all located in that
    /// file. A `standalone` program needs its own `main`.
    fn check(
        &mut self,
        text: &str,
        path: Option<&Path>,
        standalone: bool,
        let_types: Option<&mut LetTypes>,
    ) -> Vec<(Span, String)> {
        let key = path.map(|p| (self.loader.key)(p));
        self.programs.extend(key.clone());
        let mut errors = self.check_source(text, path, standalone, let_types);
        if key.is_some() {
            self.programs.pop();
        }
        // An error outside of any function is found again on every attempt.
        let mut seen = HashSet::new();
        errors.retain(|(span, message)| seen.insert((span.line, span.col, message.clone())));
        errors
    }

    fn check_source(
        &mut self,
        text: &str,
        path: Option<&Path>,
        standalone: bool,
        mut let_types: Option<&mut LetTypes>,
    ) -> Vec<(Span, String)> {
        let loader = self.loader;
        let tokens = match Lexer::with_file(text, 0).tokenize() {
            Ok(t) => t,
            Err(e) => return vec![(e.span, e.message)],
        };
        let mut program = match Parser::new(tokens).parse_program() {
            Ok(p) => p,
            Err(e) => return vec![(e.span, e.message)],
        };

        let mut paths: Vec<Option<PathBuf>> = vec![path.map(Path::to_path_buf)];
        let mut origin: Vec<Option<Span>> = vec![None];
        let mut seen: HashSet<String> = path.map(|p| (loader.key)(p)).into_iter().collect();
        let mut queue: Vec<(usize, String, Span)> = program
            .imports
            .iter()
            .map(|(import, span)| (0, import.clone(), *span))
            .collect();
        let mut next = 0;
        while next < queue.len() {
            let (from, import, span) = queue[next].clone();
            next += 1;
            let root = origin[from].unwrap_or(span);
            let target = (loader.resolve)(paths[from].as_deref(), &import);
            if !seen.insert((loader.key)(&target)) {
                continue;
            }
            let display = target.to_string_lossy().replace('\\', "/");
            let Some(src) = (loader.read)(&target) else {
                return vec![(root, format!("cannot read {display}"))];
            };
            let file = paths.len();
            paths.push(Some(target));
            origin.push(Some(root));
            let parsed = Lexer::with_file(&src, file)
                .tokenize()
                .map_err(|e| (e.span, e.message))
                .and_then(|t| Parser::new(t).parse_program().map_err(|e| (e.span, e.message)));
            match parsed {
                Ok(p) => {
                    for (import, span) in &p.imports {
                        queue.push((file, import.clone(), *span));
                    }
                    program.merge(p);
                }
                Err((s, message)) => {
                    return vec![(root, format!("{display}:{}:{}: {message}", s.line, s.col))];
                }
            }
        }

        if !standalone && !program.functions.iter().any(|f| f.name == "main") {
            let span = Span {
                line: 1,
                col: 1,
                file: 0,
            };
            program.functions.push(Function {
                name: "main".into(),
                params: Vec::new(),
                return_type: None,
                body: Block {
                    stmts: Vec::new(),
                    span,
                },
                attrs: Vec::new(),
                trusted: false,
                span,
            });
        }

        // An error in an imported file is reported on the `import` that
        // brought the file in.
        let locate = |span: Span, message: String| {
            if span.file == 0 {
                return (span, message);
            }
            let file = paths
                .get(span.file)
                .cloned()
                .flatten()
                .map_or_else(|| "<import>".into(), |p| p.to_string_lossy().replace('\\', "/"));
            let root = origin.get(span.file).copied().flatten().unwrap_or(span);
            (root, format!("{file}:{}:{}: {message}", span.line, span.col))
        };

        let mut errors: Vec<(Span, String)> = self
            .embed_includes(&mut program, &paths)
            .into_iter()
            .map(|(span, message)| locate(span, message))
            .collect();

        // The checker stops at the first error. To report the others and still
        // know the types everywhere else, replace the body of the function that
        // failed with `loop {}` and check again.
        let mut stubbed = HashSet::new();
        loop {
            let mut attempt = program.clone();
            // `include_bytes` / `include_program` resolve paths from the file names.
            let files = paths
                .iter()
                .map(|p| p.as_ref().map_or_else(String::new, |p| p.to_string_lossy().into_owned()))
                .collect();
            let e = match TypeChecker::new().with_files(files).check_program(&mut attempt) {
                Ok(_) => {
                    if let Some(let_types) = let_types.as_deref_mut() {
                        for f in &attempt.functions {
                            collect_block(&f.body, &attempt, let_types);
                        }
                    }
                    break;
                }
                Err(e) => e,
            };
            errors.push(locate(e.span, e.message));

            // The function whose `fn` comes last before the error.
            let culprit = program
                .functions
                .iter()
                .enumerate()
                .filter(|(_, f)| {
                    f.span.file == e.span.file && (f.span.line, f.span.col) <= (e.span.line, e.span.col)
                })
                .max_by_key(|(_, f)| (f.span.line, f.span.col))
                .map(|(i, _)| i);
            let Some(i) = culprit.filter(|&i| stubbed.insert(i)) else {
                break;
            };
            let body = &mut program.functions[i].body;
            body.stmts = vec![Stmt::Loop {
                body: Block {
                    stmts: Vec::new(),
                    span: body.span,
                },
                span: body.span,
            }];
        }
        errors
    }

    /// Replaces every `include_bytes("f")` / `include_program("f.psy")` with
    /// empty bytes and returns what is wrong with the files. The compiler
    /// would read them from disk, build the programs and stop at the first
    /// error: here programs are only checked, from the open documents, and
    /// every include is reported on its own path.
    fn embed_includes(
        &mut self,
        program: &mut Program,
        paths: &[Option<PathBuf>],
    ) -> Vec<(Span, String)> {
        let mut errors = Vec::new();
        let mut embed = |e: &mut Expr| {
            let ExprKind::Call { callee, args, .. } = &e.kind else {
                return;
            };
            let (ExprKind::Var(name), [arg]) = (&callee.kind, args.as_slice()) else {
                return;
            };
            let ExprKind::Str(rel) = &arg.kind else {
                return;
            };
            let compiled = match name.as_str() {
                "include_program" => true,
                "include_bytes" => false,
                _ => return,
            };
            let from = paths.get(arg.span.file).and_then(|p| p.as_deref());
            let target = crate::resolve_include(from, &String::from_utf8_lossy(rel));
            let result = if compiled {
                self.include_program(&target)
            } else if target.is_file() || (self.loader.read)(&target).is_some() {
                Ok(())
            } else {
                Err(format!("cannot read {}", target.to_string_lossy().replace('\\', "/")))
            };
            if let Err(message) = result {
                errors.push((arg.span, message));
            }
            e.kind = ExprKind::Bytes(Vec::new());
        };
        for f in &mut program.functions {
            walk_block(&mut f.body, &mut embed);
        }
        errors
    }

    /// Checks the program an `include_program` embeds.
    fn include_program(&mut self, target: &Path) -> Result<(), String> {
        let display = target.to_string_lossy().replace('\\', "/");
        let key = (self.loader.key)(target);
        if self.programs.len() > MAX_INCLUDE_DEPTH || self.programs.contains(&key) {
            return Err("include_program nested too deeply (does a program include itself?)".into());
        }
        let Some(src) = (self.loader.read)(target) else {
            return Err(format!("cannot read {display}"));
        };
        let errors = self.check(&src, Some(target), true, None);
        if errors.is_empty() {
            return Ok(());
        }
        let lines: Vec<String> = errors
            .iter()
            .map(|(span, message)| format!("{}:{}: {message}", span.line, span.col))
            .collect();
        Err(format!("in program {display}:\n{}", lines.join("\n")))
    }
}

fn walk_block(b: &mut Block, f: &mut impl FnMut(&mut Expr)) {
    for s in &mut b.stmts {
        walk_stmt(s, f);
    }
}

fn walk_stmt(s: &mut Stmt, f: &mut impl FnMut(&mut Expr)) {
    match s {
        Stmt::Let { value, .. } | Stmt::Return { value, .. } => {
            if let Some(value) = value {
                walk_expr(value, f);
            }
        }
        Stmt::Assign { target, value, .. } => {
            walk_expr(target, f);
            walk_expr(value, f);
        }
        Stmt::Expr(e) => walk_expr(e, f),
        Stmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            walk_expr(cond, f);
            walk_block(then_block, f);
            if let Some(e) = else_block {
                walk_block(e, f);
            }
        }
        Stmt::While { cond, body, .. } => {
            walk_expr(cond, f);
            walk_block(body, f);
        }
        Stmt::For {
            start, end, body, ..
        } => {
            walk_expr(start, f);
            walk_expr(end, f);
            walk_block(body, f);
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            walk_expr(scrutinee, f);
            for arm in arms {
                for pattern in arm.patterns.iter_mut().flatten() {
                    walk_expr(pattern, f);
                }
                walk_block(&mut arm.body, f);
            }
        }
        Stmt::Loop { body, .. } | Stmt::Block(body) => walk_block(body, f),
        Stmt::Break(_) | Stmt::Continue(_) => {}
    }
}

fn walk_expr(e: &mut Expr, f: &mut impl FnMut(&mut Expr)) {
    f(e);
    match &mut e.kind {
        ExprKind::Unary { operand: a, .. }
        | ExprKind::Cast { expr: a, .. }
        | ExprKind::Field { base: a, .. }
        | ExprKind::Unsize(a) => walk_expr(a, f),
        ExprKind::Binary { lhs: a, rhs: b, .. }
        | ExprKind::Index { base: a, index: b }
        | ExprKind::ArrayRepeat { value: a, count: b } => {
            walk_expr(a, f);
            walk_expr(b, f);
        }
        ExprKind::Call { callee, args, .. } => {
            walk_expr(callee, f);
            for a in args {
                walk_expr(a, f);
            }
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, value, _) in fields {
                walk_expr(value, f);
            }
        }
        ExprKind::ArrayLit(items) => {
            for item in items {
                walk_expr(item, f);
            }
        }
        ExprKind::Range { base, start, end } => {
            walk_expr(base, f);
            for bound in [start, end].into_iter().flatten() {
                walk_expr(bound, f);
            }
        }
        _ => {}
    }
}

fn collect_block(b: &Block, p: &Program, out: &mut LetTypes) {
    for s in &b.stmts {
        collect_stmt(s, p, out);
    }
}

fn collect_stmt(s: &Stmt, p: &Program, out: &mut LetTypes) {
    match s {
        Stmt::Let {
            span,
            r#type: Some(t),
            ..
        } if span.file == 0 => {
            out.insert((span.line, span.col), type_name(t, p));
        }
        Stmt::If {
            then_block,
            else_block,
            ..
        } => {
            collect_block(then_block, p, out);
            if let Some(e) = else_block {
                collect_block(e, p, out);
            }
        }
        Stmt::For {
            start, body, span, ..
        } => {
            // Keyed by the `for` keyword: the type of the loop variable.
            if let (0, Some(t)) = (span.file, &start.r#type) {
                out.insert((span.line, span.col), type_name(t, p));
            }
            collect_block(body, p, out)
        }
        Stmt::While { body, .. } | Stmt::Loop { body, .. } => collect_block(body, p, out),
        Stmt::Match { arms, .. } => {
            for arm in arms {
                collect_block(&arm.body, p, out);
            }
        }
        Stmt::Block(b) => collect_block(b, p, out),
        _ => {}
    }
}

pub fn type_name(t: &Type, p: &Program) -> String {
    match t {
        Type::Unit => "()".into(),
        Type::Bool => "bool".into(),
        Type::Str => "Str".into(),
        Type::Int(i) => i.name().into(),
        Type::Ref(t, m) => format!("&{}{}", if *m { "mut " } else { "" }, type_name(t, p)),
        Type::Raw(t, m) => format!("*{} {}", if *m { "mut" } else { "const" }, type_name(t, p)),
        Type::Slice(t, m) => format!("&{}[{}]", if *m { "mut " } else { "" }, type_name(t, p)),
        Type::Array(t, n) => format!("[{}; {n}]", type_name(t, p)),
        Type::Struct(id) => p.structs.get(*id).map_or("?".into(), |s| s.name.clone()),
        Type::Enum(id) => p.enums.get(*id).map_or("?".into(), |e| e.name.clone()),
        Type::Fn(f) => {
            let params: Vec<_> = f.params.iter().map(|t| type_name(t, p)).collect();
            let prefix = match f.abi {
                Abi::Efi => "extern fn",
                Abi::Interrupt => "interrupt fn",
                Abi::Native => "fn",
            };
            let ret = if f.ret == Type::Unit {
                String::new()
            } else {
                format!(" -> {}", type_name(&f.ret, p))
            };
            format!("{prefix}({}){ret}", params.join(", "))
        }
    }
}
