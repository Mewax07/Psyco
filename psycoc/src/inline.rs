use std::collections::{HashMap, HashSet};
use std::mem::replace;

use crate::ir::*;
use crate::*;

pub struct Unit {
    pub label: Label,
    pub func: Func,
    pub inlinable: bool,
}

pub fn mems_mut(i: &mut Inst) -> Vec<&mut Mem> {
    match i {
        Inst::Load { mem, .. } | Inst::Store { mem, .. } | Inst::Addr { mem, .. } => vec![mem],
        Inst::Copy { dst, src, .. } => vec![dst, src],
        Inst::Zero { dst, .. } | Inst::Fill { dst, .. } => vec![dst],
        _ => Vec::new(),
    }
}

pub fn mems(i: &Inst) -> Vec<Mem> {
    let mut c = i.clone();
    mems_mut(&mut c).into_iter().map(|m| *m).collect()
}

pub fn size(f: &Func) -> usize {
    f.blocks
        .iter()
        .filter(|b| !matches!(b.term, Term::Unreachable) || !b.insts.is_empty())
        .map(|b| b.insts.len() + 1)
        .sum()
}

fn copyable(f: &Func) -> bool {
    f.blocks.iter().all(|b| {
        b.insts.iter().all(|i| {
            mems(i).iter().all(|m| match m {
                Mem::Param(_) => {
                    matches!(i, Inst::Load { mem: Mem::Param(_), ty, .. } if *ty == Ty::U64)
                }
                _ => true,
            })
        })
    })
}

fn calls(f: &Func) -> Vec<Label> {
    let mut v = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                Inst::Call {
                    target: CallT::Label(l),
                    ..
                } => v.push(*l),
                Inst::SymAddr {
                    sym: Sym::Code(l), ..
                } => v.push(*l),
                _ => {}
            }
        }
    }
    v
}

pub fn loop_blocks(f: &Func) -> Vec<bool> {
    let n = f.blocks.len();
    let mut preds: Vec<Vec<BlockId>> = vec![Vec::new(); n];
    for (b, block) in f.blocks.iter().enumerate() {
        for s in block.term.successors() {
            preds[s].push(b);
        }
    }
    let mut state = vec![0u8; n];
    let mut back = Vec::new();
    let mut stack = vec![(0usize, 0usize)];
    state[0] = 1;
    while let Some(&(b, i)) = stack.last() {
        let succ = f.blocks[b].term.successors();
        if i < succ.len() {
            stack.last_mut().unwrap().1 += 1;
            let s = succ[i];
            match state[s] {
                0 => {
                    state[s] = 1;
                    stack.push((s, 0));
                }
                1 => back.push((b, s)),
                _ => {}
            }
        } else {
            state[b] = 2;
            stack.pop();
        }
    }
    let mut inside = vec![false; n];
    for (tail, head) in back {
        inside[head] = true;
        let mut work = vec![tail];
        while let Some(x) = work.pop() {
            if inside[x] && x != tail {
                continue;
            }
            inside[x] = true;
            if x == head {
                continue;
            }
            for &p in &preds[x] {
                if !inside[p] {
                    work.push(p);
                }
            }
        }
    }
    inside
}

fn inline_at(f: &mut Func, b: BlockId, k: usize, callee: &Func) {
    let Inst::Call { dst, args, .. } = f.blocks[b].insts[k].clone() else {
        unreachable!()
    };
    let voff = f.nvregs;
    f.nvregs += callee.nvregs;
    let soff = f.slots.len();
    f.slots.extend(callee.slots.iter().copied());
    let params: Vec<VReg> = args.iter().map(|_| f.new_vreg()).collect();
    let boff = f.blocks.len();
    let cont = boff + callee.blocks.len();

    let tail = f.blocks[b].insts.split_off(k + 1);
    f.blocks[b].insts.pop();
    for (p, a) in params.iter().zip(&args) {
        f.blocks[b].insts.push(Inst::Mov { dst: *p, src: *a });
    }
    let old_term = replace(&mut f.blocks[b].term, Term::Jmp(boff));

    let n = args.len() as i32;
    let rv = |v: VReg| v + voff;
    for cb in &callee.blocks {
        let mut insts = Vec::with_capacity(cb.insts.len());
        for i in &cb.insts {
            if let Inst::Load {
                dst: d,
                mem: Mem::Param(off),
                ..
            } = i
            {
                let idx = n - 1 - (off - 16) / 8;
                insts.push(Inst::Mov {
                    dst: rv(*d),
                    src: Op::V(params[idx as usize]),
                });
                continue;
            }
            let mut i = i.clone();
            if let Some(d) = i.def_mut() {
                *d = rv(*d);
            }
            for u in i.uses_mut() {
                if let Op::V(v) = u {
                    *v = rv(*v);
                }
            }
            for m in mems_mut(&mut i) {
                if let Mem::Slot(s, _) = m {
                    *s += soff;
                }
            }
            insts.push(i);
        }
        let mut term = cb.term.clone();
        for u in term.uses_mut() {
            if let Op::V(v) = u {
                *v = rv(*v);
            }
        }
        for s in term.successors_mut() {
            *s += boff;
        }
        if let Term::Ret(r) = term {
            if let (Some(d), Some(r)) = (dst, r) {
                insts.push(Inst::Mov { dst: d, src: r });
            }
            term = Term::Jmp(cont);
        }
        f.blocks.push(ir::Block { insts, term });
    }
    f.blocks.push(ir::Block {
        insts: tail,
        term: old_term,
    });
}

pub fn inline_program(units: &mut [Unit], optimize: &dyn Fn(&mut Func)) {
    let by_label: HashMap<Label, usize> = units
        .iter()
        .enumerate()
        .map(|(i, u)| (u.label, i))
        .collect();

    let mut sites: HashMap<Label, usize> = HashMap::new();
    for u in units.iter() {
        for l in calls(&u.func) {
            *sites.entry(l).or_default() += 1;
        }
        for b in &u.func.blocks {
            for i in &b.insts {
                if let Inst::SymAddr {
                    sym: Sym::Code(l), ..
                } = i
                {
                    *sites.entry(*l).or_default() += 1000;
                }
            }
        }
    }

    let graph: Vec<Vec<usize>> = units
        .iter()
        .map(|u| {
            calls(&u.func)
                .iter()
                .filter_map(|l| by_label.get(l).copied())
                .collect()
        })
        .collect();
    let mut order = Vec::new();
    let mut state = vec![0u8; units.len()];
    for root in 0..units.len() {
        if state[root] != 0 {
            continue;
        }
        let mut stack = vec![(root, 0usize)];
        state[root] = 1;
        while let Some(&(u, next)) = stack.last() {
            if next < graph[u].len() {
                stack.last_mut().unwrap().1 += 1;
                let c = graph[u][next];
                if state[c] == 0 {
                    state[c] = 1;
                    stack.push((c, 0));
                }
            } else {
                state[u] = 2;
                order.push(u);
                stack.pop();
            }
        }
    }

    for u in order {
        let mut changed = false;
        let mut budget = 64;
        let orig = size(&units[u].func);
        let cap = (orig * 2).max(orig + 150);
        loop {
            let mut site = None;
            let hot = loop_blocks(&units[u].func);
            'find: for (b, block) in units[u].func.blocks.iter().enumerate() {
                for (k, i) in block.insts.iter().enumerate() {
                    let Inst::Call {
                        target: CallT::Label(l),
                        ..
                    } = i
                    else {
                        continue;
                    };
                    let Some(&c) = by_label.get(l) else { continue };
                    if c == u || !units[c].inlinable || calls(&units[c].func).contains(l) {
                        continue;
                    }
                    let s = size(&units[c].func);
                    let limit = if sites.get(l) == Some(&1) {
                        400
                    } else if hot[b] {
                        30
                    } else {
                        8
                    };
                    let fits = sites.get(l) == Some(&1) || size(&units[u].func) + s <= cap;
                    if s <= limit && fits && copyable(&units[c].func) {
                        site = Some((b, k, c));
                        break 'find;
                    }
                }
            }
            let Some((b, k, c)) = site else { break };
            if budget == 0 {
                break;
            }
            budget -= 1;
            let callee = units[c].func.clone();
            inline_at(&mut units[u].func, b, k, &callee);

            changed = true;
        }
        if changed {
            optimize(&mut units[u].func);
        }
    }
}

pub fn live_units(units: &[Unit], roots: &[Label]) -> HashSet<usize> {
    let by_label: HashMap<Label, usize> = units
        .iter()
        .enumerate()
        .map(|(i, u)| (u.label, i))
        .collect();
    let mut live = HashSet::new();
    let mut work: Vec<usize> = roots
        .iter()
        .filter_map(|l| by_label.get(l).copied())
        .collect();
    while let Some(u) = work.pop() {
        if !live.insert(u) {
            continue;
        }
        for l in calls(&units[u].func) {
            if let Some(&c) = by_label.get(&l) {
                work.push(c);
            }
        }
    }
    live
}
