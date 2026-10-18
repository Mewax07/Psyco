#![cfg(target_os = "linux")]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

use psycoc::{Options, Target, compile_file_with};

const CONFIGS: [(&str, Options); 5] = [
    ("all", Options::ALL),
    ("none", Options::NONE),
    (
        "no inlining",
        Options {
            optimize: true,
            inline: false,
            regalloc: true,
        },
    ),
    (
        "stack only",
        Options {
            optimize: true,
            inline: true,
            regalloc: false,
        },
    ),
    (
        "registers, no optimization",
        Options {
            optimize: false,
            inline: false,
            regalloc: true,
        },
    ),
];

fn expected(src: &str) -> (String, i32) {
    let mut out = String::new();
    let mut exit = 0;
    for line in src.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("//>") {
            out.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            out.push('\n');
        } else if let Some(rest) = line.strip_prefix("// exit:") {
            exit = rest.trim().parse().unwrap();
        }
    }
    (out, exit)
}

fn run(exe: &Path) -> (String, i32) {
    for _ in 0..100 {
        match Command::new(exe).output() {
            Err(e) if e.raw_os_error() == Some(26) => std::thread::sleep(std::time::Duration::from_millis(5)),
            Ok(o) => return (String::from_utf8_lossy(&o.stdout).into(), o.status.code().unwrap_or(-1)),
            Err(e) => panic!("{e}"),
        }
    }
    panic!("text file busy")
}

fn files(dir: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "psy"))
        .collect();
    v.sort();
    v
}

#[test]
fn every_configuration_gives_the_same_behaviour() {
    let out_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("backend");
    fs::create_dir_all(&out_dir).unwrap();
    let mut failures = Vec::new();
    for path in files("tests/programs").into_iter().chain(files("bench")) {
        let src = fs::read_to_string(&path).unwrap();
        let (want, want_exit) = expected(&src);
        for (k, (name, opts)) in CONFIGS.iter().enumerate() {
            let (bin, _) = match compile_file_with(path.to_str().unwrap(), Target::Linux, *opts) {
                Ok(b) => b,
                Err(e) => {
                    failures.push(format!("{} [{name}]: {e}", path.display()));
                    continue;
                }
            };
            let exe = out_dir.join(format!("{}-{k}", path.file_stem().unwrap().to_string_lossy()));
            fs::write(&exe, bin).unwrap();
            fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
            let (got, code) = run(&exe);
            let ok = code == want_exit && if want_exit == 0 { got == want } else { got.starts_with(&want) };
            if !ok {
                failures.push(format!(
                    "{} [{name}]: exit {code} (want {want_exit})\n--- want ---\n{want}--- got ---\n{got}",
                    path.display()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn optimizations_shrink_the_benchmarks() {
    for path in files("bench") {
        let p = path.to_str().unwrap();
        let (_, all) = compile_file_with(p, Target::Linux, Options::ALL).unwrap();
        let (_, none) = compile_file_with(p, Target::Linux, Options::NONE).unwrap();
        assert!(all < none, "{p}: {all} bytes optimized, {none} without");
    }
}
