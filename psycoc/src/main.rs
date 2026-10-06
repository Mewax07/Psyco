use std::{env, fs, process};

use psycoc::{
    Target,
    codegen::Codegen,
    lexer::{Lexer, Span},
    parser::Parser,
    typeck::TypeChecker,
};

fn fail(path: &str, stage: &str, span: Span, message: &str) -> ! {
    eprintln!(
        "{path}:{}:{}: {stage} error: {message}",
        span.line, span.col
    );
    process::exit(1);
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();

    let Some(path) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("usage: psycoc [--nasm] <file>");
        process::exit(1);
    };
    let path = path.clone();

    let src = fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("cannot read {path}: {e}");
        process::exit(1);
    });

    let tokens = Lexer::new(&src)
        .tokenize()
        .unwrap_or_else(|e| fail(&path, "lexer", e.span, &e.message));

    let mut program = Parser::new(tokens)
        .parse_program()
        .unwrap_or_else(|e| fail(&path, "parser", e.span, &e.message));

    if let Err(e) = TypeChecker::new().check_program(&mut program) {
        fail(&path, "type", e.span, &e.message);
    }

    let uefi = args.iter().any(|a| a == "--uefi");
    let (target, output) = if uefi {
        (Target::Uefi, "out.efi")
    } else {
        (Target::Linux, "out")
    };
    let binary = Codegen::new().generate(&program, target);
    fs::write(output, binary).expect("cannot write output");

    #[cfg(unix)]
    if target == Target::Linux {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = fs::set_permissions(output, fs::Permissions::from_mode(0o755)) {
            eprintln!("warning: cannot make {output} executable: {e}");
        }
    }
}
