cd .\psycoc
cargo build --release
if (-not (Test-Path .\esp)) {
    mkdir .\esp\EFI\BOOT | Out-Null
}
python .\tools\make-disk.py disk.img
cd ..
