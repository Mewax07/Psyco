pub trait Builder {
    fn out(&mut self) -> &mut Vec<u8>;

    fn put_u16(&mut self, v: u16) {
        self.out().extend_from_slice(&v.to_le_bytes());
    }

    fn put_u32(&mut self, v: u32) {
        self.out().extend_from_slice(&v.to_le_bytes());
    }

    fn put_u64(&mut self, v: u64) {
        self.out().extend_from_slice(&v.to_le_bytes());
    }

    fn align_up_64(&self, value: u64, align: u64) -> u64 {
        (value + align - 1) / align * align
    }

    fn align_up_32(&self, value: u32, align: u32) -> u32 {
        (value + align - 1) / align * align
    }
}
