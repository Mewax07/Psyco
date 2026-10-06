use crate::{Assembler, DataLabel, Label};

#[derive(Debug, Clone, Copy)]
pub struct Runtime {
    pub print_str: Label,
    pub print_int: Label,
    pub print_bool: Label,
    pub print_space: Label,
    pub print_newline: Label,
    pub panic_overflow: Label,
    pub panic_div_zero: Label,
    pub sys_write: Label,
    pub sys_exit: Label,
    pub str_true: DataLabel,
    pub str_false: DataLabel,
    pub str_space: DataLabel,
    pub str_newline: DataLabel,
    pub msg_overflow: DataLabel,
    pub msg_div_zero: DataLabel,
}

impl Runtime {
    pub fn new(asm: &mut Assembler) -> Self {
        Self {
            print_str: asm.new_label(),
            print_int: asm.new_label(),
            print_bool: asm.new_label(),
            print_space: asm.new_label(),
            print_newline: asm.new_label(),
            panic_overflow: asm.new_label(),
            panic_div_zero: asm.new_label(),
            sys_write: asm.new_label(),
            sys_exit: asm.new_label(),
            str_true: asm.data_str("true"),
            str_false: asm.data_str("false"),
            str_space: asm.data_str(" "),
            str_newline: asm.data_str("\n"),
            msg_overflow: asm.data_str("panic: integer overflow\n"),
            msg_div_zero: asm.data_str("panic: division by zero\n"),
        }
    }
}
