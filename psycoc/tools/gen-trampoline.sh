#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
OUT=../../psycos/src/trampoline.psy
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
as --64 trampoline.S -o "$WORK/t.o"
objcopy -O binary -j .text "$WORK/t.o" "$WORK/t.bin"
sym() { nm "$WORK/t.o" | python3 -c 'import sys; s = sys.argv[1]; print(next(int(l.split()[0], 16) for l in sys.stdin if l.split()[-1] == s))' "$1"; }
python3 - "$WORK/t.bin" "$(sym cr3_slot)" "$(sym stack_slot)" "$(sym entry_slot)" > "$OUT" <<'PY'
import sys
code = open(sys.argv[1], "rb").read()
cr3, stack, entry = sys.argv[2:5]
print("/// Wiki Doc sources:")
print("///   - https://wiki.osdev.org/SMP")
print("///   - https://wiki.osdev.org/Real_Mode")
print("///   - https://wiki.osdev.org/Setting_Up_Long_Mode")
print()
print(f"const trampoline_cr3: usize = {cr3}")
print(f"const trampoline_stack: usize = {stack}")
print(f"const trampoline_entry: usize = {entry}")
print(f"const trampoline_code: [u8; {len(code)}] = [")
for i in range(0, len(code), 16):
    print("    " + ", ".join(f"0x{b:02X}" for b in code[i:i + 16]) + ",")
print("]")
PY
echo "wrote $OUT"
