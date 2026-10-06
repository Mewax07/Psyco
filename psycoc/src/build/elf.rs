use std::mem::take;

use crate::{
    Builder,
    assembler::{Assembler, Label},
};

pub struct ElfBuilder {
    out: Vec<u8>,
}

impl ElfBuilder {
    pub fn new() -> Self {
        Self { out: Vec::new() }
    }

    pub fn build(&mut self, asm: &mut Assembler, entry: Label) -> Vec<u8> {
        let base = 0x400000;
        let headers = 176;
        let code_end = headers + asm.code.len() as u64;
        let rodata_offset = self.align_up_64(code_end, 0x1000);

        asm.link(base + headers, base + rodata_offset);
        let entry_addr = base + headers + asm.offset_of(entry) as u64;

        self.out.extend_from_slice(&[0x7F, b'E', b'L', b'F']); // signature
        self.out.push(2); // 64 bits
        self.out.push(1); // little endian
        self.out.push(1); // version ELF
        self.out.push(0); // ABI system V
        self.out.extend_from_slice(&[0; 8]);
        self.put_u16(2); // e_type
        self.put_u16(0x3E); // e_machine: x86_64
        self.put_u32(1); // e_version
        self.put_u64(entry_addr); // e_entry
        self.put_u64(64); // e_phoff
        self.put_u64(0); // e_shoff
        self.put_u32(0); // e_flags
        self.put_u16(64 as u16);
        self.put_u16(56 as u16);
        self.put_u16(2); // e_phnum
        self.put_u16(64); // e_shentsize
        self.put_u16(0); // e_shnum
        self.put_u16(0); // e_shstrndx

        self.program_header(4u32 | 1u32, 0, base, code_end);
        self.program_header(
            1u32,
            rodata_offset,
            base + rodata_offset,
            asm.rodata.len() as u64,
        );

        assert_eq!(self.out.len() as u64, headers);

        self.out.extend_from_slice(&asm.code);
        self.out.resize(rodata_offset as usize, 0);
        self.out.extend_from_slice(&asm.rodata);

        take(&mut self.out)
    }

    fn program_header(&mut self, flags: u32, offset: u64, vaddr: u64, size: u64) {
        self.put_u32(1); // p_type: PT_LOAD
        self.put_u32(flags);
        self.put_u64(offset); // where in file
        self.put_u64(vaddr); // where in memory
        self.put_u64(vaddr); // p_addr (ignored)
        self.put_u64(size); // size in file
        self.put_u64(size); // size in memory
        self.put_u64(0x1000); // align
    }
}

impl Builder for ElfBuilder {
    fn out(&mut self) -> &mut Vec<u8> {
        &mut self.out
    }
}
