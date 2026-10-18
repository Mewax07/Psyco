use std::{
    collections::{HashMap, HashSet},
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
};

use lsp_types::{Diagnostic, DiagnosticSeverity};
use psycoc::{
    Abi, Block, Expr, ExprKind, Function, Lexer, Parser, PatKind, Pattern, Program, ProgramInfo,
    Span, Stmt, Type, TypeChecker,
};

use crate::{analysis::Analysis, stdlib};

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

        // Load the imports the way the driver does: a file imported `as m`
        // becomes module `m`, and the same file may be loaded once per alias.
        // The standard library files come last, unless imported explicitly.
        let mut paths: Vec<Option<PathBuf>> = vec![path.map(Path::to_path_buf)];
        // File names as the driver gives them to the checker: standard library
        // files are `std/x.psy`, which is how it knows to check their
        // functions only when they are used.
        let mut names: Vec<String> =
            vec![path.map_or_else(String::new, |p| p.to_string_lossy().into_owned())];
        let mut origin: Vec<Option<Span>> = vec![None];
        let mut seen: HashSet<(String, Option<String>)> =
            path.map(|p| ((loader.key)(p), None)).into_iter().collect();
        let mut queue: Vec<(usize, String, Option<String>, Option<Span>)> = program
            .imports
            .iter()
            .map(|(import, alias, span)| (0, import.clone(), alias.clone(), Some(*span)))
            .collect();
        queue.extend(stdlib::FILES.iter().map(|(name, _)| (0, name.to_string(), None, None)));
        let mut alloc_linux = None;
        let mut next = 0;
        while next < queue.len() {
            let (from, import, alias, span) = queue[next].clone();
            next += 1;
            let root = span.map_or(origin[from], |s| Some(origin[from].unwrap_or(s)));
            let std_name = stdlib::name_of(&import);
            let target = match std_name {
                Some(name) => stdlib::path(name),
                None => (loader.resolve)(paths[from].as_deref(), &import),
            };
            if !seen.insert(((loader.key)(&target), alias.clone())) {
                continue;
            }
            let display = match std_name {
                Some(name) => name.to_string(),
                None => target.to_string_lossy().replace('\\', "/"),
            };
            let src = match std_name {
                Some(name) => Some(stdlib::source(name, loader.read)),
                None => (loader.read)(&target),
            };
            let at = root.unwrap_or(Span { line: 1, col: 1, file: 0 });
            let Some(src) = src else {
                return vec![(at, format!("cannot read {display}"))];
            };
            let file = paths.len();
            if std_name == Some(stdlib::ALLOC_LINUX) {
                alloc_linux = Some(file);
            }
            names.push(match std_name {
                Some(name) => name.to_string(),
                None => target.to_string_lossy().into_owned(),
            });
            paths.push(Some(target));
            origin.push(root);
            let parsed = Lexer::with_file(&src, file)
                .tokenize()
                .map_err(|e| (e.span, e.message))
                .and_then(|t| {
                    Parser::new(t)
                        .with_module(alias.clone().unwrap_or_default())
                        .parse_program()
                        .map_err(|e| (e.span, e.message))
                });
            match parsed {
                Ok(p) => {
                    for (import, alias, span) in &p.imports {
                        queue.push((file, import.clone(), alias.clone(), Some(*span)));
                    }
                    program.merge(p);
                }
                Err((s, message)) => {
                    return vec![(at, format!("{display}:{}:{}: {message}", s.line, s.col))];
                }
            }
        }

        // Like the driver: the Linux allocator goes away when the program
        // brings its own `psy_alloc`.
        if let Some(file) = alloc_linux {
            let own_heap = program
                .functions
                .iter()
                .any(|f| f.name == "psy_alloc" && f.span.file != file);
            if own_heap {
                program.functions.retain(|f| f.span.file != file);
                program.statics.retain(|s| s.span.file != file);
                program.consts.retain(|c| c.span.file != file);
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
                module: String::new(),
                generics: Vec::new(),
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
            let file = names
                .get(span.file)
                .filter(|n| !n.is_empty())
                .map_or_else(|| "<import>".into(), |n| n.replace('\\', "/"));
            // Files without an `import` of their own (the standard library)
            // are reported at the top of the document.
            let root = origin
                .get(span.file)
                .copied()
                .flatten()
                .unwrap_or(Span { line: 1, col: 1, file: 0 });
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
            let e = match TypeChecker::new().with_files(names.clone()).check_program(&mut attempt) {
                Ok(info) => {
                    if let Some(let_types) = let_types.as_deref_mut() {
                        for f in bodies(&attempt) {
                            collect_block(&f.body, &info, let_types);
                        }
                    }
                    break;
                }
                Err(e) => e,
            };
            errors.push(locate(e.span, e.message));

            // The function whose `fn` comes last before the error.
            let culprit = bodies(&program)
                .into_iter()
                .enumerate()
                .filter(|(_, f)| {
                    f.span.file == e.span.file && (f.span.line, f.span.col) <= (e.span.line, e.span.col)
                })
                .max_by_key(|(_, f)| (f.span.line, f.span.col))
                .map(|(i, _)| i);
            let Some(i) = culprit.filter(|&i| stubbed.insert(i)) else {
                break;
            };
            let body = &mut bodies_mut(&mut program)[i].body;
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
        for f in bodies_mut(program) {
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

/// Every function with a body: free functions, `impl` methods (generic or
/// for a trait) and default methods of traits.
fn bodies(p: &Program) -> Vec<&Function> {
    let mut out: Vec<&Function> = p.functions.iter().collect();
    out.extend(p.impls.iter().flat_map(|i| i.methods.iter()));
    out.extend(p.traits.iter().flat_map(|t| t.methods.iter().filter(|m| m.1).map(|m| &m.0)));
    out
}

fn bodies_mut(p: &mut Program) -> Vec<&mut Function> {
    let mut out: Vec<&mut Function> = p.functions.iter_mut().collect();
    out.extend(p.impls.iter_mut().flat_map(|i| i.methods.iter_mut()));
    out.extend(
        p.traits
            .iter_mut()
            .flat_map(|t| t.methods.iter_mut().filter(|m| m.1).map(|m| &mut m.0)),
    );
    out
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
                if let Some(guard) = &mut arm.guard {
                    walk_expr(guard, f);
                }
                walk_block(&mut arm.body, f);
            }
        }
        Stmt::IfLet {
            value,
            then_block,
            else_block,
            ..
        } => {
            walk_expr(value, f);
            walk_block(then_block, f);
            if let Some(e) = else_block {
                walk_block(e, f);
            }
        }
        Stmt::WhileLet { value, body, .. } => {
            walk_expr(value, f);
            walk_block(body, f);
        }
        Stmt::LetElse {
            value, else_block, ..
        } => {
            walk_expr(value, f);
            walk_block(else_block, f);
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
        | ExprKind::Try(a)
        | ExprKind::Unsize(a) => walk_expr(a, f),
        ExprKind::VariantLit { args, .. } => {
            for a in args {
                walk_expr(a, f);
            }
        }
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

fn collect_block(b: &Block, p: &ProgramInfo, out: &mut LetTypes) {
    for s in &b.stmts {
        collect_stmt(s, p, out);
    }
}

fn collect_stmt(s: &Stmt, p: &ProgramInfo, out: &mut LetTypes) {
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
        Stmt::IfLet {
            pat,
            then_block,
            else_block,
            ..
        } => {
            collect_pattern(pat, p, out);
            collect_block(then_block, p, out);
            if let Some(e) = else_block {
                collect_block(e, p, out);
            }
        }
        Stmt::WhileLet { pat, body, .. } => {
            collect_pattern(pat, p, out);
            collect_block(body, p, out);
        }
        Stmt::LetElse {
            pat, else_block, ..
        } => {
            collect_pattern(pat, p, out);
            collect_block(else_block, p, out);
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
                for pat in &arm.pats {
                    collect_pattern(pat, p, out);
                }
                collect_block(&arm.body, p, out);
            }
        }
        Stmt::Block(b) => collect_block(b, p, out),
        _ => {}
    }
}

/// Types of the names a pattern binds, keyed by where the pattern starts
/// (for `mut x`, the position of `mut`).
fn collect_pattern(pat: &Pattern, p: &ProgramInfo, out: &mut LetTypes) {
    match &pat.kind {
        PatKind::Bind { .. } | PatKind::Ident(_) | PatKind::MutBind(_) => {
            if let (0, Some(t)) = (pat.span.file, &pat.r#type) {
                out.insert((pat.span.line, pat.span.col), type_name(t, p));
            }
        }
        PatKind::Variant { args, .. } | PatKind::Ctor { args, .. } => {
            for a in args {
                collect_pattern(a, p, out);
            }
        }
        PatKind::Or(alts) => {
            for a in alts {
                collect_pattern(a, p, out);
            }
        }
        _ => {}
    }
}

pub fn type_name(t: &Type, p: &ProgramInfo) -> String {
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
        Type::Sum(id) => p.sums.get(*id).map_or("?".into(), |s| s.name.clone()),
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
