use std::{
    collections::{HashMap, HashSet},
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
};

use lsp_types::{Diagnostic, DiagnosticSeverity, Range};
use psycoc::{Abi, Block, Function, Lexer, Parser, Program, Span, Stmt, Type, TypeChecker};

use crate::analysis::Analysis;

pub struct CheckResult {
    pub diagnostics: Vec<Diagnostic>,
    pub let_types: HashMap<(usize, usize), String>,
}

pub struct Loader<'a> {
    pub read: &'a dyn Fn(&Path) -> Option<String>,
    pub resolve: &'a dyn Fn(Option<&Path>, &str) -> PathBuf,
    pub key: &'a dyn Fn(&Path) -> String,
}

pub fn check(an: &Analysis, path: Option<&Path>, loader: &Loader) -> CheckResult {
    let mut let_types = HashMap::new();
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| run(an, path, loader, &mut let_types)));
    let diagnostics = match outcome {
        Ok(Some((range, message))) => vec![Diagnostic {
            range,
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("psycoc".into()),
            message,
            ..Default::default()
        }],
        Ok(None) => Vec::new(),
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

fn run(
    an: &Analysis,
    path: Option<&Path>,
    loader: &Loader,
    let_types: &mut HashMap<(usize, usize), String>,
) -> Option<(Range, String)> {
    let at = |s: Span| an.word_range(s.line, s.col);

    let tokens = match Lexer::with_file(&an.text, 0).tokenize() {
        Ok(t) => t,
        Err(e) => return Some((at(e.span), e.message)),
    };
    let mut program = match Parser::new(tokens).parse_program() {
        Ok(p) => p,
        Err(e) => return Some((at(e.span), e.message)),
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
            return Some((at(root), format!("cannot read {display}")));
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
                return Some((at(root), format!("{display}:{}:{}: {message}", s.line, s.col)));
            }
        }
    }

    if !program.functions.iter().any(|f| f.name == "main") {
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

    match TypeChecker::new().check_program(&mut program) {
        Err(e) if e.span.file == 0 => Some((at(e.span), e.message)),
        Err(e) => {
            let file = paths
                .get(e.span.file)
                .cloned()
                .flatten()
                .map_or_else(|| "<import>".into(), |p| p.to_string_lossy().replace('\\', "/"));
            let root = origin.get(e.span.file).copied().flatten().unwrap_or(e.span);
            Some((
                at(root),
                format!("{file}:{}:{}: {}", e.span.line, e.span.col, e.message),
            ))
        }
        Ok(_) => {
            for f in &program.functions {
                collect_block(&f.body, &program, let_types);
            }
            None
        }
    }
}

fn collect_block(b: &Block, p: &Program, out: &mut HashMap<(usize, usize), String>) {
    for s in &b.stmts {
        collect_stmt(s, p, out);
    }
}

fn collect_stmt(s: &Stmt, p: &Program, out: &mut HashMap<(usize, usize), String>) {
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
        Stmt::While { body, .. } | Stmt::Loop { body, .. } | Stmt::For { body, .. } => {
            collect_block(body, p, out)
        }
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
