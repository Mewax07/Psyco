cd .\psycoc
cargo build --release
if (-not (Test-Path .\esp)) {
    mkdir .\esp\EFI\BOOT | Out-Null
}
cd ..
