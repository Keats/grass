use std::mem;

use super::eat_stmts;

use codemap::{Span, Spanned};

use peekmore::{PeekMore, PeekMoreIterator};

use crate::args::{eat_func_args, CallArgs, FuncArgs};
use crate::atrule::AtRule;
use crate::error::SassResult;
use crate::scope::Scope;
use crate::selector::Selector;
use crate::unit::Unit;
use crate::utils::{devour_whitespace, eat_ident, read_until_closing_curly_brace};
use crate::value::{Number, Value};
use crate::{Stmt, Token};

#[derive(Debug, Clone)]
pub(crate) struct Function {
    scope: Scope,
    args: FuncArgs,
    body: Vec<Token>,
    pos: Span,
}

impl PartialEq for Function {
    fn eq(&self, other: &Self) -> bool {
        self.pos == other.pos
    }
}

impl Eq for Function {}

impl Function {
    pub fn new(scope: Scope, args: FuncArgs, body: Vec<Token>, pos: Span) -> Self {
        Function {
            scope,
            args,
            body,
            pos,
        }
    }

    pub fn decl_from_tokens<I: Iterator<Item = Token>>(
        toks: &mut PeekMoreIterator<I>,
        scope: Scope,
        super_selector: &Selector,
        span_before: Span,
    ) -> SassResult<(String, Function)> {
        let Spanned { node: name, span } = eat_ident(toks, &scope, super_selector, span_before)?;
        devour_whitespace(toks);
        let args = match toks.next() {
            Some(Token { kind: '(', .. }) => eat_func_args(toks, &scope, super_selector)?,
            Some(Token { pos, .. }) => return Err(("expected \"(\".", pos).into()),
            None => return Err(("expected \"(\".", span).into()),
        };

        devour_whitespace(toks);

        let mut body = read_until_closing_curly_brace(toks)?; //eat_stmts(toks, &mut scope.clone(), super_selector)?;
        body.push(toks.next().unwrap());
        devour_whitespace(toks);

        Ok((name, Function::new(scope, args, body, span)))
    }

    pub fn args(
        &mut self,
        mut args: CallArgs,
        scope: &Scope,
        super_selector: &Selector,
    ) -> SassResult<()> {
        let mut scope = scope.clone();
        for (idx, arg) in self.args.0.iter_mut().enumerate() {
            if arg.is_variadic {
                let span = args.span();
                let arg_list = Value::ArgList(args.get_variadic(&scope, super_selector)?);
                self.scope.insert_var(
                    arg.name.clone(),
                    Spanned {
                        node: arg_list,
                        span,
                    },
                )?;
                break;
            }
            let val = match args.get(idx, arg.name.clone(), &scope, super_selector) {
                Some(v) => v?,
                None => match arg.default.as_mut() {
                    Some(v) => Value::from_vec(mem::take(v), &scope, super_selector, args.span())?,
                    None => {
                        return Err(
                            (format!("Missing argument ${}.", &arg.name), args.span()).into()
                        )
                    }
                },
            };
            scope.insert_var(arg.name.clone(), val.clone())?;
            self.scope.insert_var(mem::take(&mut arg.name), val)?;
        }
        Ok(())
    }

    pub fn eval_body(&mut self, super_selector: &Selector) -> SassResult<Vec<Spanned<Stmt>>> {
        eat_stmts(
            &mut std::mem::take(&mut self.body).into_iter().peekmore(),
            &mut self.scope,
            super_selector,
            false,
            None,
        )
    }

    pub fn eval(
        mut self,
        args: CallArgs,
        scope: &Scope,
        super_selector: &Selector,
    ) -> SassResult<Value> {
        self.args(args, scope, super_selector)?;
        let stmts = self.eval_body(super_selector)?;
        self.call(super_selector, stmts)?
            .ok_or_else(|| ("Function finished without @return.", self.pos).into())
    }

    pub fn call(
        &mut self,
        super_selector: &Selector,
        stmts: Vec<Spanned<Stmt>>,
    ) -> SassResult<Option<Value>> {
        for stmt in stmts {
            match stmt.node {
                Stmt::AtRule(AtRule::Return(toks)) => {
                    return Ok(Some(
                        Value::from_vec(toks, &self.scope, super_selector, stmt.span)?.node,
                    ));
                }
                Stmt::AtRule(AtRule::For(f)) => {
                    for i in f.iter() {
                        self.scope.insert_var(
                            &f.var.node,
                            Spanned {
                                node: Value::Dimension(Number::from(i), Unit::None),
                                span: f.var.span,
                            },
                        )?;
                        let for_stmts = eat_stmts(
                            &mut f.body.clone().into_iter().peekmore(),
                            &mut self.scope,
                            super_selector,
                            false,
                            None,
                        )?;
                        if let Some(v) = self.call(super_selector, for_stmts)? {
                            return Ok(Some(v));
                        }
                    }
                }
                Stmt::AtRule(AtRule::If(i)) => {
                    let if_stmts = i.eval(&mut self.scope, super_selector, None)?;
                    if let Some(v) = self.call(super_selector, if_stmts)? {
                        return Ok(Some(v));
                    }
                }
                Stmt::AtRule(AtRule::While(w)) => {
                    let scope = &mut self.scope.clone();
                    let mut val =
                        Value::from_vec(w.cond.clone(), scope, super_selector, stmt.span)?;
                    while val.node.is_true(val.span)? {
                        let while_stmts = eat_stmts(
                            &mut w.body.clone().into_iter().peekmore(),
                            scope,
                            super_selector,
                            false,
                            None,
                        )?;
                        if let Some(v) = self.call(super_selector, while_stmts)? {
                            return Ok(Some(v));
                        }
                        val = Value::from_vec(w.cond.clone(), scope, super_selector, val.span)?;
                    }
                }
                Stmt::AtRule(AtRule::Each(..)) => todo!("@each in @function"),
                // todo: multiline comments
                _ => return Err(("This at-rule is not allowed here.", stmt.span).into()),
            }
        }
        Ok(None)
    }
}
