use crate::{Assembler, Label};

pub trait Platform {
    fn gen_write(&self, asm: &mut Assembler, label: Label);
    fn gen_exit(&self, asm: &mut Assembler, label: Label);
}
