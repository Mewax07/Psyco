use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use crate::*;

const STD: &[(&str, &str)] = &[
    ("std/prelude.psy", include_str!("../std/prelude.psy")),
    ("std/alloc.psy", include_str!("../std/alloc.psy")),
    (
        "std/alloc_linux.psy",
        include_str!("../std/alloc_linux.psy"),
    ),
];

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
    load_file(
        Path::new(path),
        None,
        None,
        &mut sources,
        &mut program,
        &mut seen,
    )?;
    for f in STD.iter() {
        load_file(
            Path::new(f.0),
            None,
            None,
            &mut sources,
            &mut program,
            &mut seen,
        )?;
    }
    Ok((program, sources))
}

pub fn compile_str(src: &str, target: Target) -> Result<Vec<u8>, String> {
    let mut paths = vec!["<input>".to_string()];
    paths.extend(STD.iter().map(|s| s.0.to_string()));
    let sources = Sources { paths };
    let mut program = parse(src, 0, &sources)?;
    if let Some((_, _, span)) = program.imports.first() {
        return Err(sources.describe(*span, "import", "imports need a file path"));
    }
    for (i, f) in STD.iter().enumerate() {
        let text = STD.iter().find(|(n, _)| n == &f.0).unwrap().1;
        program.merge(parse(text, i + 1, &sources)?);
    }
    compile_program(program, &sources, target)
}

pub fn prelude(file: usize) -> Result<Program, String> {
    let text = STD.iter().find(|(n, _)| *n == STD[0].0).unwrap().1;
    let sources = Sources {
        paths: vec![STD[0].0.into(); file + 1],
    };
    parse(text, file, &sources)
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
    let in_file = |span: &Span, f: &str| sources.paths.get(span.file).is_some_and(|p| p == f);
    let own_heap = program
        .functions
        .iter()
        .any(|f| f.name == "psy_alloc" && !in_file(&f.span, STD[2].0));
    if target != Target::Linux || own_heap {
        program.functions.retain(|f| !in_file(&f.span, STD[2].0));
        program.statics.retain(|s| !in_file(&s.span, STD[2].0));
        program.consts.retain(|c| !in_file(&c.span, STD[2].0));
    }
    let info = TypeChecker::new()
        .with_files(sources.paths.clone())
        .check_program(&mut program)
        .map_err(|e| sources.describe(e.span, "type", &e.message))?;
    Ok(Codegen::new(&info).generate(&program, target))
}

fn parse(src: &str, file: usize, sources: &Sources) -> Result<Program, String> {
    parse_in(src, file, sources, String::new())
}

fn parse_in(src: &str, file: usize, sources: &Sources, module: String) -> Result<Program, String> {
    let tokens = Lexer::with_file(src, file)
        .tokenize()
        .map_err(|e| sources.describe(e.span, "lexer", &e.message))?;
    Parser::new(tokens)
        .with_module(module)
        .parse_program()
        .map_err(|e| sources.describe(e.span, "parser", &e.message))
}

fn load_file(
    path: &Path,
    module: Option<String>,
    from: Option<(Span, &Sources)>,
    sources: &mut Sources,
    program: &mut Program,
    seen: &mut HashSet<(PathBuf, Option<String>)>,
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
    if !seen.insert((key, module.clone())) {
        return Ok(());
    }

    let file = sources.paths.len();
    sources.paths.push(display);
    let parsed = parse_in(&src, file, sources, module.unwrap_or_default())?;
    let imports = parsed.imports.clone();
    program.merge(parsed);

    let dir = path.parent().unwrap_or(Path::new("."));
    for (import, alias, span) in imports {
        let target = if import.starts_with("std/") {
            PathBuf::from(&import)
        } else {
            dir.join(&import)
        };
        let snapshot = Sources {
            paths: sources.paths.clone(),
        };
        load_file(
            &target,
            alias,
            Some((span, &snapshot)),
            sources,
            program,
            seen,
        )?;
    }
    Ok(())
}
