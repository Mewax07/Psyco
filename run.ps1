.\psycoc\target\release\psycoc.exe --uefi .\psycos\src\kernel.psy -o .\psycoc\esp\EFI\BOOT\BOOTX64.EFI
cd .\psycoc
& "C:\Program Files\qemu\qemu-system-x86_64.exe" -machine q35 -m 256M -net none `
  -drive "if=pflash,format=raw,readonly=on,file=C:\Program Files\qemu\share\edk2-x86_64-code.fd" `
  -drive format=raw,file=fat:rw:esp `
  -serial stdio -device isa-debug-exit,iobase=0xf4,iosize=0x04
cd ..
