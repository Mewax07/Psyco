
use crate::ir::*;
use crate::*;

pub const POOL: [Reg; 11] = [
    Reg::Rsi,
    Reg::Rdi,
    Reg::R8,
    Reg::R9,
    Reg::R10,
    Reg::R11,
    Reg::Rbx,
    Reg::R12,
    Reg::R13,
    Reg::R14,
    Reg::R15,
];

pub const CALLER_SAVED: u16 = bit(Reg::Rsi) | bit(Reg::Rdi) | bit(Reg::R8) | bit(Reg::R9) | bit(Reg::R10) | bit(Reg::R11);

pub const CALLEE_SAVED: u16 = bit(Reg::Rbx) | bit(Reg::R12) | bit(Reg::R13) | bit(Reg::R14) | bit(Reg::R15);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Loc {
    Reg(Reg),
    Spill(u32),
    None,
}

pub struct Alloc {
    pub loc: Vec<Loc>,
    pub nspills: u32,
    pub saved: Vec<Reg>,
}

pub const fn bit(r: Reg) -> u16 {
    1 << r as u8
}

pub fn clobbers(i: &Inst) -> u16 {
    match i {
        Inst::Call { .. } | Inst::CallRt { .. } | Inst::PrintBytes { .. } => CALLER_SAVED,
        Inst::EfiCall { .. } => CALLER_SAVED | bit(Reg::Rbx),
        Inst::Copy { size, .. } if !small_copy(*size) => bit(Reg::Rsi) | bit(Reg::Rdi),
        Inst::Zero { size, .. } if !small_copy(*size) => bit(Reg::Rdi),
        Inst::Fill { .. } => bit(Reg::Rdi),
        Inst::Intr { kind, .. } => match kind {
            Intr::Memcpy => bit(Reg::Rsi) | bit(Reg::Rdi),
            Intr::Memset | Intr::Memset32 => bit(Reg::Rdi),
            Intr::ContextSwitch | Intr::Syscall => CALLER_SAVED,
            _ => 0,
        },
        _ => 0,
    }
}

pub fn small_copy(size: u64) -> bool {
    size <= 64
}

pub fn chunks(size: u64) -> Vec<(i32, u64)> {
    let mut v = Vec::new();
    let mut off = 0;
    for c in [8, 4, 2, 1] {
        while size - off >= c {
            v.push((off as i32, c));
            off += c;
        }
    }
    v
}

pub fn layout(f: &Func) -> Vec<BlockId> {
    let n = f.blocks.len();
    let mut reachable = vec![false; n];
    let mut preds = vec![0u32; n];
    let mut stack = vec![0];
    reachable[0] = true;
    while let Some(b) = stack.pop() {
        for s in f.blocks[b].term.successors() {
            preds[s] += 1;
            if !reachable[s] {
                reachable[s] = true;
                stack.push(s);
            }
        }
    }
    let mut placed = vec![false; n];
    let mut order = Vec::new();
    for i in 0..n {
        if !reachable[i] || placed[i] {
            continue;
        }
        let mut cur = i;
        loop {
            placed[cur] = true;
            order.push(cur);
            let next = f.blocks[cur]
                .term
                .successors()
                .into_iter()
                .find(|&s| !placed[s] && preds[s] == 1);
            match next {
                Some(s) => cur = s,
                None => break,
            }
        }
    }
    order
}

struct Bits(Vec<u64>);

impl Bits {
    fn new(n: usize) -> Self {
        Bits(vec![0; n.div_ceil(64)])
    }

    fn set(&mut self, i: u32) {
        self.0[i as usize / 64] |= 1 << (i % 64);
    }

    fn get(&self, i: u32) -> bool {
        self.0[i as usize / 64] >> (i % 64) & 1 != 0
    }

    fn union(&mut self, other: &Bits) -> bool {
        let mut changed = false;
        for (a, b) in self.0.iter_mut().zip(&other.0) {
            let n = *a | *b;
            changed |= n != *a;
            *a = n;
        }
        changed
    }

    fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.0.iter().enumerate().flat_map(|(w, &x)| {
            (0..64).filter(move |b| x >> b & 1 != 0).map(move |b| (w * 64 + b) as u32)
        })
    }
}

pub fn liveness(f: &Func, order: &[BlockId]) -> (Vec<Bits_>, Vec<Bits_>) {
    let nv = f.nvregs as usize;
    let n = f.blocks.len();
    let mut gen_ = Vec::with_capacity(n);
    let mut kill = Vec::with_capacity(n);
    for b in &f.blocks {
        let mut g = Bits::new(nv);
        let mut k = Bits::new(nv);
        for i in &b.insts {
            for u in i.uses() {
                if let Op::V(v) = u {
                    if !k.get(v) {
                        g.set(v);
                    }
                }
            }
            if let Some(d) = i.def() {
                k.set(d);
            }
        }
        for u in b.term.uses() {
            if let Op::V(v) = u {
                if !k.get(v) {
                    g.set(v);
                }
            }
        }
        gen_.push(g);
        kill.push(k);
    }
    let mut live_in: Vec<Bits> = (0..n).map(|_| Bits::new(nv)).collect();
    let mut live_out: Vec<Bits> = (0..n).map(|_| Bits::new(nv)).collect();
    let mut changed = true;
    while changed {
        changed = false;
        for &b in order.iter().rev() {
            let mut out = Bits::new(nv);
            for s in f.blocks[b].term.successors() {
                out.union(&live_in[s]);
            }
            let mut inn = Bits::new(nv);
            for (w, x) in inn.0.iter_mut().enumerate() {
                *x = gen_[b].0[w] | (out.0[w] & !kill[b].0[w]);
            }
            if inn.0 != live_in[b].0 {
                live_in[b] = inn;
                changed = true;
            }
            live_out[b] = out;
        }
    }
    (live_in.into_iter().map(Bits_).collect(), live_out.into_iter().map(Bits_).collect())
}

pub struct Bits_(Bits);

impl Bits_ {
    pub fn contains(&self, v: VReg) -> bool {
        self.0.get(v)
    }
}

pub fn allocate(f: &Func, order: &[BlockId]) -> Alloc {
    let nv = f.nvregs as usize;
    let (live_in, live_out) = liveness(f, order);

    let mut start = vec![u32::MAX; nv];
    let mut end = vec![0u32; nv];
    let extend = |v: VReg, p: u32, start: &mut Vec<u32>, end: &mut Vec<u32>| {
        let v = v as usize;
        start[v] = start[v].min(p);
        end[v] = end[v].max(p);
    };
    let mut clobber_points: Vec<(u32, u16)> = Vec::new();
    let mut pos = 0u32;
    for &b in order {
        let block = &f.blocks[b];
        let first = pos;
        for i in &block.insts {
            for u in i.uses() {
                if let Op::V(v) = u {
                    extend(v, pos, &mut start, &mut end);
                }
            }
            if let Some(d) = i.def() {
                extend(d, pos + 1, &mut start, &mut end);
            }
            let c = clobbers(i);
            if c != 0 {
                clobber_points.push((pos, c));
            }
            pos += 2;
        }
        for u in block.term.uses() {
            if let Op::V(v) = u {
                extend(v, pos, &mut start, &mut end);
            }
        }
        let last = pos + 1;
        pos += 2;
        for v in live_in[b].0.iter() {
            extend(v, first, &mut start, &mut end);
        }
        for v in live_out[b].0.iter() {
            extend(v, last, &mut start, &mut end);
        }
    }

    let mut forbidden = vec![0u16; nv];
    let mut saved_mask = 0u16;
    for &(_, c) in &clobber_points {
        saved_mask |= c & CALLEE_SAVED;
    }
    if !clobber_points.is_empty() {
        let mut by_pos = clobber_points.clone();
        by_pos.sort();
        let positions: Vec<u32> = by_pos.iter().map(|x| x.0).collect();
        for v in 0..nv {
            if start[v] == u32::MAX {
                continue;
            }
            let lo = positions.partition_point(|&p| p <= start[v]);
            let hi = positions.partition_point(|&p| p < end[v]);
            for &(_, c) in &by_pos[lo..hi.max(lo)] {
                forbidden[v] |= c;
            }
        }
    }

    let mut order_v: Vec<usize> = (0..nv).filter(|&v| start[v] != u32::MAX).collect();
    order_v.sort_by_key(|&v| (start[v], end[v]));
    let mut loc = vec![Loc::None; nv];
    let mut nspills = 0u32;
    let mut free: u16 = POOL.iter().fold(0, |m, r| m | bit(*r));
    let mut active: Vec<(u32, usize, Reg)> = Vec::new();
    for v in order_v {
        active.retain(|&(e, _, r)| {
            if e < start[v] {
                free |= bit(r);
                false
            } else {
                true
            }
        });
        let allowed = free & !forbidden[v];
        if let Some(r) = POOL.iter().copied().find(|r| allowed & bit(*r) != 0) {
            free &= !bit(r);
            active.push((end[v], v, r));
            loc[v] = Loc::Reg(r);
            continue;
        }
        let victim = active
            .iter()
            .enumerate()
            .filter(|(_, (e, _, r))| *e > end[v] && forbidden[v] & bit(*r) == 0)
            .max_by_key(|(_, (e, _, _))| *e)
            .map(|(i, _)| i);
        match victim {
            Some(i) => {
                let (_, w, r) = active[i];
                loc[w] = Loc::Spill(nspills);
                nspills += 1;
                active[i] = (end[v], v, r);
                loc[v] = Loc::Reg(r);
            }
            None => {
                loc[v] = Loc::Spill(nspills);
                nspills += 1;
            }
        }
    }

    for l in &loc {
        if let Loc::Reg(r) = l {
            if CALLEE_SAVED & bit(*r) != 0 {
                saved_mask |= bit(*r);
            }
        }
    }
    let saved = POOL.iter().copied().filter(|r| saved_mask & bit(*r) != 0).collect();
    Alloc { loc, nspills, saved }
}

pub fn allocate_none(f: &Func) -> Alloc {
    let mut saved_mask = 0;
    for b in &f.blocks {
        for i in &b.insts {
            saved_mask |= clobbers(i) & CALLEE_SAVED;
        }
    }
    Alloc {
        loc: (0..f.nvregs).map(Loc::Spill).collect(),
        nspills: f.nvregs,
        saved: POOL.iter().copied().filter(|r| saved_mask & bit(*r) != 0).collect(),
    }
}
