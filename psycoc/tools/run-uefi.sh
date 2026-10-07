#!/usr/bin/env bash
set -euo pipefail

EFI=${1:?usage: run-uefi.sh <file.efi> [extra qemu args...]}
shift

OVMF_CODE=${OVMF_CODE:-/usr/share/OVMF/OVMF_CODE_4M.fd}
OVMF_VARS_TEMPLATE=${OVMF_VARS:-/usr/share/OVMF/OVMF_VARS_4M.fd}

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/esp/EFI/BOOT"
cp "$EFI" "$WORK/esp/EFI/BOOT/BOOTX64.EFI"
cp "$OVMF_VARS_TEMPLATE" "$WORK/vars.fd"

DISPLAY_ARGS=(-display none)
if [[ "${GUI:-0}" == 1 ]]; then DISPLAY_ARGS=(); fi

set +e
timeout "${TIMEOUT:-30}" qemu-system-x86_64 \
    -machine q35 -m 256M -cpu qemu64 \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
    -drive if=pflash,format=raw,file="$WORK/vars.fd" \
    -drive format=raw,file=fat:rw:"$WORK/esp" \
    -net none \
    -serial stdio \
    "${DISPLAY_ARGS[@]}" \
    -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
    -no-reboot \
    "$@"
code=$?
set -e

if (( code == 124 )); then
    echo "run-uefi: timeout" >&2
    exit 124
fi
if (( code & 1 )); then
    exit $(( code >> 1 ))
fi
exit "$code"
