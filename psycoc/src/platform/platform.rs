use crate::{Assembler, Label, Runtime};

pub trait Platform {
    fn gen_entry(&self, a: &mut Assembler, rt: &Runtime, entry: Label, main: Label);
    fn gen_write(&self, a: &mut Assembler, rt: &Runtime);
    fn gen_exit(&self, a: &mut Assembler, rt: &Runtime);
}
