#!/bin/sh
# usage : ./run-uefi.sh examples/fact.psy
set -e
OVMF_DIR=${OVMF_DIR:-/usr/share/edk2/x64}

cargo build --release -q
./target/release/psycoc "$1" --uefi

mkdir -p esp/EFI/BOOT
cp out.efi esp/EFI/BOOT/BOOTX64.EFI

[ -f ovmf_vars.fd ] || cp "$OVMF_DIR/OVMF_VARS.4m.fd" ovmf_vars.fd

set +e
qemu-system-x86_64 \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_DIR/OVMF_CODE.4m.fd" \
    -drive if=pflash,format=raw,file=ovmf_vars.fd \
    -drive format=raw,file=fat:rw:esp \
    -serial stdio \
    -display none \
    -no-reboot \
    -device isa-debug-exit,iobase=0xf4,iosize=0x04
code=$?

case $code in
    1)
        echo "programme terminé avec le code 0"
        ;;
    *)
        guest_code=$(( (code - 1) / 2 ))
        echo "programme terminé avec le code $guest_code"
        ;;
esac
