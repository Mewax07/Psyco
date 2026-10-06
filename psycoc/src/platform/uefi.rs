use crate::{Alu, Assembler, Cond, Label, Platform, Reg::*};

pub struct UefiPlatform;

impl Platform for UefiPlatform {
    fn gen_write(&self, asm: &mut Assembler, label: Label) {
        let com1: i64 = 0x3F8;

        let putc = asm.new_label();
        let wait = asm.new_label();
        asm.bind(putc);
        asm.push(Rax);
        asm.bind(wait);
        asm.mov_ri(Rdx, com1 + 5);
        asm.in_al_dx();
        asm.alu_ri(Alu::And, Rax, 0x20);
        asm.jcc(Cond::E, wait);
        asm.pop(Rax);
        asm.mov_ri(Rdx, com1);
        asm.out_dx_al();
        asm.ret();

        // sys_write: rsi = octets, rdx = length, \n => \r\n
        let next = asm.new_label();
        let send = asm.new_label();
        let done = asm.new_label();
        asm.bind(label);
        asm.mov_rr(Rcx, Rdx);
        asm.bind(next);
        asm.test_rr(Rcx, Rcx);
        asm.jcc(Cond::E, done);
        asm.load8(Rax, Rsi);
        asm.alu_ri(Alu::Cmp, Rax, b'\n' as i32);
        asm.jcc(Cond::NE, send);
        asm.push(Rax);
        asm.mov_ri(Rax, b'\r' as i64);
        asm.call(putc);
        asm.pop(Rax);
        asm.bind(send);
        asm.call(putc);
        asm.alu_ri(Alu::Add, Rsi, 1);
        asm.alu_ri(Alu::Sub, Rcx, 1);
        asm.jmp(next);
        asm.bind(done);
        asm.ret();
    }

    fn gen_exit(&self, asm: &mut Assembler, label: Label) {
        // sys_exit : rdi = code
        let halt = asm.new_label();
        asm.bind(label);
        asm.mov_rr(Rax, Rdi);
        asm.mov_ri(Rdx, 0xF4);
        asm.out_dx_eax();
        asm.bind(halt);
        asm.cli();
        asm.hlt();
        asm.jmp(halt);
    }
}
