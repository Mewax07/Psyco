use crate::{Alu, Assembler, Label, Platform, Reg::*, Runtime};

pub struct LinuxPlatform;

impl Platform for LinuxPlatform {
    fn gen_entry(&self, a: &mut Assembler, rt: &Runtime, entry: Label, main: Label) {
        a.bind(entry);
        a.alu_ri(Alu::And, Rsp, -16);
        a.call(main);
        a.zero(Rdi);
        a.jmp(rt.sys_exit);
    }

    fn gen_write(&self, a: &mut Assembler, rt: &Runtime) {
        a.bind(rt.sys_write);
        a.mov_ri(Rax, 1);
        a.mov_ri(Rdi, 1);
        a.syscall();
        a.ret();
    }

    fn gen_exit(&self, a: &mut Assembler, rt: &Runtime) {
        a.bind(rt.sys_exit);
        a.mov_ri(Rax, 60);
        a.syscall();
    }
}
