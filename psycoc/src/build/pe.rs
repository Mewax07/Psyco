use std::mem::take;

use crate::{Assembler, Builder, Label};

pub struct PeBuilder {
    out: Vec<u8>,
}

impl PeBuilder {
    pub fn new() -> Self {
        Self { out: Vec::new() }
    }

    pub fn build(&mut self, asm: &mut Assembler, entry: Label) -> Vec<u8> {
        let section_align = 0x1000;
        let file_align = 0x200;
        let num_sections = 2u16;

        let code_len = asm.code.len() as u32;
        let data_len = asm.rodata.len() as u32;

        let text_rva = section_align;
        let rdata_rva = section_align + self.align_up_32(code_len, section_align);
        let image_size = rdata_rva + self.align_up_32(data_len, section_align);

        let headers_len = 0x40 + 4 + 20 + 240 + 40 * num_sections as u32;
        let headers_size = self.align_up_32(headers_len, file_align);
        let text_raw = self.align_up_32(code_len, file_align);
        let rdata_raw = self.align_up_32(data_len, file_align);
        let text_offset = headers_size;
        let rdata_offset = text_offset + text_raw;

        asm.link(text_rva as u64, rdata_rva as u64);
        let entry_rva = text_rva + asm.offset_of(entry) as u32;

        let total = (rdata_offset + rdata_raw) as usize;
        self.out.clear();
        self.out.reserve(total);

        self.out.extend_from_slice(b"MZ");
        self.out.resize(0x3C, 0);
        self.put_u32(0x40);

        self.out.extend_from_slice(b"PE\0\0");
        self.put_u16(0x8664); // Machine: AMD64
        self.put_u16(num_sections);
        self.put_u32(0); // TimeDateStamp
        self.put_u32(0); // PointerToSymbolTable
        self.put_u32(0); // NumberOfSymbols
        self.put_u16(240); // SizeOfOptionalHeader
        self.put_u16(0x0002 | 0x0020); // EXECUTABLE_IMAGE | LARGE_ADDRESS_AWARE

        self.put_u16(0x20B); // PE32+
        self.out.push(0); // MajorLinkerVersion
        self.out.push(0); // MinorLinkerVersion
        self.put_u32(text_raw); // SizeOfCode
        self.put_u32(rdata_raw); // SizeOfInitializedData
        self.put_u32(0); // SizeOfUninitializedData
        self.put_u32(entry_rva); // AddressOfEntryPoint
        self.put_u32(text_rva); // BaseOfCode
        self.put_u64(0x1_4000_0000); // ImageBase
        self.put_u32(section_align);
        self.put_u32(file_align);
        self.out.resize(self.out.len() + 12, 0); // versions OS / image
        self.put_u32(0); // Win32VersionValue
        self.put_u32(image_size); // SizeOfImage
        self.put_u32(headers_size); // SizeOfHeaders
        self.put_u32(0); // CheckSum
        self.put_u16(10); // Subsystem: EFI application
        self.put_u16(0); // DllCharacteristics
        self.out.resize(self.out.len() + 32, 0); // StackSize/HeapSize (4 x u64)
        self.put_u32(0); // LoaderFlags
        self.put_u32(16); // NumberOfRvaAndSizes
        self.out.resize(self.out.len() + 16 * 8, 0); // DataDirectory

        self.section_header(
            b".text\0\0\0",
            code_len,
            text_rva,
            text_raw,
            text_offset,
            0x0000_0020 | 0x2000_0000 | 0x4000_0000, // code | execute | read
        );
        self.section_header(
            b".rdata\0\0",
            data_len,
            rdata_rva,
            rdata_raw,
            rdata_offset,
            0x0000_0040 | 0x4000_0000, // initialized data | read
        );

        assert_eq!(self.out.len() as u32, headers_len);
        self.out.resize(text_offset as usize, 0);
        self.out.extend_from_slice(&asm.code);
        self.out.resize(rdata_offset as usize, 0);
        self.out.extend_from_slice(&asm.rodata);
        self.out.resize(total, 0);

        take(&mut self.out)
    }

    fn section_header(
        &mut self,
        name: &[u8; 8],
        virtual_size: u32,
        rva: u32,
        raw_size: u32,
        raw_offset: u32,
        characteristics: u32,
    ) {
        self.out.extend_from_slice(name);
        self.put_u32(virtual_size); // taille réelle en mémoire
        self.put_u32(rva); // où en mémoire
        self.put_u32(raw_size); // taille dans le fichier (alignée)
        self.put_u32(raw_offset); // où dans le fichier
        self.put_u32(0); // PointerToRelocations
        self.put_u32(0); // PointerToLinenumbers
        self.put_u16(0); // NumberOfRelocations
        self.put_u16(0); // NumberOfLinenumbers
        self.put_u32(characteristics);
    }
}

impl Builder for PeBuilder {
    fn out(&mut self) -> &mut Vec<u8> {
        &mut self.out
    }
}
