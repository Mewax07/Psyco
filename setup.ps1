cd .\psycoc
cargo build --release
if (-not (Test-Path .\esp)) {
    mkdir .\esp\EFI\BOOT | Out-Null
}
python .\tools\mkfat32.py --size 64 --mbr disk.img ..\psycos
cd ..
