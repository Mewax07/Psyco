use std::collections::HashMap;

use crate::{
    emit::Emitter,
    inline::{Unit, inline_program, live_units},
    ir::Func,
    lower::Lower,
    opt::optimize,
    regalloc::{allocate, allocate_none},
    *,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Linux,
    Uefi,
}

#[derive(Clone, Copy)]
pub(crate) enum StaticLoc {
    Ro(DataLabel),
    Rw(RwLabel),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    pub optimize: bool,
    pub inline: bool,
    pub regalloc: bool,
}

impl Options {
    pub const ALL: Options = Options {
        optimize: true,
        inline: true,
        regalloc: true,
    };
    pub const NONE: Options = Options {
        optimize: false,
        inline: false,
        regalloc: false,
    };
}

impl Default for Options {
    fn default() -> Self {
        let on = |v: &str| std::env::var(v).is_err();
        Options {
            optimize: on("PSYCOC_NO_OPT"),
            inline: on("PSYCOC_NO_INLINE"),
            regalloc: on("PSYCOC_NO_REGALLOC"),
        }
    }
}

pub struct Codegen<'a> {
    pub(crate) opts: Options,
    pub(crate) asm: Assembler,
    pub(crate) rt: Runtime,
    pub(crate) info: &'a ProgramInfo,
    pub(crate) functions: HashMap<String, Label>,
    pub(crate) strings: HashMap<Vec<u8>, DataLabel>,
    pub(crate) cstrings: HashMap<Vec<u8>, DataLabel>,
    pub(crate) wstrings: HashMap<Vec<u16>, DataLabel>,
    pub(crate) statics: HashMap<String, StaticLoc>,
}

impl<'a> Codegen<'a> {
    pub fn new(info: &'a ProgramInfo) -> Self {
        let mut asm = Assembler::new();
        let rt = Runtime::new(&mut asm);
        Self {
            opts: Options::default(),
            asm,
            rt,
            info,
            functions: HashMap::new(),
            strings: HashMap::new(),
            cstrings: HashMap::new(),
            wstrings: HashMap::new(),
            statics: HashMap::new(),
        }
    }

    pub fn with_options(mut self, opts: Options) -> Self {
        self.opts = opts;
        self
    }

    // Generator

    pub fn generate(self, program: &Program, target: Target) -> Vec<u8> {
        self.generate_with_stats(program, target).0
    }

    pub fn generate_with_stats(mut self, program: &Program, target: Target) -> (Vec<u8>, usize) {
        for f in &program.functions {
            let label = self.asm.new_label();
            self.functions.insert(f.name.clone(), label);
        }
        self.layout_statics();

        let platform: Box<dyn Platform> = match target {
            Target::Linux => Box::new(LinuxPlatform),
            Target::Uefi => Box::new(UefiPlatform),
        };
        let entry = self.asm.new_label();
        let main = self.functions["main"];
        platform.gen_entry(&mut self.asm, &self.rt, entry, main);
        platform.gen_write(&mut self.asm, &self.rt);
        platform.gen_exit(&mut self.asm, &self.rt);
        self.gen_runtime();

        let no_opt = !self.opts.optimize;
        let mut units = Vec::new();
        let mut sources = Vec::new();
        for f in &program.functions {
            if self.info.skip_functions.contains(&f.name) {
                continue;
            }
            let mut func = Lower::new(&mut self, &f.name).lower(f);
            if !no_opt {
                optimize(&mut func);
            }
            let plain = !has_attr(&f.attrs, "interrupt") && !has_attr(&f.attrs, "syscall");
            units.push(Unit {
                label: self.functions[&f.name],
                func,
                inlinable: plain,
            });
            sources.push(f);
        }
        if !no_opt && self.opts.inline {
            inline_program(&mut units, &optimize);
        }
        let mut roots = vec![main];
        for s in &self.info.statics {
            for (_, r) in &s.relocs {
                if let Reloc::Func(name) = r {
                    roots.push(self.functions[name]);
                }
            }
        }
        let live = live_units(&units, &roots);
        for (i, (unit, f)) in units.iter().zip(sources).enumerate() {
            if live.contains(&i) {
                self.gen_function(f, &unit.func);
            }
        }

        self.asm.relax();
        let code_size = self.asm.code.len();
        let image = match target {
            Target::Linux => ElfBuilder::new().build(&mut self.asm, entry),
            Target::Uefi => PeBuilder::new().build(&mut self.asm, entry),
        };
        (image, code_size)
    }

    fn layout_statics(&mut self) {
        let mut with_addresses = Vec::new();
        for s in &self.info.statics {
            let size = self.info.size_of(&s.r#type) as usize;
            let align = s.align as usize;
            let loc = match (&s.init, s.mutable) {
                (Some(bytes), _) if !s.relocs.is_empty() => {
                    let l = self.asm.rw_data(bytes, align.max(8));
                    with_addresses.push((s.name.clone(), l));
                    StaticLoc::Rw(l)
                }
                (Some(bytes), false) => StaticLoc::Ro(self.asm.data_aligned(bytes, align)),
                (None, false) => StaticLoc::Ro(self.asm.data_aligned(&vec![0; size], align)),
                (Some(bytes), true) => StaticLoc::Rw(self.asm.rw_data(bytes, align)),
                (None, true) => StaticLoc::Rw(self.asm.bss(size, align)),
            };
            self.statics.insert(s.name.clone(), loc);
        }
        for (name, label) in with_addresses {
            let RwLabel::Data(base) = label else {
                unreachable!()
            };
            let relocs = self.info.statics[self.info.static_index(&name).unwrap()]
                .relocs
                .clone();
            for (off, r) in relocs {
                let target = match r {
                    Reloc::Func(f) => AddrTarget::Code(self.functions[&f]),
                    Reloc::Str(b) => AddrTarget::Data(self.intern_str(&b)),
                    Reloc::CStr(b) => AddrTarget::Data(self.intern_cstr(&b)),
                    Reloc::WStr(w) => AddrTarget::Data(self.intern_wstr(&w)),
                    Reloc::Static(n) => match self.statics[&n] {
                        StaticLoc::Ro(d) => AddrTarget::Data(d),
                        StaticLoc::Rw(r) => AddrTarget::Rw(r),
                    },
                };
                self.asm.abs_address(base + off, target, 0);
            }
        }
    }

    pub(crate) fn intern_cstr(&mut self, s: &[u8]) -> DataLabel {
        if let Some(&l) = self.cstrings.get(s) {
            return l;
        }
        let mut bytes = s.to_vec();
        bytes.push(0);
        let l = self.asm.data_aligned(&bytes, 1);
        self.cstrings.insert(s.to_vec(), l);
        l
    }

    pub(crate) fn intern_wstr(&mut self, s: &[u16]) -> DataLabel {
        if let Some(&l) = self.wstrings.get(s) {
            return l;
        }
        let mut bytes: Vec<u8> = s.iter().flat_map(|c| c.to_le_bytes()).collect();
        bytes.extend_from_slice(&[0, 0]);
        let l = self.asm.data_aligned(&bytes, 2);
        self.wstrings.insert(s.to_vec(), l);
        l
    }

    fn gen_runtime(&mut self) {
        use Reg::*;
        let rt = self.rt;
        let a = &mut self.asm;

        // print_str : rax = Str
        a.bind(rt.print_str);
        a.load(Rdx, Rax, 0);
        a.lea(Rsi, Rax, 8);
        a.jmp(rt.sys_write);

        // print_int (signed) / print_uint (unsigned) : rax = value
        a.bind(rt.print_uint);
        a.zero(R8);
        let digits = a.new_label();
        a.jmp(digits);

        a.bind(rt.print_int);
        a.mov_rr(R8, Rax);
        let positive = a.new_label();
        a.test_rr(Rax, Rax);
        a.jcc(Cond::NS, positive);
        a.neg(Rax);
        a.bind(positive);

        a.bind(digits);
        a.push(Rbp);
        a.mov_rr(Rbp, Rsp);
        a.alu_ri(Alu::Sub, Rsp, 32);
        a.mov_rr(Rsi, Rbp);
        a.mov_ri(Rcx, 10);
        let digit = a.new_label();
        a.bind(digit);
        a.zero(Rdx);
        a.div(Rcx);
        a.alu_ri(Alu::Add, Rdx, b'0' as i32);
        a.alu_ri(Alu::Sub, Rsi, 1);
        a.store8(Rsi, Rdx);
        a.test_rr(Rax, Rax);
        a.jcc(Cond::NE, digit);
        let write = a.new_label();
        a.test_rr(R8, R8);
        a.jcc(Cond::NS, write);
        a.alu_ri(Alu::Sub, Rsi, 1);
        a.mov_ri(Rdx, b'-' as i64);
        a.store8(Rsi, Rdx);
        a.bind(write);
        a.mov_rr(Rdx, Rbp);
        a.alu_rr(Alu::Sub, Rdx, Rsi);
        a.call(rt.sys_write);
        a.mov_rr(Rsp, Rbp);
        a.pop(Rbp);
        a.ret();

        a.bind(rt.print_bool);
        let is_false = a.new_label();
        a.test_rr(Rax, Rax);
        a.jcc(Cond::E, is_false);
        a.lea_data(Rax, rt.str_true);
        a.jmp(rt.print_str);
        a.bind(is_false);
        a.lea_data(Rax, rt.str_false);
        a.jmp(rt.print_str);

        a.bind(rt.print_space);
        a.lea_data(Rax, rt.str_space);
        a.jmp(rt.print_str);

        a.bind(rt.print_newline);
        a.lea_data(Rax, rt.str_newline);
        a.jmp(rt.print_str);

        // panics
        let panic = a.new_label();
        a.bind(rt.panic_overflow);
        a.lea_data(Rax, rt.msg_overflow);
        a.jmp(panic);
        a.bind(rt.panic_div_zero);
        a.lea_data(Rax, rt.msg_div_zero);
        a.jmp(panic);
        a.bind(rt.panic_bounds);
        a.lea_data(Rax, rt.msg_bounds);
        a.bind(panic);
        a.alu_ri(Alu::And, Rsp, -16);
        a.call(rt.print_str);
        a.mov_ri(Rdi, 101);
        a.jmp(rt.sys_exit);

        // panic(msg) : "panic: " + msg + "\n"
        a.bind(rt.panic_msg);
        a.alu_ri(Alu::And, Rsp, -16);
        a.push(Rax);
        a.push(Rax);
        a.lea_data(Rax, rt.str_panic);
        a.call(rt.print_str);
        a.pop(Rax);
        a.call(rt.print_str);
        a.call(rt.print_newline);
        a.mov_ri(Rdi, 101);
        a.jmp(rt.sys_exit);
    }

    fn gen_function(&mut self, f: &Function, func: &Func) {
        let sig = self.info.functions[&f.name].clone();
        let interrupt = has_attr(&f.attrs, "interrupt");
        let syscall = has_attr(&f.attrs, "syscall");

        let body_label = if syscall {
            let stub = self.functions[&f.name];
            let body = self.asm.new_label();
            self.gen_syscall_stub(stub, body);
            body
        } else if interrupt {
            let stub = self.functions[&f.name];
            let body = self.asm.new_label();
            self.gen_interrupt_stub(stub, body, sig.params.len() == 2);
            body
        } else {
            self.functions[&f.name]
        };

        if std::env::var("PSYC_DUMP_IR").is_ok_and(|n| n == f.name || n == "all") {
            eprintln!("{func}");
        }
        let alloc = if !self.opts.regalloc {
            allocate_none(func)
        } else {
            allocate(func, &crate::regalloc::layout(func))
        };
        Emitter::emit(&mut self.asm, self.rt, func, &alloc, body_label);
    }

    fn gen_syscall_stub(&mut self, stub: Label, body: Label) {
        use Reg::*;
        const SAVED: [Reg; 15] = [
            Rcx, R11, Rax, Rdi, Rsi, Rdx, R10, R8, R9, Rbx, Rbp, R12, R13, R14, R15,
        ];
        let rt = self.rt;
        let a = &mut self.asm;
        a.bind(stub);
        a.store_rw(rt.syscall_user_rsp, Rsp);
        a.load_rw(Rsp, rt.syscall_kernel_rsp);
        a.alu_ri(Alu::Sub, Rsp, 8); // slot for the user rsp
        for r in SAVED {
            a.push(r);
        }
        a.load_rw(Rax, rt.syscall_user_rsp);
        a.store(Rsp, 8 * SAVED.len() as i32, Rax);
        a.cld();
        a.mov_rr(Rax, Rsp); // the saved registers
        save_xmm(a);
        a.push(Rax);
        a.call(body);
        a.alu_ri(Alu::Add, Rsp, 8);
        restore_xmm(a);
        for r in SAVED.iter().rev() {
            a.pop(*r);
        }
        a.pop(Rsp); // user stack
        a.sysretq();
    }

    fn gen_interrupt_stub(&mut self, stub: Label, body: Label, has_code: bool) {
        use Reg::*;
        const SAVED: [Reg; 15] = [
            Rax, Rcx, Rdx, Rbx, Rbp, Rsi, Rdi, R8, R9, R10, R11, R12, R13, R14, R15,
        ];
        let a = &mut self.asm;
        a.bind(stub);
        for r in SAVED {
            a.push(r);
        }
        a.cld();
        save_xmm(a);
        let saved_bytes = 64 + 8 * SAVED.len() as i32;
        if has_code {
            // [regs][code][rip cs rflags rsp ss]
            a.lea(Rax, Rsp, saved_bytes + 8);
            a.push(Rax); // frame
            a.load(Rax, Rsp, saved_bytes + 8);
            a.push(Rax); // code
            a.call(body);
            a.alu_ri(Alu::Add, Rsp, 16);
        } else {
            a.lea(Rax, Rsp, saved_bytes);
            a.push(Rax);
            a.call(body);
            a.alu_ri(Alu::Add, Rsp, 8);
        }
        restore_xmm(a);
        for r in SAVED.iter().rev() {
            a.pop(*r);
        }
        if has_code {
            a.alu_ri(Alu::Add, Rsp, 8);
        }
        a.iretq();
    }

    pub(crate) fn intern_str(&mut self, s: &[u8]) -> DataLabel {
        if let Some(&l) = self.strings.get(s) {
            return l;
        }
        let l = self.asm.data_str(s);
        self.strings.insert(s.to_vec(), l);
        l
    }
}

fn save_xmm(a: &mut Assembler) {
    a.alu_ri(Alu::Sub, Reg::Rsp, 64);
    for k in 0..4u8 {
        a.movdqu_store(Addr::base(Reg::Rsp, 16 * k as i32), k);
    }
}

fn restore_xmm(a: &mut Assembler) {
    for k in 0..4u8 {
        a.movdqu_load(k, Addr::base(Reg::Rsp, 16 * k as i32));
    }
    a.alu_ri(Alu::Add, Reg::Rsp, 64);
}
