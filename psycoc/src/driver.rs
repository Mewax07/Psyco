use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use crate::*;

const STD: &[(&str, &str)] = &[];

pub struct Sources {
    pub paths: Vec<String>,
}

impl Sources {
    pub fn describe(&self, span: Span, stage: &str, message: &str) -> String {
        let path = self
            .paths
            .get(span.file)
            .map_or("<unknown>", |p| p.as_str());
        format!(
            "{path}:{}:{}: {stage} error: {message}",
            span.line, span.col
        )
    }
}

pub fn load(path: &str) -> Result<(Program, Sources), String> {
    let mut sources = Sources { paths: Vec::new() };
    let mut program = Program::default();
    let mut seen = HashSet::new();
    load_file(Path::new(path), None, &mut sources, &mut program, &mut seen)?;
    Ok((program, sources))
}

pub fn compile_str(src: &str, target: Target) -> Result<Vec<u8>, String> {
    let sources = Sources {
        paths: vec!["<input>".into()],
    };
    let program = parse(src, 0, &sources)?;
    if let Some((_, span)) = program.imports.first() {
        return Err(sources.describe(*span, "import", "imports need a file path"));
    }
    compile_program(program, &sources, target)
}

pub fn compile_file(path: &str, target: Target) -> Result<Vec<u8>, String> {
    let (program, sources) = load(path)?;
    compile_program(program, &sources, target)
}

pub fn compile_program(
    mut program: Program,
    sources: &Sources,
    target: Target,
) -> Result<Vec<u8>, String> {
    let info = TypeChecker::new()
        .check_program(&mut program)
        .map_err(|e| sources.describe(e.span, "type", &e.message))?;
    Ok(Codegen::new(&info).generate(&program, target))
}

fn parse(src: &str, file: usize, sources: &Sources) -> Result<Program, String> {
    let tokens = Lexer::with_file(src, file)
        .tokenize()
        .map_err(|e| sources.describe(e.span, "lexer", &e.message))?;
    Parser::new(tokens)
        .parse_program()
        .map_err(|e| sources.describe(e.span, "parser", &e.message))
}

fn load_file(
    path: &Path,
    from: Option<(Span, &Sources)>,
    sources: &mut Sources,
    program: &mut Program,
    seen: &mut HashSet<PathBuf>,
) -> Result<(), String> {
    let display = path.to_string_lossy().replace('\\', "/");
    let (key, src) = if let Some((_, text)) = STD.iter().find(|(name, _)| display.ends_with(name)) {
        (PathBuf::from(&display), text.to_string())
    } else {
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let src = fs::read_to_string(path).map_err(|e| match from {
            Some((span, s)) => s.describe(span, "import", &format!("cannot read {display}: {e}")),
            None => format!("cannot read {display}: {e}"),
        })?;
        (canonical, src)
    };
    if !seen.insert(key) {
        return Ok(());
    }

    let file = sources.paths.len();
    sources.paths.push(display);
    let parsed = parse(&src, file, sources)?;
    let imports = parsed.imports.clone();
    program.merge(parsed);

    let dir = path.parent().unwrap_or(Path::new("."));
    for (import, span) in imports {
        let target = if import.starts_with("std/") {
            PathBuf::from(&import)
        } else {
            dir.join(&import)
        };
        let snapshot = Sources {
            paths: sources.paths.clone(),
        };
        load_file(&target, Some((span, &snapshot)), sources, program, seen)?;
    }
    Ok(())
}
