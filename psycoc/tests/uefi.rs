use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
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

#[derive(Clone, Copy, PartialEq)]
enum Bus {
    Virtio,
    Ahci,
    Nvme,
}

#[derive(Clone, Copy)]
struct DiskSpec {
    programs: bool,
    gpt: bool,
    bus: Bus,
    sector: u64,
}

#[derive(Default)]
struct Spec {
    expected: Vec<String>,
    exit: i32,
    input: Vec<String>,
    disk: Option<DiskSpec>,
    ordered: bool,
    esp: Vec<(String, String)>,
    qemu: Vec<String>,
    boot: Option<String>,
    audio: bool,
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
            let words: Vec<&str> = rest.split_whitespace().collect();
            let has = |w: &str| words.contains(&w);
            spec.disk = Some(DiskSpec {
                programs: has("+programs"),
                gpt: has("gpt"),
                bus: if has("ahci") {
                    Bus::Ahci
                } else if has("nvme") {
                    Bus::Nvme
                } else {
                    Bus::Virtio
                },
                sector: if has("4k") { 4096 } else { 512 },
            });
        } else if let Some(rest) = line.strip_prefix("// esp:") {
            let (name, text) = rest.trim().split_once(' ').expect("'// esp: NAME TEXT'");
            spec.esp.push((name.to_string(), text.replace("\\n", "\n")));
        } else if line.starts_with("// audio: wav") {
            spec.audio = true;
        } else if let Some(rest) = line.strip_prefix("// boot:") {
            spec.boot = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("// qemu:") {
            spec.qemu.extend(rest.split_whitespace().map(String::from));
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

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

const ESP_GUID: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];

fn gpt_header(ss: u64, total: u64, current: u64, backup: u64, entries_lba: u64, entries_crc: u32) -> Vec<u8> {
    let entry_sectors = 128 * 128 / ss;
    let mut h = vec![0u8; ss as usize];
    h[0..8].copy_from_slice(b"EFI PART");
    h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
    h[12..16].copy_from_slice(&92u32.to_le_bytes());
    h[24..32].copy_from_slice(&current.to_le_bytes());
    h[32..40].copy_from_slice(&backup.to_le_bytes());
    h[40..48].copy_from_slice(&(2 + entry_sectors).to_le_bytes());
    h[48..56].copy_from_slice(&(total - 2 - entry_sectors).to_le_bytes());
    h[56..72].copy_from_slice(b"psyos-test-disk!");
    h[72..80].copy_from_slice(&entries_lba.to_le_bytes());
    h[80..84].copy_from_slice(&128u32.to_le_bytes());
    h[84..88].copy_from_slice(&128u32.to_le_bytes());
    h[88..92].copy_from_slice(&entries_crc.to_le_bytes());
    let crc = crc32(&h[0..92]);
    h[16..20].copy_from_slice(&crc.to_le_bytes());
    h
}

fn write_gpt(img: &Path, ss: u64, total: u64, start: u64, end: u64) -> Result<(), String> {
    use std::os::unix::fs::FileExt;
    let f = fs::OpenOptions::new().write(true).open(img).map_err(|e| e.to_string())?;
    let at = |lba: u64| lba * ss;
    let mut mbr = vec![0u8; 512];
    mbr[446 + 4] = 0xEE;
    mbr[446 + 8..446 + 12].copy_from_slice(&1u32.to_le_bytes());
    mbr[446 + 12..446 + 16].copy_from_slice(&((total - 1).min(0xFFFF_FFFF) as u32).to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    f.write_at(&mbr, 0).map_err(|e| e.to_string())?;
    let mut entries = vec![0u8; 128 * 128];
    entries[0..16].copy_from_slice(&ESP_GUID);
    entries[16..32].copy_from_slice(b"psyos-partition1");
    entries[32..40].copy_from_slice(&start.to_le_bytes());
    entries[40..48].copy_from_slice(&end.to_le_bytes());
    for (i, c) in "PSYOS".encode_utf16().enumerate() {
        entries[56 + 2 * i..58 + 2 * i].copy_from_slice(&c.to_le_bytes());
    }
    let ecrc = crc32(&entries);
    let entry_sectors = entries.len() as u64 / ss;
    let backup_entries = total - 1 - entry_sectors;
    f.write_at(&gpt_header(ss, total, 1, total - 1, 2, ecrc), at(1)).map_err(|e| e.to_string())?;
    f.write_at(&entries, at(2)).map_err(|e| e.to_string())?;
    f.write_at(&entries, at(backup_entries)).map_err(|e| e.to_string())?;
    f.write_at(&gpt_header(ss, total, total - 1, 1, backup_entries, ecrc), at(total - 1))
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn make_disk(dir: &Path, d: DiskSpec) -> Result<Option<PathBuf>, String> {
    if !have("mkfs.fat") || !have("mcopy") || !have("mmd") {
        return Ok(None);
    }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let img = dir.join("disk.img");
    let size: u64 = if d.sector == 4096 { 320 << 20 } else { 40 << 20 };
    let _ = fs::remove_file(&img);
    fs::File::create(&img)
        .and_then(|f| f.set_len(size))
        .map_err(|e| e.to_string())?;
    let run = |cmd: &mut Command| -> Result<(), String> {
        let out = cmd.output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).to_string())
        }
    };
    let ss = d.sector;
    let total = size / ss;
    let mut target = img.to_string_lossy().to_string();
    let mut mkfs = Command::new("mkfs.fat");
    mkfs.arg("-F").arg("32").arg("-S").arg(ss.to_string());
    if d.gpt {
        let start = (1 << 20) / ss;
        let end = total - (1 << 20) / ss - 1;
        write_gpt(&img, ss, total, start, end)?;
        mkfs.arg("-n").arg("PSYOS").arg(format!("--offset={start}"));
        mkfs.arg(&img).arg(((end - start + 1) * ss / 1024).to_string());
        target = format!("{}@@{}", img.display(), start * ss);
    } else {
        mkfs.arg(&img);
    }
    run(&mut mkfs)?;
    let img_arg = target;
    let readme = dir.join("readme.txt");
    fs::write(&readme, "hello from the disk\n").map_err(|e| e.to_string())?;
    run(Command::new("mcopy").arg("-i").arg(&img_arg).arg(&readme).arg("::/"))?;
    if d.programs {
        run(Command::new("mmd").arg("-i").arg(&img_arg).arg("::/bin"))?;
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
            run(Command::new("mcopy").arg("-i").arg(&img_arg).arg(&exe).arg("::/bin/"))?;
        }
    }
    Ok(Some(img))
}

struct Qmp {
    out: UnixStream,
    lines: BufReader<UnixStream>,
}

impl Qmp {
    fn connect(path: &Path) -> Result<Qmp, String> {
        let start = Instant::now();
        let stream = loop {
            match UnixStream::connect(path) {
                Ok(s) => break s,
                Err(e) if start.elapsed() > Duration::from_secs(10) => return Err(format!("qmp: {e}")),
                Err(_) => thread::sleep(Duration::from_millis(50)),
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
        let lines = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        let mut q = Qmp { out: stream, lines };
        q.reply()?;
        q.execute(r#"{"execute":"qmp_capabilities"}"#)?;
        Ok(q)
    }

    fn reply(&mut self) -> Result<String, String> {
        loop {
            let mut line = String::new();
            if self.lines.read_line(&mut line).map_err(|e| format!("qmp: {e}"))? == 0 {
                return Err("qmp: connection closed".into());
            }
            if line.contains("\"QMP\"") || line.contains("\"return\"") {
                return Ok(line);
            }
            if line.contains("\"error\"") {
                return Err(format!("qmp: {line}"));
            }
        }
    }

    fn execute(&mut self, json: &str) -> Result<String, String> {
        self.out.write_all(json.as_bytes()).map_err(|e| e.to_string())?;
        self.out.write_all(b"\n").map_err(|e| e.to_string())?;
        self.reply()
    }

    fn events(&mut self, events: &[String]) -> Result<(), String> {
        self.execute(&format!(
            r#"{{"execute":"input-send-event","arguments":{{"events":[{}]}}}}"#,
            events.join(",")
        ))
        .map(|_| ())
    }
}

fn rel(axis: &str, v: i64) -> String {
    format!(r#"{{"type":"rel","data":{{"axis":"{axis}","value":{v}}}}}"#)
}

fn abs(axis: &str, v: i64) -> String {
    format!(r#"{{"type":"abs","data":{{"axis":"{axis}","value":{v}}}}}"#)
}

fn btn(button: &str, down: bool) -> String {
    format!(r#"{{"type":"btn","data":{{"button":"{button}","down":{down}}}}}"#)
}

fn number(words: &[&str], i: usize, cmd: &str) -> Result<i64, String> {
    words
        .get(i)
        .and_then(|w| w.parse().ok())
        .ok_or_else(|| format!("'// input: {cmd}': number expected"))
}

fn pointer_command(q: &mut Qmp, cmd: &str) -> Result<(), String> {
    let words: Vec<&str> = cmd.split_whitespace().collect();
    let pause = Duration::from_millis(150);
    match words.first().copied() {
        Some("@mouse") => q.events(&[rel("x", number(&words, 1, cmd)?), rel("y", number(&words, 2, cmd)?)]),
        Some("@abs") => q.events(&[abs("x", number(&words, 1, cmd)?), abs("y", number(&words, 2, cmd)?)]),
        Some("@press") => q.events(&[btn(words.get(1).unwrap_or(&"left"), true)]),
        Some("@release") => q.events(&[btn(words.get(1).unwrap_or(&"left"), false)]),
        Some("@click") | Some("@dblclick") => {
            let b = words.get(1).unwrap_or(&"left");
            let times = if words[0] == "@dblclick" { 2 } else { 1 };
            for _ in 0..times {
                q.events(&[btn(b, true)])?;
                thread::sleep(pause);
                q.events(&[btn(b, false)])?;
                thread::sleep(pause);
            }
            Ok(())
        }
        Some("@wheel") => {
            let n = number(&words, 1, cmd)?;
            let b = if n > 0 { "wheel-up" } else { "wheel-down" };
            for _ in 0..n.abs() {
                q.events(&[btn(b, true)])?;
                q.events(&[btn(b, false)])?;
                thread::sleep(Duration::from_millis(30));
            }
            Ok(())
        }
        Some("@drag") => {
            let dx = number(&words, 1, cmd)?;
            let dy = number(&words, 2, cmd)?;
            let steps = 4;
            q.events(&[btn("left", true)])?;
            thread::sleep(pause);
            for k in 0..steps {
                let part = |d: i64| d * (k + 1) / steps - d * k / steps;
                q.events(&[rel("x", part(dx)), rel("y", part(dy))])?;
                thread::sleep(pause);
            }
            q.events(&[btn("left", false)])
        }
        Some("@sendkey") => {
            for key in &words[1..] {
                q.execute(&format!(
                    r#"{{"execute":"human-monitor-command","arguments":{{"command-line":"sendkey {key}"}}}}"#
                ))?;
                thread::sleep(pause);
            }
            Ok(())
        }
        Some("@wait") => {
            thread::sleep(Duration::from_millis(number(&words, 1, cmd)? as u64));
            Ok(())
        }
        _ => Err(format!("unknown pointer command '{cmd}'")),
    }
}

fn wav_stats(path: &Path) -> Result<(u64, u64), String> {
    let data = fs::read(path).map_err(|e| format!("audio: {}: {e}", path.display()))?;
    if data.len() < 44 || &data[0..4] != b"RIFF" {
        return Err("audio: not a WAV file".into());
    }
    let mut loud = 0u64;
    let mut crossings = 0u64;
    let mut last = 0i16;
    for frame in data[44..].chunks_exact(4) {
        let left = i16::from_le_bytes([frame[0], frame[1]]);
        if left.unsigned_abs() > 1000 {
            loud += 1;
            if (left > 0) != (last > 0) {
                crossings += 1;
            }
            last = left;
        }
    }
    Ok((loud, crossings))
}

fn make_usb(efi: &Path, img: &Path, cfg: &str) -> Result<(), String> {
    let tool = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/make-usb.py");
    let out = Command::new("python3")
        .arg(tool)
        .arg("--kernel")
        .arg(efi)
        .arg("--cfg")
        .arg(cfg)
        .arg("--out")
        .arg(img)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("make-usb: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(())
}

fn run_qemu(efi: &Path, disk: Option<(&Path, DiskSpec)>, input: &[String], esp: Option<&Path>, extra: &[String], qmp: &Path, boot: Option<(&str, &Path)>) -> Result<(i32, String, String), String> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/run-uefi.sh");
    let mut cmd = Command::new(script);
    cmd.arg(efi).env("TIMEOUT", "90");
    if let Some((medium, img)) = boot {
        cmd.env("NO_ESP", "1");
        cmd.arg("-drive").arg(format!("if=none,id=stick,format=raw,file={}", img.display()));
        if medium == "usb" {
            cmd.arg("-device").arg("qemu-xhci,id=bootxhci");
            cmd.arg("-device").arg("usb-storage,bus=bootxhci.0,drive=stick,bootindex=0");
        } else {
            cmd.arg("-device").arg("ide-hd,drive=stick,bus=ide.0,bootindex=0");
        }
    }
    let pointer = input.iter().any(|l| l.starts_with('@'));
    if pointer {
        let _ = fs::remove_file(qmp);
        cmd.arg("-qmp").arg(format!("unix:{},server=on,wait=off", qmp.display()));
    }
    cmd.args(extra);
    if let Some(dir) = esp {
        cmd.env("ESP_EXTRA", dir);
    }
    if let Some((d, spec)) = disk {
        match spec.bus {
            Bus::Virtio => {
                cmd.arg("-drive").arg(format!("file={},if=virtio,format=raw", d.display()));
            }
            Bus::Ahci => {
                cmd.arg("-drive").arg(format!("file={},if=none,id=disk0,format=raw", d.display()));
                cmd.arg("-device").arg("ide-hd,drive=disk0,bus=ide.1");
            }
            Bus::Nvme => {
                cmd.arg("-drive").arg(format!("file={},if=none,id=disk0,format=raw", d.display()));
                cmd.arg("-device").arg(format!(
                    "nvme,serial=psyos,drive=disk0,logical_block_size={0},physical_block_size={0}",
                    spec.sector
                ));
            }
        }
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
        let mut qmp_link = if pointer { Some(Qmp::connect(qmp)?) } else { None };
        for line in input {
            if line.starts_with('@') {
                if let Some(q) = qmp_link.as_mut() {
                    pointer_command(q, line)?;
                }
                thread::sleep(Duration::from_millis(400));
                continue;
            }
            let _ = stdin.write_all(format!("{line}\r").as_bytes());
            let _ = stdin.flush();
            thread::sleep(Duration::from_millis(1500));
        }
        drop(qmp_link);
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
        Some(d) => match make_disk(&out_dir.join(stem), d)? {
            Some(img) => Some((img, d)),
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
    let qmp = out_dir.join(format!("{}.qmp", stem.to_string_lossy()));
    let disk_arg = disk.as_ref().map(|(p, d)| (p.as_path(), *d));
    let boot_img = out_dir.join(format!("{}-boot.img", stem.to_string_lossy()));
    if spec.boot.is_some() {
        let cfg = spec.esp.iter().find(|(n, _)| n == "psyos.cfg").map_or(String::new(), |(_, t)| t.clone());
        make_usb(&efi, &boot_img, &cfg)?;
    }
    let boot = spec.boot.as_deref().map(|m| (m, boot_img.as_path()));
    let wav = out_dir.join(format!("{}.wav", stem.to_string_lossy()));
    let mut extra = spec.qemu.clone();
    if spec.audio {
        let _ = fs::remove_file(&wav);
        extra.push("-audiodev".into());
        extra.push(format!("wav,id=snd0,path={}", wav.display()));
        extra.push("-device".into());
        extra.push("intel-hda".into());
        extra.push("-device".into());
        extra.push("hda-output,audiodev=snd0".into());
    }
    let (code, raw, stderr) = run_qemu(&efi, disk_arg, &spec.input, esp.as_deref(), &extra, &qmp, boot)?;
    if spec.audio {
        let (loud, crossings) = wav_stats(&wav)?;
        if loud < 20_000 || crossings < 200 {
            return Err(format!("audio: the WAV file has {loud} loud samples and {crossings} sign changes (expected a tone)"));
        }
    }
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
