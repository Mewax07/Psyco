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
        Ok(errors) => {
            let mut seen = HashSet::new();
            errors
                .into_iter()
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

fn run(
    an: &Analysis,
    path: Option<&Path>,
    loader: &Loader,
    let_types: &mut HashMap<(usize, usize), String>,
) -> Vec<(Range, String)> {
    let at = |s: Span| an.word_range(s.line, s.col);

    let tokens = match Lexer::with_file(&an.text, 0).tokenize() {
        Ok(t) => t,
        Err(e) => return vec![(at(e.span), e.message)],
    };
    let mut program = match Parser::new(tokens).parse_program() {
        Ok(p) => p,
        Err(e) => return vec![(at(e.span), e.message)],
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
            return vec![(at(root), format!("cannot read {display}"))];
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
                return vec![(at(root), format!("{display}:{}:{}: {message}", s.line, s.col))];
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

    // The checker stops at the first error. To report the others and still
    // know the types everywhere else, replace the body of the function that
    // failed with `loop {}` and check again.
    let mut errors = Vec::new();
    let mut stubbed = HashSet::new();
    loop {
        let mut attempt = program.clone();
        let e = match TypeChecker::new().check_program(&mut attempt) {
            Ok(_) => {
                for f in &attempt.functions {
                    collect_block(&f.body, &attempt, let_types);
                }
                break;
            }
            Err(e) => e,
        };
        errors.push(if e.span.file == 0 {
            (at(e.span), e.message)
        } else {
            let file = paths
                .get(e.span.file)
                .cloned()
                .flatten()
                .map_or_else(|| "<import>".into(), |p| p.to_string_lossy().replace('\\', "/"));
            let root = origin.get(e.span.file).copied().flatten().unwrap_or(e.span);
            (
                at(root),
                format!("{file}:{}:{}: {}", e.span.line, e.span.col, e.message),
            )
        });

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
