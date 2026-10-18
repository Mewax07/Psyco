use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

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

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("\\x") {
        out.push_str(&rest[..i]);
        match rest.get(i + 2..i + 4).and_then(|h| u8::from_str_radix(h, 16).ok()) {
            Some(b) => {
                out.push(b as char);
                rest = &rest[i + 4..];
            }
            None => {
                out.push_str("\\x");
                rest = &rest[i + 2..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[derive(Default)]
struct Spec {
    expected: Vec<String>,
    exit: i32,
    input: Vec<String>,
    disk: Option<bool>,
    ordered: bool,
    esp: Vec<(String, String)>,
}

fn parse_spec(src: &str) -> Spec {
    let mut spec = Spec::default();
    for line in src.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("//>") {
            spec.expected.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        } else if let Some(rest) = line.strip_prefix("// exit:") {
            spec.exit = rest.trim().parse().expect("bad '// exit:'");
        } else if let Some(rest) = line.strip_prefix("// input:") {
            spec.input.push(unescape(rest.trim()));
        } else if let Some(rest) = line.strip_prefix("// disk:") {
            let rest = rest.trim();
            assert!(rest.starts_with("fat32"), "only '// disk: fat32' is supported");
            spec.disk = Some(rest.contains("+programs"));
        } else if let Some(rest) = line.strip_prefix("// esp:") {
            let (name, text) = rest.trim().split_once(' ').expect("'// esp: NAME TEXT'");
            spec.esp.push((name.to_string(), text.replace("\\n", "\n")));
        } else if line.starts_with("// match: ordered") {
            spec.ordered = true;
        }
    }
    spec
}

fn have(tool: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool}"))
        .output()
        .is_ok_and(|o| o.status.success())
}

fn make_disk(dir: &Path, programs: bool) -> Result<Option<PathBuf>, String> {
    if !have("mkfs.fat") || !have("mcopy") || !have("mmd") {
        return Ok(None);
    }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let img = dir.join("disk.img");
    fs::write(&img, vec![0u8; 40 << 20]).map_err(|e| e.to_string())?;
    let run = |cmd: &mut Command| -> Result<(), String> {
        let out = cmd.output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).to_string())
        }
    };
    run(Command::new("mkfs.fat").arg("-F").arg("32").arg(&img))?;
    let readme = dir.join("readme.txt");
    fs::write(&readme, "hello from the disk\n").map_err(|e| e.to_string())?;
    run(Command::new("mcopy").arg("-i").arg(&img).arg(&readme).arg("::/"))?;
    if programs {
        run(Command::new("mmd").arg("-i").arg(&img).arg("::/bin"))?;
        let user = Path::new(env!("CARGO_MANIFEST_DIR")).join("../psycos/user");
        let mut entries: Vec<_> = fs::read_dir(&user)
            .map_err(|e| e.to_string())?
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "psy"))
            .collect();
        entries.sort();
        for p in entries {
            let exe = dir.join(p.file_stem().unwrap());
            let bin = compile_file(p.to_str().unwrap(), Target::Linux)?;
            fs::write(&exe, bin).map_err(|e| e.to_string())?;
            run(Command::new("mcopy").arg("-i").arg(&img).arg(&exe).arg("::/bin/"))?;
        }
    }
    Ok(Some(img))
}

fn run_qemu(efi: &Path, disk: Option<&Path>, input: &[String], esp: Option<&Path>) -> Result<(i32, String, String), String> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/run-uefi.sh");
    let mut cmd = Command::new(script);
    cmd.arg(efi).env("TIMEOUT", "90");
    if let Some(dir) = esp {
        cmd.env("ESP_EXTRA", dir);
    }
    if let Some(d) = disk {
        cmd.arg("-drive")
            .arg(format!("file={},if=virtio,format=raw", d.display()));
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let output = Arc::new(Mutex::new(Vec::new()));
    let mut stdout = child.stdout.take().unwrap();
    let sink = output.clone();
    let reader = thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = stdout.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock().unwrap().extend_from_slice(&buf[..n]);
        }
    });
    let mut stdin = child.stdin.take().unwrap();
    if !input.is_empty() {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(60) {
            if String::from_utf8_lossy(&output.lock().unwrap()).contains("boot ok") {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        thread::sleep(Duration::from_millis(1500));
        for line in input {
            let _ = stdin.write_all(format!("{line}\r").as_bytes());
            let _ = stdin.flush();
            thread::sleep(Duration::from_millis(1500));
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    drop(stdin);
    reader.join().unwrap();
    let serial = String::from_utf8_lossy(&output.lock().unwrap()).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    Ok((out.status.code().unwrap_or(-1), serial, stderr))
}

fn run_one(path: &Path, out_dir: &Path) -> Result<(), String> {
    let src = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let spec = parse_spec(&src);
    let expected = &spec.expected;
    let exit = spec.exit;

    let binary = compile_file(path.to_str().unwrap(), Target::Uefi)?;
    let stem = path.file_stem().unwrap();
    let efi = out_dir.join(stem).with_extension("efi");
    fs::write(&efi, binary).map_err(|e| e.to_string())?;

    let disk = match spec.disk {
        Some(programs) => match make_disk(&out_dir.join(stem), programs)? {
            Some(d) => Some(d),
            None => {
                eprintln!("skipped {}: mkfs.fat / mtools not installed", path.display());
                return Ok(());
            }
        },
        None => None,
    };
    let esp = if spec.esp.is_empty() {
        None
    } else {
        let dir = out_dir.join(format!("{}-esp", stem.to_string_lossy()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        for (name, text) in &spec.esp {
            fs::write(dir.join(name), text).map_err(|e| e.to_string())?;
        }
        Some(dir)
    };
    let (code, raw, stderr) = run_qemu(&efi, disk.as_deref(), &spec.input, esp.as_deref())?;
    let serial = clean(&raw);

    let app_output = match serial.find("BdsDxe: starting") {
        Some(i) => serial[i..]
            .split_once('\n')
            .map_or("", |(_, rest)| rest)
            .to_string(),
        None => serial.clone(),
    };
    let lines: Vec<&str> = app_output.lines().collect();

    let lines_ok = if spec.ordered {
        let mut it = lines.iter();
        expected.iter().all(|e| it.any(|l| l.contains(e.as_str())))
    } else {
        lines.len() >= expected.len() && lines.iter().zip(expected).all(|(a, e)| a == e)
    };
    if code != exit || !lines_ok {
        return Err(format!(
            "exit: expected {exit}, got {code}\n--- expected ---\n{}\n--- serial output ---\n{app_output}\n--- stderr ---\n{stderr}",
            expected.join("\n"),
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
        .filter(|p| std::env::var("PSY_UEFI").map_or(true, |f| p.to_string_lossy().contains(&f)))
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
