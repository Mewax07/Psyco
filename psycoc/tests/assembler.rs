#[cfg(test)]
mod tests {
    use psycoc::{Alu, Assembler, Reg};

    fn encode(f: impl FnOnce(&mut Assembler)) -> Vec<u8> {
        let mut a = Assembler::new();
        f(&mut a);
        a.code
    }

    #[test]
    fn reg_encoding_order() {
        assert_eq!(Reg::Rax as u8, 0);
        assert_eq!(Reg::Rcx as u8, 1);
        assert_eq!(Reg::Rdx as u8, 2);
        assert_eq!(Reg::Rbx as u8, 3);
        assert_eq!(Reg::Rsp as u8, 4);
        assert_eq!(Reg::Rbp as u8, 5);
        assert_eq!(Reg::Rsi as u8, 6);
        assert_eq!(Reg::Rdi as u8, 7);
        assert_eq!(Reg::R8 as u8, 8);
        assert_eq!(Reg::R9 as u8, 9);
        assert_eq!(Reg::R10 as u8, 10);
        assert_eq!(Reg::R11 as u8, 11);
        assert_eq!(Reg::R12 as u8, 12);
        assert_eq!(Reg::R13 as u8, 13);
        assert_eq!(Reg::R14 as u8, 14);
        assert_eq!(Reg::R15 as u8, 15);
    }

    #[test]
    fn push_pop() {
        assert_eq!(encode(|a| a.push(Reg::Rax)), vec![0x50]);
        assert_eq!(encode(|a| a.push(Reg::Rdi)), vec![0x57]);
        assert_eq!(encode(|a| a.push(Reg::R8)), vec![0x41, 0x50]);
        assert_eq!(encode(|a| a.pop(Reg::Rcx)), vec![0x59]);
        assert_eq!(encode(|a| a.pop(Reg::R15)), vec![0x41, 0x5F]);
    }

    #[test]
    fn ret_syscall_cqo() {
        assert_eq!(encode(|a| a.ret()), vec![0xC3]);
        assert_eq!(encode(|a| a.syscall()), vec![0x0F, 0x05]);
        assert_eq!(encode(|a| a.cqo()), vec![0x48, 0x99]);
    }

    #[test]
    fn mov_rr() {
        assert_eq!(
            encode(|a| a.mov_rr(Reg::Rax, Reg::Rdi)),
            vec![0x48, 0x89, 0xF8]
        );
        assert_eq!(
            encode(|a| a.mov_rr(Reg::Rsi, Reg::Rax)),
            vec![0x48, 0x89, 0xC6]
        );
        assert_eq!(
            encode(|a| a.mov_rr(Reg::R8, Reg::Rax)),
            vec![0x49, 0x89, 0xC0]
        );
        assert_eq!(
            encode(|a| a.mov_rr(Reg::Rax, Reg::R9)),
            vec![0x4C, 0x89, 0xC8]
        );
        assert_eq!(
            encode(|a| a.mov_rr(Reg::R9, Reg::R15)),
            vec![0x4D, 0x89, 0xF9]
        );
    }

    #[test]
    fn mov_ri_imm32() {
        assert_eq!(
            encode(|a| a.mov_ri(Reg::Rbx, 3)),
            vec![0xBB, 0x03, 0x00, 0x00, 0x00]
        );
        assert_eq!(
            encode(|a| a.mov_ri(Reg::Rax, -1)),
            vec![0x48, 0xC7, 0xC0, 0xFF, 0xFF, 0xFF, 0xFF]
        );
        assert_eq!(
            encode(|a| a.mov_ri(Reg::Rax, -1)),
            vec![0x48, 0xC7, 0xC0, 0xFF, 0xFF, 0xFF, 0xFF]
        );
    }

    #[test]
    fn mov_ri_imm64() {
        let v: i64 = 0x1234_5678_9ABC_DEF0;
        let expected = std::iter::once(0x48)
            .chain(std::iter::once(0xB8))
            .chain(v.to_le_bytes())
            .collect::<Vec<u8>>();
        assert_eq!(encode(|a| a.mov_ri(Reg::Rax, v as i64)), expected);

        let out = encode(|a| a.mov_ri(Reg::R15, v as i64));
        assert_eq!(&out[..2], &[0x49, 0xBF]);
        assert_eq!(out.len(), 10);
    }

    #[test]
    fn alu_rr() {
        assert_eq!(
            encode(|a| a.alu_rr(Alu::Add, Reg::Rax, Reg::Rbx)),
            vec![0x48, 0x01, 0xD8]
        );
        assert_eq!(
            encode(|a| a.alu_rr(Alu::Sub, Reg::Rdi, Reg::Rsi)),
            vec![0x48, 0x29, 0xF7]
        );
        assert_eq!(
            encode(|a| a.alu_rr(Alu::Cmp, Reg::Rdx, Reg::Rax)),
            vec![0x48, 0x39, 0xC2]
        );
        assert_eq!(
            encode(|a| a.alu_rr(Alu::Xor, Reg::R8, Reg::R9)),
            vec![0x4D, 0x31, 0xC8]
        );
    }

    #[test]
    fn alu_ri_imm8() {
        assert_eq!(
            encode(|a| a.alu_ri(Alu::Sub, Reg::Rbx, 1)),
            vec![0x48, 0x83, 0xEB, 0x01]
        );
        assert_eq!(
            encode(|a| a.alu_ri(Alu::Cmp, Reg::Rcx, 0)),
            vec![0x48, 0x83, 0xF9, 0x00]
        );
        assert_eq!(
            encode(|a| a.alu_ri(Alu::Add, Reg::Rax, -8)),
            vec![0x48, 0x83, 0xC0, 0xF8]
        );
        assert_eq!(
            encode(|a| a.alu_ri(Alu::Add, Reg::R12, 4)),
            vec![0x49, 0x83, 0xC4, 0x04]
        );
    }

    #[test]
    fn alu_ri_imm32() {
        assert_eq!(
            encode(|a| a.alu_ri(Alu::Cmp, Reg::Rbx, 300)),
            vec![0x48, 0x81, 0xFB, 0x2C, 0x01, 0x00, 0x00]
        );
        assert_eq!(
            encode(|a| a.alu_ri(Alu::Sub, Reg::Rax, -129)),
            vec![0x48, 0x81, 0xE8, 0x7F, 0xFF, 0xFF, 0xFF]
        );
    }

    #[test]
    fn memory_operands() {
        assert_eq!(
            encode(|a| a.load(Reg::Rax, Reg::Rbp, -8)),
            vec![0x48, 0x8B, 0x45, 0xF8]
        );
        assert_eq!(
            encode(|a| a.load(Reg::Rax, Reg::Rbp, 127)),
            vec![0x48, 0x8B, 0x45, 0x7F]
        );
        assert_eq!(
            encode(|a| a.load(Reg::Rax, Reg::Rbp, -1000)),
            vec![0x48, 0x8B, 0x85, 0x18, 0xFC, 0xFF, 0xFF]
        );
    }

    #[test]
    fn kernel_ops() {
        assert_eq!(encode(|a| a.in_al_dx()), vec![0xEC]);
        assert_eq!(encode(|a| a.out_dx_al()), vec![0xEE]);
        assert_eq!(encode(|a| a.out_dx_eax()), vec![0xEF]);
        assert_eq!(encode(|a| a.hlt()), vec![0xF4]);
        assert_eq!(encode(|a| a.cli()), vec![0xFA]);
    }

    #[test]
    fn load8() {
        assert_eq!(encode(|a| a.load8(Reg::Rax, Reg::Rsi)), vec![0x48, 0x0F, 0xB6, 0x06]);
        assert_eq!(encode(|a| a.load8(Reg::Rax, Reg::Rsp)), vec![0x48, 0x0F, 0xB6, 0x04, 0x24]);
        assert_eq!(encode(|a| a.load8(Reg::Rax, Reg::Rbp)), vec![0x48, 0x0F, 0xB6, 0x45, 0x00]);
    }
}
