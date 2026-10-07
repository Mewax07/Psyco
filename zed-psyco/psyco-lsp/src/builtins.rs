pub struct Builtin {
    pub name: &'static str,
    pub params: &'static [&'static str],
    pub ret: &'static str,
    pub trusted: bool,
    pub doc: &'static str,
}

impl Builtin {
    pub fn signature(&self) -> String {
        let ret = if self.ret.is_empty() {
            String::new()
        } else {
            format!(" -> {}", self.ret)
        };
        format!("fn {}({}){ret}", self.name, self.params.join(", "))
    }
}

macro_rules! builtin {
    ($name:literal ($($p:literal),*), $ret:literal, $trusted:literal, $doc:literal) => {
        Builtin { name: $name, params: &[$($p),*], ret: $ret, trusted: $trusted, doc: $doc }
    };
}

pub const BUILTINS: &[Builtin] = &[
    builtin!("print"("values..."), "", false, "Prints its arguments separated by spaces, followed by a newline."),
    builtin!("panic"("message: Str"), "", false, "Stops the program with an error message."),
    builtin!("exit"("code: i32"), "", false, "Terminates the program with the given exit code."),
    builtin!("outb"("port: u16", "value: u8"), "", true, "Writes a byte to an I/O port."),
    builtin!("outw"("port: u16", "value: u16"), "", true, "Writes a word to an I/O port."),
    builtin!("outl"("port: u16", "value: u32"), "", true, "Writes a double word to an I/O port."),
    builtin!("inb"("port: u16"), "u8", true, "Reads a byte from an I/O port."),
    builtin!("inw"("port: u16"), "u16", true, "Reads a word from an I/O port."),
    builtin!("inl"("port: u16"), "u32", true, "Reads a double word from an I/O port."),
    builtin!("cli"(), "", false, "Disables interrupts."),
    builtin!("sti"(), "", false, "Enables interrupts."),
    builtin!("hlt"(), "", false, "Halts the CPU until the next interrupt."),
    builtin!("pause"(), "", false, "Spin-loop hint."),
    builtin!("int3"(), "", false, "Breakpoint trap."),
    builtin!("read_cr0"(), "u64", false, "Reads control register CR0."),
    builtin!("read_cr2"(), "u64", false, "Reads control register CR2 (page fault address)."),
    builtin!("read_cr3"(), "u64", false, "Reads control register CR3 (page table root)."),
    builtin!("read_cr4"(), "u64", false, "Reads control register CR4."),
    builtin!("rdtsc"(), "u64", false, "Reads the time-stamp counter."),
    builtin!("write_cr0"("value: u64"), "", true, "Writes control register CR0."),
    builtin!("write_cr3"("value: u64"), "", true, "Writes control register CR3."),
    builtin!("write_cr4"("value: u64"), "", true, "Writes control register CR4."),
    builtin!("rdmsr"("msr: u32"), "u64", true, "Reads a model-specific register."),
    builtin!("wrmsr"("msr: u32", "value: u64"), "", true, "Writes a model-specific register."),
    builtin!("invlpg"("addr: usize"), "", false, "Invalidates the TLB entry of a page."),
    builtin!("load_cs"("selector: u16"), "", true, "Reloads the code segment register."),
    builtin!("load_ds"("selector: u16"), "", true, "Reloads the data segment registers."),
    builtin!("ltr"("selector: u16"), "", true, "Loads the task register."),
    builtin!("memcpy"("dst: *mut u8", "src: *const u8", "len: usize"), "", true, "Copies `len` bytes from `src` to `dst`."),
    builtin!("memset"("dst: *mut u8", "value: u8", "len: usize"), "", true, "Fills `len` bytes at `dst` with `value`."),
    builtin!("efi_image_handle"(), "*mut u8", false, "UEFI image handle given to the program."),
    builtin!("efi_system_table"(), "*mut u8", false, "UEFI system table given to the program."),
    builtin!("switch_stack"("stack: usize", "f: fn()"), "", true, "Switches to a new stack and calls `f` on it. Never returns."),
    builtin!("lgdt"("descriptor: &T"), "", true, "Loads the global descriptor table register."),
    builtin!("lidt"("descriptor: &T"), "", true, "Loads the interrupt descriptor table register."),
];

pub fn builtin(name: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.name == name)
}

pub const KEYWORDS: &[(&str, &str)] = &[
    ("fn", "Declares a function."),
    ("let", "Declares a local variable."),
    ("mut", "Makes a binding, reference or pointer mutable."),
    ("if", "Conditional branch."),
    ("else", "Alternative branch of an `if`."),
    ("while", "Loops while a condition holds."),
    ("loop", "Loops forever, until `break`."),
    ("for", "Iterates over a range: `for i in 0..n { }`."),
    ("in", "Separates the variable and the range of a `for` loop."),
    ("break", "Exits the innermost loop."),
    ("continue", "Jumps to the next iteration of the innermost loop."),
    ("return", "Returns from the current function."),
    ("match", "Branches on a value: `match x { 1 | 2 => ..., _ => ... }`."),
    ("struct", "Declares a structure."),
    ("enum", "Declares an enumeration, optionally with an integer representation."),
    ("impl", "Adds methods to a type."),
    ("as", "Casts a value to another type."),
    ("static", "Declares a global variable."),
    ("const", "Declares a compile-time constant."),
    ("extern", "Uses the UEFI (Microsoft x64) calling convention for a function type."),
    ("import", "Imports another source file: `import \"path.psy\"`."),
    ("sizeof", "Size of a type in bytes: `sizeof(T)`."),
    ("true", "Boolean true."),
    ("false", "Boolean false."),
];

pub const PRIMITIVES: &[(&str, &str)] = &[
    ("Int", "Signed 64-bit integer (alias of `i64`)."),
    ("i8", "Signed 8-bit integer."),
    ("i16", "Signed 16-bit integer."),
    ("i32", "Signed 32-bit integer."),
    ("i64", "Signed 64-bit integer."),
    ("isize", "Signed pointer-sized integer."),
    ("u8", "Unsigned 8-bit integer."),
    ("u16", "Unsigned 16-bit integer."),
    ("u32", "Unsigned 32-bit integer."),
    ("u64", "Unsigned 64-bit integer."),
    ("usize", "Unsigned pointer-sized integer."),
    ("Bool", "Boolean (alias of `bool`)."),
    ("bool", "Boolean."),
    ("Str", "String slice: length and bytes."),
];

pub fn keyword_doc(word: &str) -> Option<&'static str> {
    KEYWORDS
        .iter()
        .chain(PRIMITIVES)
        .find(|(k, _)| *k == word)
        .map(|(_, d)| *d)
}
