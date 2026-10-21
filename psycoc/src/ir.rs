use std::fmt;

use crate::*;

pub type VReg = u32;
pub type BlockId = usize;
pub type SlotId = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    V(VReg),
    I(i64),
}

impl Op {
    pub fn vreg(self) -> Option<VReg> {
        match self {
            Op::V(v) => Some(v),
            Op::I(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ty {
    pub size: u8,
    pub signed: bool,
}

impl Ty {
    pub const U64: Ty = Ty {
        size: 8,
        signed: false,
    };
    pub const U8: Ty = Ty {
        size: 1,
        signed: false,
    };
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mem {
    Base(Op, i32),
    Slot(SlotId, i32),
    Param(i32),
    Index(Op, Op, u8, i32),
}

impl Mem {
    pub fn offset(self, d: i32) -> Mem {
        match self {
            Mem::Base(o, x) => Mem::Base(o, x + d),
            Mem::Slot(s, x) => Mem::Slot(s, x + d),
            Mem::Param(x) => Mem::Param(x + d),
            Mem::Index(b, i, s, x) => Mem::Index(b, i, s, x + d),
        }
    }

    pub fn ops(&self) -> Vec<Op> {
        match self {
            Mem::Base(o, _) => vec![*o],
            Mem::Index(b, i, _, _) => vec![*b, *i],
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinK {
    Add,
    Sub,
    Mul,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Div,
    Mod,
}

impl BinK {
    pub fn commutative(self) -> bool {
        matches!(
            self,
            BinK::Add | BinK::Mul | BinK::And | BinK::Or | BinK::Xor
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arith {
    Checked,
    Wrap,
    Plain,
}

#[derive(Debug, Clone, Copy)]
pub enum Sym {
    Data(DataLabel),
    Rw(RwLabel),
    Code(Label),
}

#[derive(Debug, Clone, Copy)]
pub enum CallT {
    Label(Label),
    Op(Op),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Intr {
    Out(u8),
    In(u8),
    Cli,
    Sti,
    Hlt,
    Pause,
    Int3,
    Ud2,
    ReadFlags,
    ReadSreg(u8),
    ReadTr,
    ReadCr(u8),
    WriteCr(u8),
    Rdmsr,
    Wrmsr,
    Rdtsc,
    Invlpg,
    Lgdt,
    Lidt,
    LoadCs,
    LoadDs,
    Ltr,
    Memcpy,
    Memset,
    Memset32,
    ContextSwitch,
    Syscall,
    RebaseStatics,
    SetSyscallStack,
    AtomicSwap,
    EfiImageHandle,
    EfiSystemTable,
}

#[derive(Debug, Clone)]
pub enum Inst {
    Mov {
        dst: VReg,
        src: Op,
    },
    Bin {
        dst: VReg,
        op: BinK,
        a: Op,
        b: Op,
        ty: Ty,
        mode: Arith,
    },
    Neg {
        dst: VReg,
        a: Op,
        ty: Ty,
    },
    BitNot {
        dst: VReg,
        a: Op,
        ty: Ty,
    },
    Ext {
        dst: VReg,
        a: Op,
        ty: Ty,
    },
    Set {
        dst: VReg,
        cc: Cond,
        a: Op,
        b: Op,
    },
    Load {
        dst: VReg,
        mem: Mem,
        ty: Ty,
    },
    Store {
        mem: Mem,
        src: Op,
        size: u8,
    },
    Addr {
        dst: VReg,
        mem: Mem,
    },
    SymAddr {
        dst: VReg,
        sym: Sym,
    },
    Call {
        dst: Option<VReg>,
        target: CallT,
        args: Vec<Op>,
    },
    EfiCall {
        dst: Option<VReg>,
        callee: Op,
        args: Vec<Op>,
        ret: Option<Ty>,
    },
    CallRt {
        routine: Label,
        arg: Option<Op>,
    },
    PrintBytes {
        slice: Op,
    },
    Copy {
        dst: Mem,
        src: Mem,
        size: u64,
    },
    Zero {
        dst: Mem,
        size: u64,
    },
    Fill {
        dst: Mem,
        value: Op,
        count: u64,
        size: u8,
    },
    Check {
        a: Op,
        b: Op,
        cc: Cond,
        trap: Label,
    },
    Intr {
        kind: Intr,
        dst: Option<VReg>,
        args: Vec<Op>,
    },
}

#[derive(Debug, Clone)]
pub enum Term {
    Jmp(BlockId),
    Br {
        cc: Cond,
        a: Op,
        b: Op,
        t: BlockId,
        f: BlockId,
    },
    Ret(Option<Op>),
    Panic(Op),
    Exit(Op),
    IretTo(Vec<Op>),
    SwitchStack {
        stack: Op,
        f: Op,
    },
    Unreachable,
}

impl Term {
    pub fn successors(&self) -> Vec<BlockId> {
        match self {
            Term::Jmp(b) => vec![*b],
            Term::Br { t, f, .. } => vec![*t, *f],
            _ => Vec::new(),
        }
    }

    pub fn successors_mut(&mut self) -> Vec<&mut BlockId> {
        match self {
            Term::Jmp(b) => vec![b],
            Term::Br { t, f, .. } => vec![t, f],
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Block {
    pub insts: Vec<Inst>,
    pub term: Term,
}

#[derive(Debug, Clone)]
pub struct Func {
    pub name: String,
    pub blocks: Vec<Block>,
    pub nvregs: u32,
    pub slots: Vec<(u64, u64)>,
}

impl Func {
    pub fn new_vreg(&mut self) -> VReg {
        self.nvregs += 1;
        self.nvregs - 1
    }
}

impl Inst {
    pub fn def(&self) -> Option<VReg> {
        match self {
            Inst::Mov { dst, .. }
            | Inst::Bin { dst, .. }
            | Inst::Neg { dst, .. }
            | Inst::BitNot { dst, .. }
            | Inst::Ext { dst, .. }
            | Inst::Set { dst, .. }
            | Inst::Load { dst, .. }
            | Inst::Addr { dst, .. }
            | Inst::SymAddr { dst, .. } => Some(*dst),
            Inst::Call { dst, .. } | Inst::EfiCall { dst, .. } | Inst::Intr { dst, .. } => *dst,
            _ => None,
        }
    }

    pub fn def_mut(&mut self) -> Option<&mut VReg> {
        match self {
            Inst::Mov { dst, .. }
            | Inst::Bin { dst, .. }
            | Inst::Neg { dst, .. }
            | Inst::BitNot { dst, .. }
            | Inst::Ext { dst, .. }
            | Inst::Set { dst, .. }
            | Inst::Load { dst, .. }
            | Inst::Addr { dst, .. }
            | Inst::SymAddr { dst, .. } => Some(dst),
            Inst::Call { dst, .. } | Inst::EfiCall { dst, .. } | Inst::Intr { dst, .. } => {
                dst.as_mut()
            }
            _ => None,
        }
    }

    pub fn uses(&self) -> Vec<Op> {
        let mem = |m: &Mem| m.ops();
        match self {
            Inst::Mov { src, .. } => vec![*src],
            Inst::Bin { a, b, .. } | Inst::Set { a, b, .. } | Inst::Check { a, b, .. } => {
                vec![*a, *b]
            }
            Inst::Neg { a, .. } | Inst::BitNot { a, .. } | Inst::Ext { a, .. } => vec![*a],
            Inst::Load { mem: m, .. } | Inst::Addr { mem: m, .. } => mem(m),
            Inst::Store { mem: m, src, .. } => {
                let mut v = mem(m);
                v.push(*src);
                v
            }
            Inst::SymAddr { .. } => Vec::new(),
            Inst::Call { target, args, .. } => {
                let mut v = Vec::new();
                if let CallT::Op(o) = target {
                    v.push(*o);
                }
                v.extend(args.iter().copied());
                v
            }
            Inst::EfiCall { callee, args, .. } => {
                let mut v = vec![*callee];
                v.extend(args.iter().copied());
                v
            }
            Inst::CallRt { arg, .. } => arg.iter().copied().collect(),
            Inst::PrintBytes { slice } => vec![*slice],
            Inst::Copy { dst, src, .. } => {
                let mut v = mem(dst);
                v.extend(mem(src));
                v
            }
            Inst::Zero { dst, .. } => mem(dst),
            Inst::Fill { dst, value, .. } => {
                let mut v = mem(dst);
                v.push(*value);
                v
            }
            Inst::Intr { args, .. } => args.clone(),
        }
    }

    pub fn uses_mut(&mut self) -> Vec<&mut Op> {
        fn mem(m: &mut Mem) -> Vec<&mut Op> {
            match m {
                Mem::Base(o, _) => vec![o],
                Mem::Index(b, i, _, _) => vec![b, i],
                _ => Vec::new(),
            }
        }
        match self {
            Inst::Mov { src, .. } => vec![src],
            Inst::Bin { a, b, .. } | Inst::Set { a, b, .. } | Inst::Check { a, b, .. } => {
                vec![a, b]
            }
            Inst::Neg { a, .. } | Inst::BitNot { a, .. } | Inst::Ext { a, .. } => vec![a],
            Inst::Load { mem: m, .. } | Inst::Addr { mem: m, .. } => mem(m),
            Inst::Store { mem: m, src, .. } => {
                let mut v = mem(m);
                v.push(src);
                v
            }
            Inst::SymAddr { .. } => Vec::new(),
            Inst::Call { target, args, .. } => {
                let mut v = Vec::new();
                if let CallT::Op(o) = target {
                    v.push(o);
                }
                v.extend(args.iter_mut());
                v
            }
            Inst::EfiCall { callee, args, .. } => {
                let mut v = vec![callee];
                v.extend(args.iter_mut());
                v
            }
            Inst::CallRt { arg, .. } => arg.iter_mut().collect(),
            Inst::PrintBytes { slice } => vec![slice],
            Inst::Copy { dst, src, .. } => {
                let mut v = mem(dst);
                v.extend(mem(src));
                v
            }
            Inst::Zero { dst, .. } => mem(dst),
            Inst::Fill { dst, value, .. } => {
                let mut v = mem(dst);
                v.push(value);
                v
            }
            Inst::Intr { args, .. } => args.iter_mut().collect(),
        }
    }

    pub fn is_pure(&self) -> bool {
        match self {
            Inst::Mov { .. }
            | Inst::Set { .. }
            | Inst::BitNot { .. }
            | Inst::Ext { .. }
            | Inst::Addr { .. }
            | Inst::SymAddr { .. }
            | Inst::Load { .. } => true,
            Inst::Bin { op, mode, b, .. } => match op {
                BinK::Div | BinK::Mod => matches!(b, Op::I(k) if *k != 0 && *k != -1),
                _ => *mode != Arith::Checked,
            },
            _ => false,
        }
    }
}

impl Term {
    pub fn uses(&self) -> Vec<Op> {
        match self {
            Term::Br { a, b, .. } => vec![*a, *b],
            Term::Ret(o) => o.iter().copied().collect(),
            Term::Panic(o) | Term::Exit(o) => vec![*o],
            Term::IretTo(v) => v.clone(),
            Term::SwitchStack { stack, f } => vec![*stack, *f],
            Term::Jmp(_) | Term::Unreachable => Vec::new(),
        }
    }

    pub fn uses_mut(&mut self) -> Vec<&mut Op> {
        match self {
            Term::Br { a, b, .. } => vec![a, b],
            Term::Ret(o) => o.iter_mut().collect(),
            Term::Panic(o) | Term::Exit(o) => vec![o],
            Term::IretTo(v) => v.iter_mut().collect(),
            Term::SwitchStack { stack, f } => vec![stack, f],
            Term::Jmp(_) | Term::Unreachable => Vec::new(),
        }
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Op::V(v) => write!(f, "%{v}"),
            Op::I(i) => write!(f, "{i}"),
        }
    }
}

impl fmt::Display for Mem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Mem::Base(o, d) => write!(f, "[{o}{d:+}]"),
            Mem::Slot(s, d) => write!(f, "[slot{s}{d:+}]"),
            Mem::Param(d) => write!(f, "[arg{d:+}]"),
            Mem::Index(b, i, s, d) => write!(f, "[{b}+{i}*{}{d:+}]", 1 << s),
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}{}",
            if self.signed { "i" } else { "u" },
            self.size * 8
        )
    }
}

impl fmt::Display for Func {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(
            f,
            "fn {} ({} vregs, {} slots)",
            self.name,
            self.nvregs,
            self.slots.len()
        )?;
        for (i, b) in self.blocks.iter().enumerate() {
            writeln!(f, "b{i}:")?;
            for inst in &b.insts {
                let d = |v: &VReg| format!("%{v} = ");
                let ops = |v: &[Op]| {
                    v.iter()
                        .map(|o| o.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let line = match inst {
                    Inst::Mov { dst, src } => format!("{}{src}", d(dst)),
                    Inst::Bin {
                        dst,
                        op,
                        a,
                        b,
                        ty,
                        mode,
                    } => {
                        format!(
                            "{}{op:?}.{ty}{} {a}, {b}",
                            d(dst),
                            match mode {
                                Arith::Checked => "!",
                                Arith::Wrap => "%",
                                Arith::Plain => "",
                            }
                        )
                    }
                    Inst::Neg { dst, a, ty } => format!("{}neg.{ty} {a}", d(dst)),
                    Inst::BitNot { dst, a, ty } => format!("{}not.{ty} {a}", d(dst)),
                    Inst::Ext { dst, a, ty } => format!("{}ext.{ty} {a}", d(dst)),
                    Inst::Set { dst, cc, a, b } => format!("{}set.{cc:?} {a}, {b}", d(dst)),
                    Inst::Load { dst, mem, ty } => format!("{}load.{ty} {mem}", d(dst)),
                    Inst::Store { mem, src, size } => format!("store.{} {mem}, {src}", size * 8),
                    Inst::Addr { dst, mem } => format!("{}addr {mem}", d(dst)),
                    Inst::SymAddr { dst, sym } => format!("{}sym {sym:?}", d(dst)),
                    Inst::Call { dst, target, args } => format!(
                        "{}call {target:?}({})",
                        dst.map_or(String::new(), |v| d(&v)),
                        ops(args)
                    ),
                    Inst::EfiCall {
                        dst, callee, args, ..
                    } => format!(
                        "{}efi_call {callee}({})",
                        dst.map_or(String::new(), |v| d(&v)),
                        ops(args)
                    ),
                    Inst::CallRt { routine, arg } => format!(
                        "rt {routine:?}({})",
                        arg.map_or(String::new(), |a| a.to_string())
                    ),
                    Inst::PrintBytes { slice } => format!("print_bytes {slice}"),
                    Inst::Copy { dst, src, size } => format!("copy {dst}, {src}, {size}"),
                    Inst::Zero { dst, size } => format!("zero {dst}, {size}"),
                    Inst::Fill {
                        dst,
                        value,
                        count,
                        size,
                    } => format!("fill.{} {dst}, {value} x{count}", size * 8),
                    Inst::Check { a, b, cc, .. } => format!("check !({a} {cc:?} {b})"),
                    Inst::Intr { kind, dst, args } => format!(
                        "{}{kind:?}({})",
                        dst.map_or(String::new(), |v| d(&v)),
                        ops(args)
                    ),
                };
                writeln!(f, "    {line}")?;
            }
            let t = match &b.term {
                Term::Jmp(t) => format!("jmp b{t}"),
                Term::Br { cc, a, b, t, f } => format!("br {a} {cc:?} {b} ? b{t} : b{f}"),
                Term::Ret(o) => format!("ret {}", o.map_or(String::new(), |o| o.to_string())),
                Term::Panic(o) => format!("panic {o}"),
                Term::Exit(o) => format!("exit {o}"),
                Term::IretTo(v) => format!("iret_to {v:?}"),
                Term::SwitchStack { stack, f } => format!("switch_stack {stack}, {f}"),
                Term::Unreachable => "unreachable".into(),
            };
            writeln!(f, "    {t}")?;
        }
        Ok(())
    }
}
