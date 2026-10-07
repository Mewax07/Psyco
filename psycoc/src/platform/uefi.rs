use crate::{Alu, Assembler, Cond, Label, Platform, Reg::*, Runtime};

pub struct UefiPlatform;

impl UefiPlatform {
    fn outb(a: &mut Assembler, port: i64, value: i64) {
        a.mov_ri(Rdx, port);
        a.mov_ri(Rax, value);
        a.out_dx_al();
    }

    fn emit_byte(a: &mut Assembler, byte: Option<u8>) {
        let wait = a.new_label();
        a.bind(wait);
        a.mov_ri(Rdx, 0x3F8 + 5);
        a.in_al_dx();
        a.alu_ri(Alu::And, Rax, 0x20);
        a.jcc(Cond::E, wait);
        a.mov_ri(Rdx, 0x3F8);
        match byte {
            Some(b) => a.mov_ri(Rax, b as i64),
            None => a.mov_rr(Rax, R8),
        }
        a.out_dx_al();
    }
}

impl Platform for UefiPlatform {
    fn gen_entry(&self, a: &mut Assembler, rt: &Runtime, entry: Label, main: Label) {
        a.bind(entry);
        a.lea_rw(Rax, rt.efi_image_handle);
        a.store(Rax, 0, Rcx);
        a.lea_rw(Rax, rt.efi_system_table);
        a.store(Rax, 0, Rdx);

        Self::outb(a, 0x3F8 + 1, 0x00);
        Self::outb(a, 0x3F8 + 3, 0x80);
        Self::outb(a, 0x3F8, 0x01);
        Self::outb(a, 0x3F8 + 1, 0x00);
        Self::outb(a, 0x3F8 + 3, 0x03);
        Self::outb(a, 0x3F8 + 2, 0xC7);
        Self::outb(a, 0x3F8 + 4, 0x03);

        a.alu_ri(Alu::And, Rsp, -16);
        a.call(main);
        a.zero(Rdi);
        a.jmp(rt.sys_exit);
    }

    fn gen_write(&self, a: &mut Assembler, rt: &Runtime) {
        a.bind(rt.sys_write);
        a.mov_rr(Rcx, Rdx);
        let done = a.new_label();
        let next = a.new_label();
        a.bind(next);
        a.test_rr(Rcx, Rcx);
        a.jcc(Cond::E, done);
        a.load8(R8, Rsi);
        let not_nl = a.new_label();
        a.alu_ri(Alu::Cmp, R8, b'\n' as i32);
        a.jcc(Cond::NE, not_nl);
        Self::emit_byte(a, Some(b'\r'));
        a.bind(not_nl);
        Self::emit_byte(a, None);
        a.alu_ri(Alu::Add, Rsi, 1);
        a.alu_ri(Alu::Sub, Rcx, 1);
        a.jmp(next);
        a.bind(done);
        a.ret();
    }

    fn gen_exit(&self, a: &mut Assembler, rt: &Runtime) {
        a.bind(rt.sys_exit);
        a.mov_rr(Rax, Rdi);
        a.mov_ri(Rdx, 0xF4);
        a.out_dx_al();
        let halt = a.new_label();
        a.bind(halt);
        a.cli();
        a.hlt();
        a.jmp(halt);
    }
}
