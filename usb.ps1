python psycoc\tools\make-usb.py --cfg "video=1280x800" --dir usb
& "C:\Program Files\qemu\qemu-system-x86_64.exe" -machine q35 -m 512M -net none `
   -drive "if=pflash,format=raw,readonly=on,file=C:\Program Files\qemu\share\edk2-x86_64-code.fd" `
   -drive if=none,id=stick,format=raw,file=usb/PSYOS.img `
   -device qemu-xhci,id=x -device usb-storage,bus=x.0,drive=stick,bootindex=0 `
   -device usb-kbd,bus=x.0 -device usb-tablet,bus=x.0 -serial stdio
