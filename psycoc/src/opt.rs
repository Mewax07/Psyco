use crate::bce::{eliminate, hoist_checks};
use crate::ir::*;
use std::collections::HashMap;
use std::mem::{replace, swap, take};

use crate::inline::{mems, mems_mut};
use crate::regalloc::{layout, liveness};
use crate::*;

pub fn optimize(f: &mut Func) {
    for _ in 0..16 {
        let mut changed = simplify_cfg(f);
        changed |= propagate(f);
        changed |= fold(f);
        changed |= coalesce(f);
        changed |= dce(f);
        changed |= fold_addresses(f);
        changed |= sroa(f);
        changed |= eliminate(f);
        changed |= hoist_checks(f);
        if !changed {
            break;
        }
    }
    simplify_cfg(f);
    select_addressing(f);
    dce(f);
}

fn reachable(f: &Func) -> Vec<bool> {
    let mut seen = vec![false; f.blocks.len()];
    let mut stack = vec![0];
    seen[0] = true;
    while let Some(b) = stack.pop() {
        for s in f.blocks[b].term.successors() {
            if !seen[s] {
                seen[s] = true;
                stack.push(s);
            }
        }
    }
    seen
}

fn preds(f: &Func, live: &[bool]) -> Vec<u32> {
    let mut p = vec![0; f.blocks.len()];
    for (b, block) in f.blocks.iter().enumerate() {
        if live[b] {
            for s in block.term.successors() {
                p[s] += 1;
            }
        }
    }
    p
}

pub fn eval_cond(cc: Cond, a: i64, b: i64) -> bool {
    let (ua, ub) = (a as u64, b as u64);
    match cc {
        Cond::E => a == b,
        Cond::NE => a != b,
        Cond::B => ua < ub,
        Cond::AE => ua >= ub,
        Cond::BE => ua <= ub,
        Cond::A => ua > ub,
        Cond::L => a < b,
        Cond::GE => a >= b,
        Cond::LE => a <= b,
        Cond::G => a > b,
        _ => panic!("ICE: eval_cond({cc:?})"),
    }
}

pub fn canon(v: i128, ty: Ty) -> i64 {
    let bits = ty.size as u32 * 8;
    if bits == 64 {
        return v as i64;
    }
    let mask = (1i128 << bits) - 1;
    let low = v & mask;
    if ty.signed && low >> (bits - 1) & 1 == 1 {
        (low - (1i128 << bits)) as i64
    } else {
        low as i64
    }
}

fn in_range(v: i128, ty: Ty) -> bool {
    let bits = ty.size as u32 * 8;
    if ty.signed {
        v >= -(1i128 << (bits - 1)) && v < (1i128 << (bits - 1))
    } else {
        v >= 0 && v < (1i128 << bits)
    }
}

fn eval_bin(op: BinK, a: i64, b: i64, ty: Ty, mode: Arith) -> Option<i64> {
    let (wa, wb) = if ty.signed || ty.size == 8 && mode == Arith::Plain {
        (a as i128, b as i128)
    } else {
        (a as u64 as i128, b as u64 as i128)
    };
    let bits = ty.size as i64 * 8;
    let exact: i128 = match op {
        BinK::Add => wa + wb,
        BinK::Sub => wa - wb,
        BinK::Mul => wa.checked_mul(wb)?,
        BinK::And => return Some(a & b),
        BinK::Or => return Some(a | b),
        BinK::Xor => return Some(a ^ b),
        BinK::Div | BinK::Mod => {
            if wb == 0 {
                return None;
            }
            let q = if op == BinK::Div { wa / wb } else { wa % wb };
            if !in_range(q, ty) {
                return None;
            }
            q
        }
        BinK::Shl | BinK::Shr => {
            if !(0..bits).contains(&b) {
                return if mode == Arith::Checked {
                    None
                } else {
                    Some(0)
                };
            }
            if op == BinK::Shl {
                let r = (a as u64).wrapping_shl(b as u32) as i64;
                return Some(if mode == Arith::Plain {
                    r
                } else {
                    canon(r as i128, ty)
                });
            }
            return Some(if ty.signed {
                a >> b
            } else {
                ((a as u64) >> b) as i64
            });
        }
    };
    match mode {
        Arith::Checked => in_range(exact, ty).then_some(exact as i64),
        Arith::Wrap => Some(canon(exact, ty)),
        Arith::Plain => Some(exact as i64),
    }
}

fn def_counts(f: &Func, live: &[bool]) -> (Vec<u32>, Vec<u32>) {
    let n = f.nvregs as usize;
    let mut defs = vec![0; n];
    let mut uses = vec![0; n];
    for (b, block) in f.blocks.iter().enumerate() {
        if !live[b] {
            continue;
        }
        for i in &block.insts {
            if let Some(d) = i.def() {
                defs[d as usize] += 1;
            }
            for u in i.uses() {
                if let Op::V(v) = u {
                    uses[v as usize] += 1;
                }
            }
        }
        for u in block.term.uses() {
            if let Op::V(v) = u {
                uses[v as usize] += 1;
            }
        }
    }
    (defs, uses)
}

fn simplify_cfg(f: &mut Func) -> bool {
    let mut changed = false;
    for b in &mut f.blocks {
        if let Term::Br {
            cc,
            a,
            b: bb,
            t,
            f: ff,
        } = b.term
        {
            if t == ff {
                b.term = Term::Jmp(t);
                changed = true;
            } else if let (Op::I(x), Op::I(y)) = (a, bb) {
                b.term = Term::Jmp(if eval_cond(cc, x, y) { t } else { ff });
                changed = true;
            }
        }
    }
    let n = f.blocks.len();
    let target = |f: &Func, mut s: BlockId| {
        for _ in 0..n {
            match f.blocks[s].term {
                Term::Jmp(t) if f.blocks[s].insts.is_empty() && t != s && s != 0 => s = t,
                _ => break,
            }
        }
        s
    };
    for b in 0..n {
        let succ = f.blocks[b].term.successors();
        let new: Vec<BlockId> = succ.iter().map(|&s| target(f, s)).collect();
        if new != succ {
            for (s, t) in f.blocks[b].term.successors_mut().into_iter().zip(new) {
                *s = t;
            }
            changed = true;
        }
    }
    loop {
        let live = reachable(f);
        let p = preds(f, &live);
        let mut merged = false;
        for b in 0..n {
            if !live[b] {
                continue;
            }
            if let Term::Jmp(s) = f.blocks[b].term {
                if s != b && s != 0 && p[s] == 1 {
                    let tail = replace(
                        &mut f.blocks[s],
                        ir::Block {
                            insts: Vec::new(),
                            term: Term::Unreachable,
                        },
                    );
                    f.blocks[b].insts.extend(tail.insts);
                    f.blocks[b].term = tail.term;
                    merged = true;
                    changed = true;
                    break;
                }
            }
        }
        if !merged {
            break;
        }
    }
    let live = reachable(f);
    for (b, block) in f.blocks.iter_mut().enumerate() {
        if !live[b] && (!block.insts.is_empty() || !matches!(block.term, Term::Unreachable)) {
            block.insts.clear();
            block.term = Term::Unreachable;
        }
    }
    changed
}

fn replace_uses(f: &mut Func, map: &dyn Fn(VReg) -> Option<Op>) -> bool {
    let mut changed = false;
    for block in &mut f.blocks {
        for i in &mut block.insts {
            for u in i.uses_mut() {
                if let Op::V(v) = *u {
                    if let Some(n) = map(v) {
                        *u = n;
                        changed = true;
                    }
                }
            }
        }
        for u in block.term.uses_mut() {
            if let Op::V(v) = *u {
                if let Some(n) = map(v) {
                    *u = n;
                    changed = true;
                }
            }
        }
    }
    changed
}

fn propagate(f: &mut Func) -> bool {
    let live = reachable(f);
    let (defs, _) = def_counts(f, &live);
    let n = f.nvregs as usize;
    let mut subst: Vec<Option<Op>> = vec![None; n];
    for (b, block) in f.blocks.iter().enumerate() {
        if !live[b] {
            continue;
        }
        for i in &block.insts {
            if let Inst::Mov { dst, src } = i {
                if defs[*dst as usize] != 1 {
                    continue;
                }
                match src {
                    Op::I(_) => subst[*dst as usize] = Some(*src),
                    Op::V(y) if defs[*y as usize] == 1 && y != dst => {
                        subst[*dst as usize] = Some(*src)
                    }
                    _ => {}
                }
            }
        }
    }
    let resolve = |mut o: Op| {
        for _ in 0..n {
            match o {
                Op::V(v) => match subst[v as usize] {
                    Some(next) => o = next,
                    None => break,
                },
                Op::I(_) => break,
            }
        }
        o
    };
    let resolved: Vec<Option<Op>> = (0..n).map(|v| subst[v].map(resolve)).collect();
    let mut changed = replace_uses(f, &|v| resolved[v as usize]);

    for block in &mut f.blocks {
        let mut known: Vec<(VReg, i64)> = Vec::new();
        for i in &mut block.insts {
            for u in i.uses_mut() {
                if let Op::V(v) = *u {
                    if let Some(&(_, c)) = known.iter().find(|(k, _)| *k == v) {
                        *u = Op::I(c);
                        changed = true;
                    }
                }
            }
            if let Some(d) = i.def() {
                known.retain(|(k, _)| *k != d);
                if let Inst::Mov { src: Op::I(c), .. } = i {
                    known.push((d, *c));
                }
            }
        }
        for u in block.term.uses_mut() {
            if let Op::V(v) = *u {
                if let Some(&(_, c)) = known.iter().find(|(k, _)| *k == v) {
                    *u = Op::I(c);
                    changed = true;
                }
            }
        }
    }
    changed
}

fn power_of_two(k: i64) -> Option<u32> {
    (k > 0 && (k as u64).is_power_of_two()).then(|| k.trailing_zeros())
}

fn fold_inst(i: &Inst) -> Option<Option<Inst>> {
    let mov = |dst: VReg, src: Op| Some(Some(Inst::Mov { dst, src }));
    match *i {
        Inst::Bin {
            dst,
            op,
            a: Op::I(x),
            b: Op::I(y),
            ty,
            mode,
        } => eval_bin(op, x, y, ty, mode).and_then(|r| mov(dst, Op::I(r))),
        Inst::Bin {
            dst,
            op,
            a,
            b: Op::I(k),
            ty,
            mode,
        } => match (op, k) {
            (BinK::Add | BinK::Sub | BinK::Or | BinK::Xor | BinK::Shl | BinK::Shr, 0) => {
                mov(dst, a)
            }
            (BinK::Mul | BinK::Div, 1) => mov(dst, a),
            (BinK::Mul | BinK::And, 0) => mov(dst, Op::I(0)),
            (BinK::Mul, _) if mode != Arith::Checked => power_of_two(k).map(|s| {
                Some(Inst::Bin {
                    dst,
                    op: BinK::Shl,
                    a,
                    b: Op::I(s as i64),
                    ty,
                    mode,
                })
            }),
            (BinK::Div, _) if !ty.signed => power_of_two(k).map(|s| {
                Some(Inst::Bin {
                    dst,
                    op: BinK::Shr,
                    a,
                    b: Op::I(s as i64),
                    ty,
                    mode: Arith::Plain,
                })
            }),
            (BinK::Mod, _) if !ty.signed => power_of_two(k).map(|_| {
                Some(Inst::Bin {
                    dst,
                    op: BinK::And,
                    a,
                    b: Op::I(k - 1),
                    ty,
                    mode: Arith::Plain,
                })
            }),
            (BinK::Shl | BinK::Shr, _)
                if mode == Arith::Checked && (0..ty.size as i64 * 8).contains(&k) =>
            {
                Some(Some(Inst::Bin {
                    dst,
                    op,
                    a,
                    b: Op::I(k),
                    ty,
                    mode: Arith::Wrap,
                }))
            }
            _ => None,
        },
        Inst::Bin {
            dst,
            op: BinK::Add | BinK::Mul | BinK::And | BinK::Or | BinK::Xor,
            a: Op::I(_),
            b: Op::V(_),
            ..
        } => {
            let mut n = i.clone();
            if let Inst::Bin { a, b, .. } = &mut n {
                swap(a, b);
            }
            let _ = dst;
            Some(Some(n))
        }
        Inst::Set {
            dst,
            cc,
            a: Op::I(x),
            b: Op::I(y),
        } => mov(dst, Op::I(eval_cond(cc, x, y) as i64)),
        Inst::Ext {
            dst,
            a: Op::I(x),
            ty,
        } => mov(dst, Op::I(canon(x as i128, ty))),
        Inst::BitNot {
            dst,
            a: Op::I(x),
            ty,
        } => mov(dst, Op::I(canon(!x as i128, ty))),
        Inst::Neg {
            dst,
            a: Op::I(x),
            ty,
        } => {
            let r = -(x as i128);
            in_range(r, ty).then(|| {
                Some(Inst::Mov {
                    dst,
                    src: Op::I(r as i64),
                })
            })
        }
        Inst::Check {
            a: Op::I(x),
            b: Op::I(y),
            cc,
            ..
        } if !eval_cond(cc, x, y) => Some(None),
        _ => None,
    }
}

fn fold(f: &mut Func) -> bool {
    let mut changed = false;
    for block in &mut f.blocks {
        let mut out = Vec::with_capacity(block.insts.len());
        for i in block.insts.drain(..) {
            match fold_inst(&i) {
                Some(new) => {
                    changed = true;
                    out.extend(new);
                }
                None => out.push(i),
            }
        }
        block.insts = out;
    }
    changed
}

fn coalesce(f: &mut Func) -> bool {
    let live = reachable(f);
    let (defs, uses) = def_counts(f, &live);
    let mut changed = false;
    for block in &mut f.blocks {
        let mut j = 0;
        while j < block.insts.len() {
            let Inst::Mov {
                dst: x,
                src: Op::V(t),
            } = block.insts[j]
            else {
                j += 1;
                continue;
            };
            if x == t || defs[t as usize] != 1 || uses[t as usize] != 1 {
                j += 1;
                continue;
            }
            let Some(i) = (0..j).rev().find(|&i| block.insts[i].def() == Some(t)) else {
                j += 1;
                continue;
            };
            let touches_x = block.insts[i + 1..j]
                .iter()
                .any(|k| k.def() == Some(x) || k.uses().contains(&Op::V(x)));
            if touches_x {
                j += 1;
                continue;
            }
            *block.insts[i].def_mut().unwrap() = x;
            block.insts.remove(j);
            changed = true;
        }
    }
    changed
}

fn dce(f: &mut Func) -> bool {
    let order = layout(f);
    let (_, live_out) = liveness(f, &order);
    let n = f.nvregs as usize;
    let mut changed = false;
    for &b in &order {
        let mut live = vec![false; n];
        for (v, l) in live.iter_mut().enumerate() {
            *l = live_out[b].contains(v as VReg);
        }
        let block = &mut f.blocks[b];
        for u in block.term.uses() {
            if let Op::V(v) = u {
                live[v as usize] = true;
            }
        }
        let mut keep = vec![true; block.insts.len()];
        for (k, i) in block.insts.iter().enumerate().rev() {
            if let Some(d) = i.def() {
                let dead_mov = matches!(i, Inst::Mov { dst, src: Op::V(s) } if dst == s);
                if (!live[d as usize] && i.is_pure()) || dead_mov {
                    keep[k] = false;
                    changed = true;
                    continue;
                }
                live[d as usize] = false;
            }
            for u in i.uses() {
                if let Op::V(v) = u {
                    live[v as usize] = true;
                }
            }
        }
        let mut k = 0;
        block.insts.retain(|_| {
            k += 1;
            keep[k - 1]
        });
    }
    changed
}

fn fold_addresses(f: &mut Func) -> bool {
    let live = reachable(f);
    let (defs, _) = def_counts(f, &live);
    let n = f.nvregs as usize;
    let mut known: Vec<Option<Mem>> = vec![None; n];
    for (b, block) in f.blocks.iter().enumerate() {
        if !live[b] {
            continue;
        }
        for i in &block.insts {
            match *i {
                Inst::Addr { dst, mem } if defs[dst as usize] == 1 => {
                    known[dst as usize] = Some(mem)
                }
                Inst::Bin {
                    dst,
                    op: BinK::Add,
                    a: Op::V(y),
                    b: Op::I(k),
                    mode: Arith::Plain,
                    ..
                } if defs[dst as usize] == 1
                    && defs[y as usize] == 1
                    && i32::try_from(k).is_ok() =>
                {
                    known[dst as usize] = Some(Mem::Base(Op::V(y), k as i32))
                }
                _ => {}
            }
        }
    }
    for _ in 0..4 {
        for v in 0..n {
            if let Some(Mem::Base(Op::V(y), k)) = known[v] {
                if let Some(m) = known[y as usize] {
                    known[v] = Some(m.offset(k));
                }
            }
        }
    }
    let mut changed = false;
    for block in &mut f.blocks {
        for i in &mut block.insts {
            for m in mems_mut(i) {
                if let Mem::Base(Op::V(x), d) = *m {
                    if let Some(k) = known[x as usize] {
                        let new = k.offset(d);
                        if new != *m {
                            *m = new;
                            changed = true;
                        }
                    }
                }
            }
        }
    }
    changed
}

fn sroa(f: &mut Func) -> bool {
    let live = reachable(f);
    let mut cand = vec![true; f.slots.len()];
    for (b, block) in f.blocks.iter().enumerate() {
        if !live[b] {
            continue;
        }
        for i in &block.insts {
            if let Inst::Addr {
                mem: Mem::Slot(s, _),
                ..
            } = i
            {
                cand[*s] = false;
                continue;
            }
            for m in mems(i) {
                let Mem::Slot(s, off) = m else { continue };
                let ok = off % 8 == 0
                    && match i {
                        Inst::Load { ty, .. } => ty.size == 8,
                        Inst::Store { size, .. } => *size == 8,
                        Inst::Copy { size, .. } | Inst::Zero { size, .. } => size % 8 == 0,
                        _ => false,
                    };
                if !ok {
                    cand[s] = false;
                }
            }
        }
    }
    if !cand.iter().any(|c| *c) {
        return false;
    }
    let mut words: HashMap<(SlotId, i32), VReg> = HashMap::new();
    let mut word = |f: &mut Func, s: SlotId, off: i32| {
        *words.entry((s, off / 8)).or_insert_with(|| f.new_vreg())
    };
    let is_cand = |m: &Mem| matches!(m, Mem::Slot(s, _) if cand[*s]);
    let mut changed = false;
    for b in 0..f.blocks.len() {
        if !live[b] {
            continue;
        }
        let insts = take(&mut f.blocks[b].insts);
        let mut out = Vec::with_capacity(insts.len());
        for i in insts {
            match i {
                Inst::Load {
                    dst,
                    mem: Mem::Slot(s, o),
                    ..
                } if cand[s] => {
                    let w = word(f, s, o);
                    out.push(Inst::Mov { dst, src: Op::V(w) });
                }
                Inst::Store {
                    mem: Mem::Slot(s, o),
                    src,
                    ..
                } if cand[s] => {
                    let w = word(f, s, o);
                    out.push(Inst::Mov { dst: w, src });
                }
                Inst::Copy { dst, src, size } if is_cand(&dst) || is_cand(&src) => {
                    for k in 0..(size / 8) as i32 {
                        let value = match src {
                            Mem::Slot(s, o) if cand[s] => Op::V(word(f, s, o + 8 * k)),
                            _ => {
                                let t = f.new_vreg();
                                out.push(Inst::Load {
                                    dst: t,
                                    mem: src.offset(8 * k),
                                    ty: Ty::U64,
                                });
                                Op::V(t)
                            }
                        };
                        match dst {
                            Mem::Slot(s, o) if cand[s] => {
                                let w = word(f, s, o + 8 * k);
                                out.push(Inst::Mov { dst: w, src: value });
                            }
                            _ => out.push(Inst::Store {
                                mem: dst.offset(8 * k),
                                src: value,
                                size: 8,
                            }),
                        }
                    }
                }
                Inst::Zero {
                    dst: Mem::Slot(s, o),
                    size,
                } if cand[s] => {
                    for k in 0..(size / 8) as i32 {
                        let w = word(f, s, o + 8 * k);
                        out.push(Inst::Mov {
                            dst: w,
                            src: Op::I(0),
                        });
                    }
                }
                other => {
                    out.push(other);
                    continue;
                }
            }
            changed = true;
        }
        f.blocks[b].insts = out;
    }
    changed
}

fn select_addressing(f: &mut Func) {
    for block in &mut f.blocks {
        for k in 0..block.insts.len() {
            let mems_k = mems(&block.insts[k]);
            let Some(pos) = mems_k
                .iter()
                .position(|m| matches!(m, Mem::Base(Op::V(_), _)))
            else {
                continue;
            };
            let Mem::Base(Op::V(a), d) = mems_k[pos] else {
                unreachable!()
            };
            let Some(i) = (0..k).rev().find(|&i| block.insts[i].def() == Some(a)) else {
                continue;
            };
            let Inst::Bin {
                op: BinK::Add,
                a: Op::V(b),
                b: Op::V(t),
                mode: Arith::Plain,
                ..
            } = block.insts[i]
            else {
                continue;
            };
            let written = |from: usize, v: VReg, insts: &[Inst]| {
                insts[from + 1..k].iter().any(|x| x.def() == Some(v))
            };
            if written(i, b, &block.insts) || written(i, t, &block.insts) {
                continue;
            }
            let mut index = (t, 0u8);
            if let Some(j) = (0..i).rev().find(|&j| block.insts[j].def() == Some(t)) {
                if let Inst::Bin {
                    op: BinK::Shl,
                    a: Op::V(x),
                    b: Op::I(sh),
                    mode: Arith::Plain,
                    ..
                } = block.insts[j]
                {
                    if (0..=3).contains(&sh) && !written(j, x, &block.insts) {
                        index = (x, sh as u8);
                    }
                }
            }
            let new = Mem::Index(Op::V(b), Op::V(index.0), index.1, d);
            if let Some(m) = mems_mut(&mut block.insts[k]).into_iter().nth(pos) {
                *m = new;
            }
        }
    }
}
