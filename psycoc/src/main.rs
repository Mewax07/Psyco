use std::{env, fs, process};

use psycoc::{Target, compile_file};

fn usage_error(message: &str) -> ! {
    eprintln!("psycoc: {message}\nusage: psycoc [--uefi] [-o <output>] <file>");
    process::exit(1);
}

fn main() {
    let mut uefi = false;
    let mut path = None;
    let mut output = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--uefi" => uefi = true,
            "-o" => match args.next() {
                Some(o) => output = Some(o),
                None => usage_error("-o needs a file name"),
            },
            "-h" | "--help" => {
                println!("usage: psycoc [--uefi] [-o <output>] <file>");
                return;
            }
            flag if flag.starts_with('-') => usage_error(&format!("unknown option '{flag}'")),
            _ if path.is_some() => usage_error("only one input file is supported"),
            _ => path = Some(arg),
        }
    }

    let Some(path) = path else {
        usage_error("no input file");
    };

    let target = if uefi { Target::Uefi } else { Target::Linux };
    let output = output.unwrap_or_else(|| if uefi { "out.efi".into() } else { "out".into() });

    let binary = compile_file(&path, target).unwrap_or_else(|e| {
        eprintln!("{e}");
        process::exit(1);
    });

    fs::write(&output, binary).unwrap_or_else(|e| {
        eprintln!("cannot write {output}: {e}");
        process::exit(1);
    });

    #[cfg(unix)]
    if target == Target::Linux {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = fs::set_permissions(&output, fs::Permissions::from_mode(0o755)) {
            eprintln!("warning: cannot make {output} executable: {e}");
        }
    }
}
