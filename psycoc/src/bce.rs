use std::{
    collections::{HashMap, HashSet},
    mem::take,
};

use crate::{
    Cond,
    inline::loop_blocks,
    ir::{Arith, BinK, Block, BlockId, Func, Inst, Op, Term, VReg},
    regalloc::layout,
};

type Facts = Vec<(Op, Op)>;

fn upper(facts: &Facts, a: Op, depth: u32) -> Option<u128> {
    match a {
        Op::I(c) => Some(c as u64 as u128 + 1),
        Op::V(_) => {
            let mut best: Option<u128> = None;
            for &(x, b) in facts {
                if x != a {
                    continue;
                }
                let bound = match b {
                    Op::I(c) => Some(c as u64 as u128),
                    Op::V(_) if depth > 0 => {
                        upper(facts, b, depth - 1).map(|u| u.saturating_sub(1))
                    }
                    _ => None,
                };
                if let Some(u) = bound {
                    best = Some(best.map_or(u, |x| x.min(u)));
                }
            }
            best
        }
    }
}

fn less(facts: &Facts, a: Op, b: Op) -> bool {
    if facts.contains(&(a, b)) {
        return true;
    }
    match b {
        Op::I(c) => upper(facts, a, 2).is_some_and(|u| u <= c as u64 as u128),
        Op::V(_) => false,
    }
}

fn less_eq(facts: &Facts, a: Op, b: Op) -> bool {
    if a == b || less(facts, a, b) || a == Op::I(0) {
        return true;
    }
    match b {
        Op::I(c) => upper(facts, a, 2).is_some_and(|u| u <= c as u64 as u128 + 1),
        _ => false,
    }
}

fn kill(facts: &mut Facts, v: VReg) {
    facts.retain(|(a, b)| *a != Op::V(v) && *b != Op::V(v));
}

fn add(facts: &mut Facts, a: Op, b: Op) {
    if !facts.contains(&(a, b)) {
        facts.push((a, b));
    }
}

fn cond_fact(cc: Cond, a: Op, b: Op) -> Option<(Op, Op)> {
    match cc {
        Cond::B => Some((a, b)),
        Cond::A => Some((b, a)),
        Cond::E => match b {
            Op::I(c) if c != -1 => Some((a, Op::I(c + 1))),
            _ => None,
        },
        Cond::BE => match b {
            Op::I(c) if c != -1 => Some((a, Op::I(c + 1))),
            _ => None,
        },
        Cond::AE => match a {
            Op::I(c) if c != -1 => Some((b, Op::I(c + 1))),
            _ => None,
        },
        _ => None,
    }
}

fn transfer(facts: &mut Facts, i: &Inst) {
    if let Inst::Check { a, b, cc, .. } = *i {
        if let Some((x, y)) = cond_fact(cc.negate(), a, b) {
            add(facts, x, y);
        }
        return;
    }
    let Some(d) = i.def() else { return };
    let mut gen_: Facts = Vec::new();
    match *i {
        Inst::Mov { src: Op::I(c), .. } if c != -1 => gen_.push((Op::V(d), Op::I(c + 1))),
        Inst::Mov { src: Op::V(y), .. } if y != d => {
            for &(a, b) in facts.iter() {
                if a == Op::V(y) {
                    gen_.push((Op::V(d), b));
                }
            }
        }
        Inst::Bin {
            op: BinK::And,
            b: Op::I(c),
            ..
        } if c >= 0 && c != i64::MAX => gen_.push((Op::V(d), Op::I(c + 1))),
        Inst::Bin {
            op: BinK::Mod,
            b,
            ty,
            ..
        } if !ty.signed && b != Op::V(d) => gen_.push((Op::V(d), b)),
        Inst::Bin {
            op: BinK::Shr,
            b: Op::I(k),
            ty,
            ..
        } if !ty.signed && ty.size == 8 && (2..64).contains(&k) => {
            gen_.push((Op::V(d), Op::I(1i64 << (64 - k))))
        }
        Inst::Ext { ty, .. } | Inst::Load { ty, .. } if !ty.signed && ty.size < 8 => {
            gen_.push((Op::V(d), Op::I(1i64 << (8 * ty.size))))
        }
        Inst::Set { .. } => gen_.push((Op::V(d), Op::I(2))),
        _ => {}
    }
    kill(facts, d);
    for (a, b) in gen_ {
        add(facts, a, b);
    }
}

fn simplify(facts: &Facts, i: &Inst) -> Option<Inst> {
    match *i {
        Inst::Check {
            a, b, cc: Cond::AE, ..
        } if less(facts, a, b) => None,
        Inst::Check {
            a, b, cc: Cond::A, ..
        } if less_eq(facts, a, b) => None,
        Inst::Bin {
            dst,
            op,
            a,
            b,
            ty,
            mode: Arith::Checked,
        } if !ty.signed && matches!(op, BinK::Add | BinK::Mul | BinK::Sub) => {
            let limit = 1u128 << (8 * ty.size as u32);
            let safe = match op {
                BinK::Sub => less_eq(facts, b, a),
                _ => match (upper(facts, a, 2), upper(facts, b, 2)) {
                    (Some(ua), Some(ub)) if ua > 0 && ub > 0 => {
                        let (x, y) = (ua - 1, ub - 1);
                        let max = if op == BinK::Add {
                            x + y
                        } else {
                            x.saturating_mul(y)
                        };
                        max < limit
                    }
                    _ => false,
                },
            };
            Some(if safe {
                Inst::Bin {
                    dst,
                    op,
                    a,
                    b,
                    ty,
                    mode: Arith::Plain,
                }
            } else {
                i.clone()
            })
        }
        _ => Some(i.clone()),
    }
}

fn intersect(a: &Facts, b: &Facts) -> Facts {
    a.iter().filter(|x| b.contains(x)).copied().collect()
}

fn edge(out: &Facts, term: &Term, succ: BlockId) -> Facts {
    let mut f = out.clone();
    if let Term::Br { cc, a, b, t, f: fb } = *term {
        if t != fb {
            let c = if succ == t { cc } else { cc.negate() };
            if let Some((x, y)) = cond_fact(c, a, b) {
                add(&mut f, x, y);
            }
        }
    }
    f
}

pub fn eliminate(f: &mut Func) -> bool {
    let order = layout(f);
    let n = f.blocks.len();
    let mut preds: Vec<Vec<BlockId>> = vec![Vec::new(); n];
    for &b in &order {
        for s in f.blocks[b].term.successors() {
            if !preds[s].contains(&b) {
                preds[s].push(b);
            }
        }
    }
    let mut out: Vec<Option<Facts>> = vec![None; n];
    let mut inn: Vec<Option<Facts>> = vec![None; n];
    let mut changed = true;
    let mut rounds = 0;
    while changed && rounds < 50 {
        changed = false;
        rounds += 1;
        for &b in &order {
            let new_in = if b == 0 {
                Vec::new()
            } else {
                let mut acc: Option<Facts> = None;
                for &p in &preds[b] {
                    if let Some(o) = &out[p] {
                        let e = edge(o, &f.blocks[p].term, b);
                        acc = Some(match acc {
                            None => e,
                            Some(a) => intersect(&a, &e),
                        });
                    }
                }
                acc.unwrap_or_default()
            };
            let mut facts = new_in.clone();
            for i in &f.blocks[b].insts {
                transfer(&mut facts, i);
            }
            if inn[b].as_ref() != Some(&new_in) || out[b].as_ref() != Some(&facts) {
                inn[b] = Some(new_in);
                out[b] = Some(facts);
                changed = true;
            }
        }
    }

    let mut any = false;
    for &b in &order {
        let mut facts = inn[b].clone().unwrap_or_default();
        let insts = take(&mut f.blocks[b].insts);
        let mut kept = Vec::with_capacity(insts.len());
        for i in insts {
            let new = simplify(&facts, &i);
            match &new {
                None => any = true,
                Some(n) => {
                    if !same(n, &i) {
                        any = true;
                    }
                }
            }
            transfer(&mut facts, &i);
            kept.extend(new);
        }
        f.blocks[b].insts = kept;
    }
    any
}

fn same(a: &Inst, b: &Inst) -> bool {
    match (a, b) {
        (Inst::Bin { mode: m1, .. }, Inst::Bin { mode: m2, .. }) => m1 == m2,
        _ => true,
    }
}

pub fn hoist_checks(f: &mut Func) -> bool {
    let inside = loop_blocks(f);
    let n = f.blocks.len();
    let mut preds: Vec<Vec<BlockId>> = vec![Vec::new(); n];
    let order = layout(f);
    for &b in &order {
        for s in f.blocks[b].term.successors() {
            preds[s].push(b);
        }
    }
    let mut changed = false;
    for h in 0..n {
        let Term::Br { cc, a, b, t, f: fb } = f.blocks[h].term.clone() else {
            continue;
        };
        if !inside[h] || !f.blocks[h].insts.is_empty() {
            continue;
        }
        let body = if inside[t] && !inside[fb] {
            t
        } else if inside[fb] && !inside[t] {
            fb
        } else {
            continue;
        };
        if preds[body] != [h] {
            continue;
        }
        let mut in_loop = vec![false; n];
        in_loop[h] = true;
        let mut work: Vec<BlockId> = preds[h].iter().copied().filter(|&p| inside[p]).collect();
        while let Some(x) = work.pop() {
            if in_loop[x] {
                continue;
            }
            in_loop[x] = true;
            work.extend(preds[x].iter().copied());
        }
        if !in_loop[body] {
            continue;
        }
        let outside: Vec<BlockId> = preds[h].iter().copied().filter(|&p| !in_loop[p]).collect();
        if outside.len() != 1 || !matches!(f.blocks[outside[0]].term, Term::Jmp(x) if x == h) {
            continue;
        }
        let pre = outside[0];
        let mut defs: HashMap<VReg, u32> = HashMap::new();
        let mut writes_memory = false;
        for (x, block) in f.blocks.iter().enumerate() {
            if in_loop[x] {
                for i in &block.insts {
                    if let Some(d) = i.def() {
                        *defs.entry(d).or_default() += 1;
                    }
                    writes_memory |= !i.is_pure()
                        && !matches!(i, Inst::Check { .. } | Inst::Bin { .. } | Inst::Neg { .. });
                }
            }
        }
        let header_uses = f.blocks[h].term.uses();
        let mut moved: HashSet<VReg> = HashSet::new();
        let mut hoisted = Vec::new();
        let mut k = 0;
        while k < f.blocks[body].insts.len() {
            let i = &f.blocks[body].insts[k];
            let invariant = |o: &Op| match o {
                Op::I(_) => true,
                Op::V(v) => !defs.contains_key(v) || moved.contains(v),
            };
            let ops_invariant = i.uses().iter().all(invariant);
            let movable = match i {
                Inst::Check { .. } => ops_invariant,
                Inst::Load { .. } => ops_invariant && !writes_memory,
                _ => ops_invariant && i.is_pure(),
            };
            let def_ok = i
                .def()
                .is_none_or(|d| defs.get(&d) == Some(&1) && !header_uses.contains(&Op::V(d)));
            if movable && def_ok {
                let i = f.blocks[body].insts.remove(k);
                if let Some(d) = i.def() {
                    moved.insert(d);
                }
                hoisted.push(i);
            } else if i.is_pure() {
                k += 1;
            } else {
                break;
            }
        }
        if !hoisted.iter().any(|i| matches!(i, Inst::Check { .. })) {
            let body_insts = take(&mut f.blocks[body].insts);
            f.blocks[body].insts = hoisted.into_iter().chain(body_insts).collect();
            continue;
        }
        if hoisted.is_empty() {
            continue;
        }
        let guard = f.blocks.len();
        f.blocks.push(Block {
            insts: hoisted,
            term: Term::Jmp(h),
        });
        let into_body = if body == t { (guard, h) } else { (h, guard) };
        f.blocks[pre].term = Term::Br {
            cc,
            a,
            b,
            t: into_body.0,
            f: into_body.1,
        };
        changed = true;
        break;
    }
    changed
}
