import os
import shutil
import subprocess
import sys
import tempfile

here = os.path.dirname(os.path.abspath(__file__))
compiler_dir = os.path.dirname(here)
repo = os.path.dirname(compiler_dir)
kernel_source = os.path.join(repo, "psycos", "src", "kernel.psy")

usage = """usage: make-usb.py [--kernel BOOTX64.EFI] [--cfg TEXT] [--size MIB] [--out usb.img | --dir FOLDER]

Builds a bootable PsycOS USB key:
  usb.img     a GPT disk image (one EFI system partition), to write raw on a key
  --dir F     the same files in a folder, to copy onto a key already formatted in FAT32
The key holds EFI/BOOT/BOOTX64.EFI, psyos.cfg and PSYOS.IMG (the system volume,
loaded in memory by the kernel before it leaves the firmware)."""


def compiler():
    names = ("psycoc.exe",) if os.name == "nt" else ("psycoc",)
    for build in ("release", "debug"):
        for name in names:
            path = os.path.join(compiler_dir, "target", build, name)
            if os.path.isfile(path):
                return path
    sys.exit("make-usb: build the compiler first (cargo build --release)")


def main():
    args = sys.argv[1:]
    kernel, out, folder, size = None, "usb.img", None, 96
    cfg = ["log=on"]
    i = 0
    while i < len(args):
        a = args[i]
        if a in ("--kernel", "--cfg", "--out", "--dir", "--size") and i + 1 >= len(args):
            sys.exit(usage)
        if a == "--kernel":
            kernel = args[i + 1]
        elif a == "--cfg":
            cfg.extend(args[i + 1].replace("\\n", "\n").splitlines())
        elif a == "--out":
            out = args[i + 1]
        elif a == "--dir":
            folder = args[i + 1]
        elif a == "--size":
            size = int(args[i + 1])
        else:
            sys.exit(usage)
        i += 2

    stage = tempfile.mkdtemp(prefix="psycos-usb-")
    try:
        boot = os.path.join(stage, "EFI", "BOOT")
        os.makedirs(boot)
        efi = os.path.join(boot, "BOOTX64.EFI")
        if kernel:
            shutil.copyfile(kernel, efi)
        else:
            subprocess.run([compiler(), "--uefi", kernel_source, "-o", efi], check=True)
        with open(os.path.join(stage, "psyos.cfg"), "w", newline="\n") as f:
            f.write("\n".join(cfg) + "\n")
        system = os.path.join(stage, "PSYOS.IMG")
        subprocess.run(
            [sys.executable, os.path.join(here, "make-disk.py"), system, "--size", "40", "--label", "PSYOS"],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        if folder:
            shutil.copytree(stage, folder, dirs_exist_ok=True)
            print(f"make-usb: files in {os.path.abspath(folder)} (copy them to the root of a FAT32 key)")
        else:
            subprocess.run(
                [sys.executable, os.path.join(here, "mkfat32.py"), out, stage, "--size", str(size), "--gpt", "--label", "PSYOSBOOT"],
                check=True,
            )
            print(f"make-usb: wrote {os.path.abspath(out)} (write it raw to the whole key)")
    finally:
        shutil.rmtree(stage, ignore_errors=True)


if __name__ == "__main__":
    main()
