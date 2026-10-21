import os
import shutil
import subprocess
import sys
import tempfile

here = os.path.dirname(os.path.abspath(__file__))
compiler_dir = os.path.dirname(here)
repo = os.path.dirname(compiler_dir)

psycos = os.path.join(repo, "psycos")
user = os.path.join(psycos, "user")
files = os.path.join(psycos, "disk")


def compiler():
    names = ("psycoc.exe",) if os.name == "nt" else ("psycoc",)

    for build in ("release", "debug"):
        for name in names:
            path = os.path.join(compiler_dir, "target", build, name)
            if os.path.isfile(path):
                print(f"make-disk: compiler = {path}")
                return path

    sys.exit("make-disk: build the compiler first (build.ps1)")


def main():
    args = sys.argv[1:]
    image = "disk.img"
    extra = []

    i = 0
    while i < len(args):
        if args[i] == "--size":
            if i + 1 >= len(args):
                sys.exit("make-disk: --size requires a value")
            extra.extend(args[i : i + 2])
            i += 2
        elif args[i] in ("--mbr", "--gpt"):
            extra.append(args[i])
            i += 1
        elif args[i] == "--label":
            if i + 1 >= len(args):
                sys.exit("make-disk: --label requires a value")
            extra.extend(args[i : i + 2])
            i += 2
        else:
            image = args[i]
            i += 1

    print(f"make-disk: repo  = {repo}")
    print(f"make-disk: user  = {user}")
    print(f"make-disk: disk  = {files}")

    if not os.path.isdir(user):
        sys.exit(f"make-disk: user directory not found: {user}")

    if not os.path.isdir(files):
        sys.exit(f"make-disk: disk directory not found: {files}")

    psycoc = compiler()
    stage = tempfile.mkdtemp(prefix="psycos-disk-")

    try:
        shutil.copytree(files, stage, dirs_exist_ok=True)

        print("make-disk: copied files:")
        for root, dirs, names in os.walk(files):
            for name in sorted(names):
                path = os.path.relpath(os.path.join(root, name), files)
                print(f"  {path}")

        bin_dir = os.path.join(stage, "bin")
        os.makedirs(bin_dir, exist_ok=True)

        for name in sorted(os.listdir(user)):
            if not name.endswith(".psy"):
                continue

            source = os.path.join(user, name)
            output = os.path.join(bin_dir, name[:-4])

            subprocess.run(
                [psycoc, source, "-o", output],
                check=True,
            )

            print(f"  bin/{name[:-4]}")

        image = os.path.abspath(image)
        mkfat32 = os.path.join(here, "mkfat32.py")

        subprocess.run(
            [
                sys.executable,
                mkfat32,
                image,
                stage,
                *extra,
            ],
            check=True,
        )

        print(f"make-disk: wrote {image}")

    finally:
        shutil.rmtree(stage, ignore_errors=True)


if __name__ == "__main__":
    main()
