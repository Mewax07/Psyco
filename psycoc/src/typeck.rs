use std::collections::HashMap;

use crate::{
    ast::{
        BinOp::{self},
        Block, Expr, ExprKind, Function, Program, Stmt, Type, TypeExpr, UnaryOp,
    },
    lexer::Span,
};

#[derive(Debug)]
pub struct TypeError {
    pub message: String,
    pub span: Span,
}

type TypeResult<T> = Result<T, TypeError>;

struct FnSig {
    params: Vec<Type>,
    ret: Type,
}

pub struct TypeChecker {
    functions: HashMap<String, FnSig>,
    scopes: Vec<HashMap<String, Type>>,
    current_ret: Type,
}

impl TypeChecker {
    pub fn new() -> Self {
        TypeChecker {
            functions: HashMap::new(),
            scopes: Vec::new(),
            current_ret: Type::Unit,
        }
    }

    fn error(&self, message: impl Into<String>, span: Span) -> TypeError {
        TypeError {
            message: message.into(),
            span,
        }
    }

    fn resolve_type(&self, t: &TypeExpr) -> TypeResult<Type> {
        match t.name.as_str() {
            "Int" => Ok(Type::Int),
            "Bool" => Ok(Type::Bool),
            "Str" => Ok(Type::Str),
            other => Err(self.error(format!("unknown type '{}'", other), t.span)),
        }
    }

    fn expect_type(&self, expected: Type, found: Type, span: Span) -> TypeResult<()> {
        if expected == found {
            Ok(())
        } else {
            Err(self.error(format!("expected {:?}, found {:?}", expected, found), span))
        }
    }

    fn binary_type(&self, op: BinOp, left: Type, right: Type, span: Span) -> TypeResult<Type> {
        let result = match (op, left, right) {
            (
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod,
                Type::Int,
                Type::Int,
            ) => Type::Int,
            (
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge,
                Type::Int,
                Type::Int,
            ) => Type::Bool,
            (BinOp::Eq | BinOp::Ne | BinOp::And | BinOp::Or, Type::Bool, Type::Bool) => Type::Bool,
            _ => {
                return Err(self.error(
                    format!("cannot apply {:?} to {:?} and {:?}", op, left, right),
                    span,
                ));
            }
        };
        Ok(result)
    }

    pub fn check_program(&mut self, program: &mut Program) -> TypeResult<()> {
        for f in &program.functions {
            if f.name == "print" || self.functions.contains_key(&f.name) {
                return Err(self.error(format!("function '{}' is already defined", f.name), f.span));
            }
            let params = f
                .params
                .iter()
                .map(|p| self.resolve_type(&p.r#type))
                .collect::<TypeResult<Vec<_>>>()?;
            let ret = match &f.return_type {
                Some(t) => self.resolve_type(t)?,
                None => Type::Unit,
            };
            self.functions.insert(f.name.clone(), FnSig { params, ret });
        }

        let Some(main) = program.functions.iter().find(|f| f.name == "main") else {
            return Err(self.error("no 'main' function", Span { line: 1, col: 1 }));
        };
        if !main.params.is_empty() || main.return_type.is_some() {
            return Err(self.error(
                "'main' must take no parameters and return nothing",
                main.span,
            ));
        }

        for f in &mut program.functions {
            self.check_function(f)?;
        }

        Ok(())
    }

    fn check_function(&mut self, f: &mut Function) -> TypeResult<()> {
        let sig = &self.functions[&f.name];
        let param_types = sig.params.clone();
        self.current_ret = sig.ret;

        let mut frame = HashMap::new();
        for (param, ty) in f.params.iter().zip(param_types) {
            if frame.insert(param.name.clone(), ty).is_some() {
                return Err(self.error(
                    format!("parameter '{}' declared twice", param.name),
                    param.span,
                ));
            }
        }

        self.scopes = vec![frame];
        let always_returns = self.check_block(&mut f.body)?;
        self.scopes.clear();

        if self.current_ret != Type::Unit && !always_returns {
            return Err(self.error(
                format!(
                    "function '{}' must return a {:?} on every path",
                    f.name, self.current_ret
                ),
                f.span,
            ));
        }
        Ok(())
    }

    fn check_block(&mut self, block: &mut Block) -> TypeResult<bool> {
        self.scopes.push(HashMap::new());
        let result = self.check_stmts(&mut block.stmts);
        self.scopes.pop();
        result
    }

    fn check_stmts(&mut self, stmts: &mut [Stmt]) -> TypeResult<bool> {
        let mut returns = false;
        for stmt in stmts {
            returns |= self.check_stmt(stmt)?;
        }
        Ok(returns)
    }

    fn check_stmt(&mut self, stmt: &mut Stmt) -> TypeResult<bool> {
        match stmt {
            Stmt::Let { name, value, .. } => {
                let r#type = self.check_expr(value)?;
                if r#type == Type::Unit {
                    return Err(self.error("cannot store a value of type Unit", value.span));
                }
                self.scopes.last_mut().unwrap().insert(name.clone(), r#type);
                Ok(false)
            }
            Stmt::Return { value, span } => {
                let r#type = match value {
                    Some(e) => self.check_expr(e)?,
                    None => Type::Unit,
                };
                let err_span = value.as_ref().map_or(*span, |e| e.span);
                self.expect_type(self.current_ret, r#type, err_span)?;
                Ok(true)
            }
            Stmt::While { cond, body, .. } => {
                let r#type = self.check_expr(cond)?;
                self.expect_type(Type::Bool, r#type, cond.span)?;
                self.check_block(body)?;
                Ok(false)
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                let r#type = self.check_expr(cond)?;
                self.expect_type(Type::Bool, r#type, cond.span)?;
                let then_returns = self.check_block(then_block)?;
                let else_returns = match else_block {
                    Some(block) => self.check_block(block)?,
                    None => false,
                };
                Ok(then_returns && else_returns)
            }
            Stmt::Assign { name, value, span } => {
                let Some(var_type) = self.lookup(name) else {
                    return Err(self.error(format!("unknown variable '{name}'"), *span));
                };
                let r#type = self.check_expr(value)?;
                self.expect_type(var_type, r#type, value.span)?;
                Ok(false)
            }
            Stmt::Expr(e) => {
                self.check_expr(e)?;
                Ok(false)
            }
        }
    }

    fn check_expr(&mut self, expr: &mut Expr) -> TypeResult<Type> {
        let span = expr.span;
        let r#type = match &mut expr.kind {
            ExprKind::Int(_) => Type::Int,
            ExprKind::Str(_) => Type::Str,
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::Var(name) => self
                .lookup(name)
                .ok_or_else(|| self.error(format!("unknown variable '{}'", name), span))?,
            ExprKind::Unary { op, operand } => {
                let t = self.check_expr(operand)?;
                match op {
                    UnaryOp::Neg => {
                        self.expect_type(Type::Int, t, operand.span)?;
                        Type::Int
                    }
                    UnaryOp::Not => {
                        self.expect_type(Type::Bool, t, operand.span)?;
                        Type::Bool
                    }
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let left = self.check_expr(lhs)?;
                let right = self.check_expr(rhs)?;
                self.binary_type(*op, left, right, span)?
            }
            ExprKind::Call { callee, args } => {
                let mut arg_types = Vec::with_capacity(args.len());
                for arg in args.iter_mut() {
                    arg_types.push(self.check_expr(arg)?);
                }

                if callee.as_str() == "print" {
                    for (r#type, arg) in arg_types.iter().zip(args.iter()) {
                        if *r#type == Type::Unit {
                            return Err(self.error("cannot print a value of type Unit", arg.span));
                        }
                    }
                    Type::Unit
                } else {
                    let Some(sig) = self.functions.get(callee.as_str()) else {
                        return Err(self.error(format!("unknown function '{}'", callee), span));
                    };
                    if sig.params.len() != arg_types.len() {
                        return Err(self.error(
                            format!(
                                "'{}' expects {} argument(s), got {}",
                                callee,
                                sig.params.len(),
                                arg_types.len()
                            ),
                            span,
                        ));
                    }
                    for ((expected, found), arg) in
                        sig.params.iter().zip(&arg_types).zip(args.iter())
                    {
                        self.expect_type(*expected, *found, arg.span)?;
                    }
                    sig.ret
                }
            }
        };
        expr.r#type = Some(r#type);
        Ok(r#type)
    }

    fn lookup(&self, name: &str) -> Option<Type> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .copied()
    }
}
