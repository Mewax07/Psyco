use std::collections::HashSet;

use super::*;

pub(super) fn fn_refs(f: &Function, out: &mut HashSet<String>) {
    block(&f.body, out);
}

fn block(b: &Block, out: &mut HashSet<String>) {
    for s in &b.stmts {
        stmt(s, out);
    }
}

fn stmt(s: &Stmt, out: &mut HashSet<String>) {
    match s {
        Stmt::Let { value, .. } => {
            if let Some(v) = value {
                expr(v, out);
            }
        }
        Stmt::Assign { target, value, .. } => {
            expr(target, out);
            expr(value, out);
        }
        Stmt::Expr(e) => expr(e, out),
        Stmt::Return { value, .. } => {
            if let Some(v) = value {
                expr(v, out);
            }
        }
        Stmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            expr(cond, out);
            block(then_block, out);
            if let Some(b) = else_block {
                block(b, out);
            }
        }
        Stmt::While { cond, body, .. } => {
            expr(cond, out);
            block(body, out);
        }
        Stmt::Loop { body, .. } => block(body, out),
        Stmt::For {
            start, end, body, ..
        } => {
            expr(start, out);
            expr(end, out);
            block(body, out);
        }
        Stmt::Break(_) | Stmt::Continue(_) => {}
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            expr(scrutinee, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    expr(g, out);
                }
                block(&arm.body, out);
            }
        }
        Stmt::IfLet {
            value,
            then_block,
            else_block,
            ..
        } => {
            expr(value, out);
            block(then_block, out);
            if let Some(b) = else_block {
                block(b, out);
            }
        }

        Stmt::LetElse { value, else_block, .. } => {
            expr(value, out);
            block(else_block, out);
        }
        Stmt::WhileLet { value, body, .. } => {
            expr(value, out);
            block(body, out);
        }
        Stmt::Block(b) => block(b, out),
    }
}

fn expr(e: &Expr, out: &mut HashSet<String>) {
    match &e.kind {
        ExprKind::Var(n) => {
            out.insert(n.clone());
        }
        ExprKind::Call {
            callee,
            args,
            target,
        } => {
            if let CallTarget::Direct(n) = target {
                out.insert(n.clone());
            }
            expr(callee, out);
            for a in args {
                expr(a, out);
            }
        }
        ExprKind::Unary { operand, .. } => expr(operand, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            expr(lhs, out);
            expr(rhs, out);
        }
        ExprKind::Cast { expr: x, .. } => expr(x, out),
        ExprKind::Field { base, .. } => expr(base, out),
        ExprKind::Index { base, index } => {
            expr(base, out);
            expr(index, out);
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, v, _) in fields {
                expr(v, out);
            }
        }
        ExprKind::ArrayLit(items) => {
            for i in items {
                expr(i, out);
            }
        }
        ExprKind::ArrayRepeat { value, .. } => expr(value, out),
        ExprKind::Range { base, start, end } => {
            expr(base, out);
            for x in [start, end].into_iter().flatten() {
                expr(x, out);
            }
        }
        ExprKind::Unsize(x) | ExprKind::Try(x) => expr(x, out),
        ExprKind::TryOp { inner, .. } => expr(inner, out),
        ExprKind::VariantLit { args, .. } => {
            for a in args {
                expr(a, out);
            }
        }
        _ => {}
    }
}
