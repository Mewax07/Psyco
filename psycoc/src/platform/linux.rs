use crate::{Assembler, Label, Platform, Reg::*};

pub struct LinuxPlatform;

impl Platform for LinuxPlatform {
    fn gen_write(&self, asm: &mut Assembler, label: Label) {
        // sys_write: rsi = octets, rdx = length
        asm.bind(label);
        asm.mov_ri(Rax, 1); // write
        asm.mov_ri(Rdi, 1); // stdout
        asm.syscall();
        asm.ret();
    }

    fn gen_exit(&self, asm: &mut Assembler, label: Label) {
        // sys_exit: rdi = code
        asm.bind(label);
        asm.mov_ri(Rax, 60); // exit
        asm.syscall();
    }
}
