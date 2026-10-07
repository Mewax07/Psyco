use crate::assembler::{Assembler, Label};

pub struct ElfBuilder;

impl Default for ElfBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ElfBuilder {
    pub fn new() -> Self {
        Self
    }

    pub fn build(&self, asm: &mut Assembler, entry: Label) -> Vec<u8> {
        let has_rw = asm.rw_size() > 0;
        let phnum = if has_rw { 2 } else { 1 };
        let code_off = (64usize + 56usize * phnum).next_multiple_of(16);
        let rodata_off = (code_off + asm.code.len()).next_multiple_of(16);
        let rx_end = rodata_off + asm.rodata.len();
        let data_off = rx_end.next_multiple_of(0x1000);

        let addr = |off: usize| 0x40_0000 + off as u64;
        asm.link(addr(code_off), addr(rodata_off), addr(data_off));
        let entry_addr = addr(code_off) + asm.offset_of(entry) as u64;

        let mut out = Vec::new();
        out.extend_from_slice(&[0x7F, b'E', b'L', b'F', 2, 1, 1, 0]);
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(&2u16.to_le_bytes()); // ET_EXEC
        out.extend_from_slice(&0x3Eu16.to_le_bytes()); // x86-64
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&entry_addr.to_le_bytes());
        out.extend_from_slice(&(64 as u64).to_le_bytes()); // phoff
        out.extend_from_slice(&0u64.to_le_bytes()); // shoff
        out.extend_from_slice(&0u32.to_le_bytes()); // flags
        out.extend_from_slice(&(64 as u16).to_le_bytes());
        out.extend_from_slice(&(56 as u16).to_le_bytes());
        out.extend_from_slice(&(phnum as u16).to_le_bytes());
        out.extend_from_slice(&[0; 6]);

        let phdr = |out: &mut Vec<u8>, flags: u32, off: usize, filesz: usize, memsz: usize| {
            out.extend_from_slice(&1u32.to_le_bytes()); // PT_LOAD
            out.extend_from_slice(&flags.to_le_bytes());
            out.extend_from_slice(&(off as u64).to_le_bytes());
            out.extend_from_slice(&addr(off).to_le_bytes());
            out.extend_from_slice(&addr(off).to_le_bytes());
            out.extend_from_slice(&(filesz as u64).to_le_bytes());
            out.extend_from_slice(&(memsz as u64).to_le_bytes());
            out.extend_from_slice(&(0x1000 as u64).to_le_bytes());
        };
        phdr(&mut out, 5, 0, rx_end, rx_end); // R+X
        if has_rw {
            phdr(&mut out, 6, data_off, asm.data.len(), asm.rw_size()); // R+W
        }

        out.resize(code_off, 0);
        out.extend_from_slice(&asm.code);
        out.resize(rodata_off, 0);
        out.extend_from_slice(&asm.rodata);
        if has_rw {
            out.resize(data_off, 0);
            out.extend_from_slice(&asm.data);
        }
        out
    }
}
