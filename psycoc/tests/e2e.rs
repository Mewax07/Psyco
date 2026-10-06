use std::{
    fs, path::{Path, PathBuf}, process::{Command, Output}, thread::sleep, time::Duration,
};

use psycoc::{
    Target,
    ast::{Block, Expr, ExprKind, Program, Stmt, UnaryOp},
    codegen::Codegen,
    lexer::Lexer,
    parser::Parser,
    typeck::TypeChecker,
};

type Transform = fn(&mut Program);

fn compile(src: &str, transform: Transform) -> Result<Vec<u8>, String> {
    let tokens = Lexer::new(src)
        .tokenize()
        .map_err(|e| format!("lexer {}:{}: {}", e.span.line, e.span.col, e.message))?;
    let mut program = Parser::new(tokens)
        .parse_program()
        .map_err(|e| format!("parser {}:{}: {}", e.span.line, e.span.col, e.message))?;
    TypeChecker::new()
        .check_program(&mut program)
        .map_err(|e| format!("type {}:{}: {}", e.span.line, e.span.col, e.message))?;
    transform(&mut program);
    Ok(Codegen::new().generate(&program, Target::Linux))
}

fn identity(_: &mut Program) {}

fn fold_negative_literals(program: &mut Program) {
    fn expr(e: &mut Expr) {
        match &mut e.kind {
            ExprKind::Unary { op, operand } => {
                expr(operand);
                if let (UnaryOp::Neg, ExprKind::Int(n)) = (*op, &operand.kind) {
                    e.kind = ExprKind::Int(-*n);
                }
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                expr(lhs);
                expr(rhs);
            }
            ExprKind::Call { args, .. } => args.iter_mut().for_each(expr),
            ExprKind::Int(_) | ExprKind::Bool(_) | ExprKind::Str(_) | ExprKind::Var(_) => {}
        }
    }
    fn block(b: &mut Block) {
        for s in &mut b.stmts {
            match s {
                Stmt::Let { value, .. } | Stmt::Assign { value, .. } | Stmt::Expr(value) => {
                    expr(value)
                }
                Stmt::Return { value, .. } => value.iter_mut().for_each(expr),
                Stmt::If { cond, then_block, else_block, .. } => {
                    expr(cond);
                    block(then_block);
                    else_block.iter_mut().for_each(block);
                }
                Stmt::While { cond, body, .. } => {
                    expr(cond);
                    block(body);
                }
            }
        }
    }
    program.functions.iter_mut().for_each(|f| block(&mut f.body));
}

struct Expect {
    stdout: String,
    exit: i32,
}

fn parse_expect(src: &str) -> Expect {
    let mut stdout = String::new();
    let mut exit = 0;
    for line in src.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("//>") {
            stdout.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            stdout.push('\n');
        } else if let Some(rest) = line.strip_prefix("// exit:") {
            exit = rest.trim().parse().expect("bad '// exit:' value");
        }
    }
    Expect { stdout, exit }
}

fn run_one(path: &Path, out_dir: &Path, transform: Transform) -> Result<(), String> {
    let src = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let expect = parse_expect(&src);
    let binary = compile(&src, transform)?;

    let exe = out_dir.join(path.file_stem().unwrap());
    fs::write(&exe, &binary).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;

    let out = run_exe(&exe)?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let Some(code) = out.status.code() else {
        return Err(format!("killed by signal: {:?}\nstdout:\n{stdout}", out.status));
    };

    let stdout_ok = if expect.exit == 0 {
        stdout == expect.stdout
    } else {
        stdout.starts_with(&expect.stdout)
    };
    if code != expect.exit || !stdout_ok {
        return Err(format!(
            "exit: expected {}, got {code}\n--- expected stdout ---\n{}--- actual stdout ---\n{stdout}",
            expect.exit, expect.stdout
        ));
    }
    Ok(())
}

fn run_exe(exe: &Path) -> Result<Output, String> {
    const ETXTBSY: i32 = 26;
    for _ in 0..100 {
        match Command::new(exe).output() {
            Err(e) if e.raw_os_error() == Some(ETXTBSY) => {
                sleep(Duration::from_millis(5))
            }
            other => return other.map_err(|e| e.to_string()),
        }
    }
    Err("exec: text file busy (gave up)".into())
}

fn programs() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/programs");
    let mut files: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "psy"))
        .collect();
    files.sort();
    files
}

#[cfg(target_os = "linux")]
#[test]
fn programs_behave_as_expected() {
    run_all("plain", identity);
}

#[cfg(target_os = "linux")]
#[test]
fn programs_with_negative_literals_folded() {
    run_all("negfold", fold_negative_literals);
}

fn run_all(name: &str, transform: Transform) {
    let out_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("e2e").join(name);
    fs::create_dir_all(&out_dir).unwrap();

    let files = programs();
    assert!(!files.is_empty(), "no test programs found");

    let failures: Vec<String> = files
        .iter()
        .filter_map(|p| {
            run_one(p, &out_dir, transform)
                .err()
                .map(|e| format!("=== {} ===\n{e}", p.file_name().unwrap().to_string_lossy()))
        })
        .collect();

    if !failures.is_empty() {
        panic!(
            "{}/{} programs failed:\n\n{}",
            failures.len(),
            files.len(),
            failures.join("\n")
        );
    }
}
