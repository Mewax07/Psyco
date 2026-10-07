use std::{fs, path::Path, process::Command};

use psycoc::{Target, compile_file};

fn qemu_available() -> bool {
    Path::new("/usr/share/OVMF/OVMF_CODE_4M.fd").exists()
        && Command::new("qemu-system-x86_64")
            .arg("--version")
            .output()
            .is_ok()
}

fn clean(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&n) = chars.peek() {
                    chars.next();
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else if c != '\r' {
            out.push(c);
        }
    }
    out
}

fn run_one(path: &Path, out_dir: &Path) -> Result<(), String> {
    let src = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut expected = Vec::new();
    let mut exit = 0;
    for line in src.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("//>") {
            expected.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        } else if let Some(rest) = line.strip_prefix("// exit:") {
            exit = rest.trim().parse().expect("bad '// exit:'");
        }
    }

    let binary = compile_file(path.to_str().unwrap(), Target::Uefi)?;
    let efi = out_dir
        .join(path.file_stem().unwrap())
        .with_extension("efi");
    fs::write(&efi, binary).map_err(|e| e.to_string())?;

    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/run-uefi.sh");
    let out = Command::new(script)
        .arg(&efi)
        .env("TIMEOUT", "90")
        .output()
        .map_err(|e| e.to_string())?;
    let serial = clean(&String::from_utf8_lossy(&out.stdout));

    let app_output = match serial.find("BdsDxe: starting") {
        Some(i) => serial[i..]
            .split_once('\n')
            .map_or("", |(_, rest)| rest)
            .to_string(),
        None => serial.clone(),
    };
    let lines: Vec<&str> = app_output.lines().collect();
    let code = out.status.code().unwrap_or(-1);

    let lines_ok =
        lines.len() >= expected.len() && lines.iter().zip(&expected).all(|(a, e)| a == e);
    if code != exit || !lines_ok {
        return Err(format!(
            "exit: expected {exit}, got {code}\n--- expected ---\n{}\n--- serial output ---\n{app_output}\n--- stderr ---\n{}",
            expected.join("\n"),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

#[test]
fn uefi_programs_boot_in_qemu() {
    if !qemu_available() {
        eprintln!("skipped: qemu-system-x86_64 or OVMF not installed");
        return;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/uefi");
    let out_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("uefi");
    fs::create_dir_all(&out_dir).unwrap();

    let mut files: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "psy"))
        .collect();
    files.sort();

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
            "{}/{} UEFI programs failed:\n\n{}",
            failures.len(),
            files.len(),
            failures.join("\n")
        );
    }
}
