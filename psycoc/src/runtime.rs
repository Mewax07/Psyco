use crate::{Assembler, DataLabel, Label, RwLabel};

#[derive(Clone, Copy)]
pub struct Runtime {
    pub sys_write: Label,
    pub sys_exit: Label,
    pub print_str: Label,
    pub print_int: Label,
    pub print_uint: Label,
    pub print_bool: Label,
    pub print_space: Label,
    pub print_newline: Label,
    pub panic_overflow: Label,
    pub panic_div_zero: Label,
    pub panic_bounds: Label,
    pub panic_msg: Label,
    pub str_true: DataLabel,
    pub str_false: DataLabel,
    pub str_space: DataLabel,
    pub str_newline: DataLabel,
    pub str_panic: DataLabel,
    pub msg_overflow: DataLabel,
    pub msg_div_zero: DataLabel,
    pub msg_bounds: DataLabel,
    pub efi_image_handle: RwLabel,
    pub efi_system_table: RwLabel,
}

impl Runtime {
    pub fn new(a: &mut Assembler) -> Self {
        Self {
            sys_write: a.new_label(),
            sys_exit: a.new_label(),
            print_str: a.new_label(),
            print_int: a.new_label(),
            print_uint: a.new_label(),
            print_bool: a.new_label(),
            print_space: a.new_label(),
            print_newline: a.new_label(),
            panic_overflow: a.new_label(),
            panic_div_zero: a.new_label(),
            panic_bounds: a.new_label(),
            panic_msg: a.new_label(),
            str_true: a.data_str(b"true"),
            str_false: a.data_str(b"false"),
            str_space: a.data_str(b" "),
            str_newline: a.data_str(b"\n"),
            str_panic: a.data_str(b"panic: "),
            msg_overflow: a.data_str(b"panic: integer overflow\n"),
            msg_div_zero: a.data_str(b"panic: division by zero\n"),
            msg_bounds: a.data_str(b"panic: index out of bounds\n"),
            efi_image_handle: a.bss(8, 8),
            efi_system_table: a.bss(8, 8),
        }
    }
}
