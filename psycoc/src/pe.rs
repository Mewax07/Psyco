use crate::{Assembler, Label};

struct Section {
    name: [u8; 8],
    rva: u32,
    vsize: u32,
    raw: Vec<u8>,
    flags: u32,
}

pub struct PeBuilder;

impl Default for PeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl PeBuilder {
    pub fn new() -> Self {
        Self
    }

    fn align(&self, v: u32, a: u32) -> u32 {
        v.next_multiple_of(a)
    }

    pub fn build(&self, asm: &mut Assembler, entry: Label) -> Vec<u8> {
        let text_rva = 0x1000u32;
        let rdata_rva = self.align(text_rva + asm.code.len().max(1) as u32, 0x1000u32);
        let data_rva = self.align(rdata_rva + asm.rodata.len().max(1) as u32, 0x1000u32);
        let rw_size = asm.rw_size().max(1) as u32;
        let reloc_rva = self.align(data_rva + rw_size, 0x1000u32);

        asm.link(text_rva as u64, rdata_rva as u64, data_rva as u64);
        let entry_rva = text_rva + asm.offset_of(entry) as u32;

        let mut reloc = Vec::new();
        reloc.extend_from_slice(&text_rva.to_le_bytes());
        reloc.extend_from_slice(&8u32.to_le_bytes());

        let sections = vec![
            Section {
                name: *b".text\0\0\0",
                rva: text_rva,
                vsize: asm.code.len() as u32,
                raw: asm.code.clone(),
                flags: 0x0000_0020u32 | 0x2000_0000u32 | 0x4000_0000u32,
            },
            Section {
                name: *b".rdata\0\0",
                rva: rdata_rva,
                vsize: asm.rodata.len().max(1) as u32,
                raw: asm.rodata.clone(),
                flags: 0x0000_0040u32 | 0x4000_0000u32,
            },
            Section {
                name: *b".data\0\0\0",
                rva: data_rva,
                vsize: rw_size,
                raw: asm.data.clone(),
                flags: 0x0000_0040u32 | 0x4000_0000u32 | 0x8000_0000u32,
            },
            Section {
                name: *b".reloc\0\0",
                rva: reloc_rva,
                vsize: reloc.len() as u32,
                raw: reloc,
                flags: 0x0000_0040u32 | 0x4000_0000u32 | 0x0200_0000u32,
            },
        ];

        let headers_len = 0x40 + 4 + 20 + 240 + 40 * sections.len() as u32;
        let size_of_headers = self.align(headers_len, 0x200u32);
        let size_of_image = self.align(reloc_rva + 8, 0x1000u32);

        let mut file_offsets = Vec::new();
        let mut cursor = size_of_headers;
        for s in &sections {
            file_offsets.push(cursor);
            cursor += self.align(s.raw.len() as u32, 0x200u32);
        }

        let mut out = Vec::new();
        out.extend_from_slice(b"MZ");
        out.resize(0x3C, 0);
        out.extend_from_slice(&0x40u32.to_le_bytes()); // e_lfanew
        out.extend_from_slice(b"PE\0\0");
        out.extend_from_slice(&0x8664u16.to_le_bytes()); // x86-64
        out.extend_from_slice(&(sections.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&240u16.to_le_bytes());
        out.extend_from_slice(&0x0022u16.to_le_bytes()); // EXECUTABLE | LARGE_ADDRESS_AWARE
        let code_size = self.align(sections[0].raw.len() as u32, 0x200u32);
        let init_data: u32 = sections[1..]
            .iter()
            .map(|s| self.align(s.raw.len() as u32, 0x200u32))
            .sum();
        out.extend_from_slice(&0x20Bu16.to_le_bytes());
        out.extend_from_slice(&[1, 0]);
        out.extend_from_slice(&code_size.to_le_bytes());
        out.extend_from_slice(&init_data.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&entry_rva.to_le_bytes());
        out.extend_from_slice(&text_rva.to_le_bytes());
        out.extend_from_slice(&0x40_0000u64.to_le_bytes());
        out.extend_from_slice(&0x1000u32.to_le_bytes());
        out.extend_from_slice(&0x200u32.to_le_bytes());
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&size_of_image.to_le_bytes());
        out.extend_from_slice(&size_of_headers.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&10u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        for v in [0x10_0000u64, 0x1000, 0x10_0000, 0x1000] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&16u32.to_le_bytes());
        for i in 0..16 {
            if i == 5 {
                out.extend_from_slice(&reloc_rva.to_le_bytes());
                out.extend_from_slice(&8u32.to_le_bytes());
            } else {
                out.extend_from_slice(&[0; 8]);
            }
        }
        for (s, &off) in sections.iter().zip(&file_offsets) {
            out.extend_from_slice(&s.name);
            out.extend_from_slice(&s.vsize.to_le_bytes());
            out.extend_from_slice(&s.rva.to_le_bytes());
            let raw_size = self.align(s.raw.len() as u32, 0x200u32);
            out.extend_from_slice(&raw_size.to_le_bytes());
            out.extend_from_slice(&(if raw_size == 0 { 0 } else { off }).to_le_bytes());
            out.extend_from_slice(&[0; 12]);
            out.extend_from_slice(&s.flags.to_le_bytes());
        }
        for (s, &off) in sections.iter().zip(&file_offsets) {
            out.resize(off as usize, 0);
            out.extend_from_slice(&s.raw);
        }
        out.resize(cursor as usize, 0);
        out
    }
}
