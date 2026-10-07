#![cfg(target_os = "linux")]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

use psycoc::{Target, compile_str};

fn compile(src: &str) -> Result<Vec<u8>, String> {
    compile_str(src, Target::Linux)
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

fn run_one(path: &Path, out_dir: &Path) -> Result<(), String> {
    let src = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let expect = parse_expect(&src);
    let binary = compile(&src)?;

    let exe = out_dir.join(path.file_stem().unwrap());
    fs::write(&exe, &binary).map_err(|e| e.to_string())?;
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;

    let out = run_exe(&exe)?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let Some(code) = out.status.code() else {
        return Err(format!(
            "killed by signal: {:?}\nstdout:\n{stdout}",
            out.status
        ));
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

fn run_exe(exe: &Path) -> Result<std::process::Output, String> {
    const ETXTBSY: i32 = 26;
    for _ in 0..100 {
        match Command::new(exe).output() {
            Err(e) if e.raw_os_error() == Some(ETXTBSY) => {
                std::thread::sleep(std::time::Duration::from_millis(5))
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

#[test]
fn programs_behave_as_expected() {
    let out_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("e2e");
    fs::create_dir_all(&out_dir).unwrap();

    let files = programs();
    assert!(!files.is_empty(), "no test programs found");

    let failures: Vec<String> = files
        .iter()
        .filter_map(|p| {
            run_one(p, &out_dir)
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
