#[cfg(test)]
mod tests {
    use psycoc::{Alu, Assembler, Cond, Reg};

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
        // 0 reste un vrai `mov` : mov_ri ne doit pas toucher aux flags.
        assert_eq!(
            encode(|a| a.mov_ri(Reg::Rax, 0)),
            vec![0xB8, 0x00, 0x00, 0x00, 0x00]
        );
        // u32::MAX tient dans `mov r32, imm32` (zero-extend), pas besoin de REX.W.
        assert_eq!(
            encode(|a| a.mov_ri(Reg::Rcx, 0xFFFF_FFFF)),
            vec![0xB9, 0xFF, 0xFF, 0xFF, 0xFF]
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

    // Les octets attendus ci-dessous ont été obtenus avec GNU as + objdump.

    #[test]
    fn load8() {
        assert_eq!(encode(|a| a.load8(Reg::Rax, Reg::Rsi)), vec![0x0F, 0xB6, 0x06]);
        assert_eq!(encode(|a| a.load8(Reg::Rax, Reg::Rsp)), vec![0x0F, 0xB6, 0x04, 0x24]);
        assert_eq!(encode(|a| a.load8(Reg::Rax, Reg::Rbp)), vec![0x0F, 0xB6, 0x45, 0x00]);
        assert_eq!(
            encode(|a| a.load8(Reg::R9, Reg::R12)),
            vec![0x45, 0x0F, 0xB6, 0x0C, 0x24]
        );
    }

    #[test]
    fn setcc_all_byte_regs() {
        assert_eq!(encode(|a| a.setcc(Cond::E, Reg::Rax)), vec![0x0F, 0x94, 0xC0]);
        assert_eq!(encode(|a| a.setcc(Cond::LE, Reg::Rbx)), vec![0x0F, 0x9E, 0xC3]);
        // sil : sans le 0x40 forcé, ce serait `setl dh`.
        assert_eq!(encode(|a| a.setcc(Cond::L, Reg::Rsi)), vec![0x40, 0x0F, 0x9C, 0xC6]);
        assert_eq!(encode(|a| a.setcc(Cond::G, Reg::R8)), vec![0x41, 0x0F, 0x9F, 0xC0]);
        assert_eq!(encode(|a| a.setcc(Cond::NE, Reg::R15)), vec![0x41, 0x0F, 0x95, 0xC7]);
    }

    #[test]
    fn movzx8_all_byte_regs() {
        assert_eq!(encode(|a| a.movzx8(Reg::Rax, Reg::Rax)), vec![0x0F, 0xB6, 0xC0]);
        assert_eq!(encode(|a| a.movzx8(Reg::Rcx, Reg::Rbx)), vec![0x0F, 0xB6, 0xCB]);
        assert_eq!(encode(|a| a.movzx8(Reg::Rax, Reg::Rsi)), vec![0x40, 0x0F, 0xB6, 0xC6]);
        assert_eq!(encode(|a| a.movzx8(Reg::R8, Reg::Rax)), vec![0x44, 0x0F, 0xB6, 0xC0]);
        assert_eq!(encode(|a| a.movzx8(Reg::Rax, Reg::R9)), vec![0x41, 0x0F, 0xB6, 0xC1]);
        assert_eq!(encode(|a| a.movzx8(Reg::R15, Reg::Rdi)), vec![0x44, 0x0F, 0xB6, 0xFF]);
    }

    #[test]
    fn store8() {
        assert_eq!(encode(|a| a.store8(Reg::Rsi, Reg::Rdx)), vec![0x88, 0x16]);
        assert_eq!(encode(|a| a.store8(Reg::Rsi, Reg::Rsi)), vec![0x40, 0x88, 0x36]);
        assert_eq!(encode(|a| a.store8(Reg::R8, Reg::Rax)), vec![0x41, 0x88, 0x00]);
        assert_eq!(encode(|a| a.store8(Reg::Rsi, Reg::R9)), vec![0x44, 0x88, 0x0E]);
        assert_eq!(encode(|a| a.store8(Reg::Rbp, Reg::Rdi)), vec![0x40, 0x88, 0x7D, 0x00]);
    }

    #[test]
    fn imul_ri() {
        assert_eq!(
            encode(|a| a.imul_ri(Reg::Rax, Reg::Rax, 3)),
            vec![0x48, 0x6B, 0xC0, 0x03]
        );
        assert_eq!(
            encode(|a| a.imul_ri(Reg::Rax, Reg::Rcx, -8)),
            vec![0x48, 0x6B, 0xC1, 0xF8]
        );
        assert_eq!(
            encode(|a| a.imul_ri(Reg::Rax, Reg::Rax, 1000)),
            vec![0x48, 0x69, 0xC0, 0xE8, 0x03, 0x00, 0x00]
        );
        assert_eq!(
            encode(|a| a.imul_ri(Reg::R8, Reg::R9, 127)),
            vec![0x4D, 0x6B, 0xC1, 0x7F]
        );
        assert_eq!(
            encode(|a| a.imul_ri(Reg::Rax, Reg::R12, 128)),
            vec![0x49, 0x69, 0xC4, 0x80, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn zero_and_test() {
        assert_eq!(encode(|a| a.zero(Reg::Rax)), vec![0x31, 0xC0]);
        assert_eq!(encode(|a| a.zero(Reg::R8)), vec![0x45, 0x31, 0xC0]);
        assert_eq!(encode(|a| a.test_rr(Reg::Rax, Reg::Rax)), vec![0x48, 0x85, 0xC0]);
        assert_eq!(encode(|a| a.test_rr(Reg::R8, Reg::Rcx)), vec![0x49, 0x85, 0xC8]);
    }

    const ALL_CONDS: [Cond; 10] = [
        Cond::O, Cond::NO, Cond::E, Cond::NE, Cond::S,
        Cond::NS, Cond::L, Cond::GE, Cond::LE, Cond::G,
    ];

    #[test]
    fn cond_negate_flips_low_bit() {
        for c in ALL_CONDS {
            assert_eq!(c.negate() as u8, c as u8 ^ 1, "{c:?}");
            assert_eq!(c.negate().negate(), c);
        }
    }

    #[test]
    fn cond_swap() {
        use Cond::*;
        for (c, s) in [(L, G), (G, L), (LE, GE), (GE, LE), (E, E), (NE, NE)] {
            assert_eq!(c.swap(), s, "{c:?}");
            assert_eq!(c.swap().swap(), c);
        }
    }
}
