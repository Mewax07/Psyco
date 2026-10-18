#![cfg(target_os = "linux")]

use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

use psycoc::{Target, compile_file};

#[test]
fn imported_file_gets_its_own_namespace() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("modules");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("elf.psy"),
        "const pt_load: u32 = 1\nstruct Header { kind: u32 }\nfn is_load(h: &Header) -> bool { return h.kind == pt_load }\nfn parse(k: u32) -> Header { return Header { kind: k } }\n",
    )
    .unwrap();
    fs::write(
        dir.join("pe.psy"),
        "const pt_load: u32 = 9\nfn is_load(k: u32) -> bool { return k == pt_load }\n",
    )
    .unwrap();
    fs::write(
        dir.join("main.psy"),
        "import \"elf.psy\" as elf\nimport \"pe.psy\" as pe\nfn main() {\n    let h = elf::parse(1)\n    print(elf::is_load(&h), pe::is_load(1), elf::pt_load, pe::pt_load)\n}\n",
    )
    .unwrap();
    let bin = compile_file(dir.join("main.psy").to_str().unwrap(), Target::Linux).unwrap();
    let exe = dir.join("main");
    fs::write(&exe, bin).unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    let out = Command::new(&exe).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "true false 1 9\n");
}
