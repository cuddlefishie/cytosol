use std::iter::Peekable;

use thiserror::Error;

use cytosol_syntax::{
    Binding, BindingAttribute, Expression, Extern, File, FileId, Gene, GeneStatement, HasFC,
    Identifier, InfixOperator, Literal, PrefixOperator, Product, Record, Rule, Type, FC,
};

use crate::{lexer::TokenKind, Token};

/// The context in which an error appeared in.
#[derive(Default, Debug, Clone, Copy)]
pub struct ErrorContext {
    /// The file context and description of item which is currently being
    /// parsed. This is so that errors can reference back to the start
    /// and give more context in the error message.
    pub start: Option<(FC, &'static str)>,
    /// The description of the type of item that is being parsed.
    pub while_parsing: &'static str,
    /// What was expected when the error appeared.
    pub expected: Option<&'static str>,
}

const CTX: ErrorContext = ErrorContext {
    start: None,
    while_parsing: "",
    expected: None,
};

impl ErrorContext {
    pub fn start(mut self, fc: FC, reference_desc: &'static str) -> Self {
        self.start = Some((fc, reference_desc));
        self
    }
    pub fn while_parsing(mut self, while_parsing: &'static str) -> Self {
        self.while_parsing = while_parsing;
        self
    }
    pub fn expected(mut self, expected: &'static str) -> Self {
        self.expected = Some(expected);
        self
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("Unexpected token at {:?}", .0)]
    UnexpectedToken(FC, ErrorContext),
    #[error("Unexpected end")]
    UnexpectedEnd(FileId, ErrorContext),
}

type Result<T> = core::result::Result<T, Error>;

/// Parse a list of tokens into the [`File`](cytosol_syntax::types::File) AST.
///
/// All tokens should be from the same file, so should have the same FileId.
/// The `file` parameter is passed so that the FileID can be used even when the
/// list of tokens is empty.
pub fn parse_file<'src>(file: FileId, tokens: impl Iterator<Item = Token<'src>>) -> Result<File> {
    let mut p = Parser {
        file,
        toks: tokens.peekable(),
    };
    p.parse_file()
}

struct Parser<'src, I: Iterator<Item = Token<'src>>> {
    file: FileId,
    toks: Peekable<I>,
}

impl<'src, I: Iterator<Item = Token<'src>>> Parser<'src, I> {
    fn parse_file(&mut self) -> Result<File> {
        let mut file = File::default();

        while let Some(t) = self.peek() {
            match t.kind {
                TokenKind::Record => {
                    let start_tok = self.next().unwrap();
                    let t = &start_tok;
                    let ec = CTX
                        .start(start_tok.fc, "record definition")
                        .while_parsing("a record definition");

                    let name = self.parse_identifier(ec)?;

                    let (fc, fields) = if self.peek_kind(|t| t == &TokenKind::ParenOpen) {
                        self.grouped_separated(
                            (TokenKind::ParenOpen, TokenKind::ParenClose),
                            ec.start(t.fc, "field list")
                                .while_parsing("the field list of a record item")
                                .expected("`(`"),
                            TokenKind::Comma,
                            ec.start(t.fc, "field list")
                                .while_parsing("the field list of a record item")
                                .expected("`,` or `)`"),
                            |s| {
                                let ident =
                                    s.parse_identifier(ec.while_parsing("a record field"))?;
                                let (colon_fc, _) = s.expect_tok_and_fc(
                                    ec.while_parsing("a record field").expected("`:`"),
                                    |t| matches!(t.kind, TokenKind::Colon),
                                )?;
                                let ty = s.parse_type(ec.start(colon_fc, "beginning of type"))?;
                                Ok((ident, ty))
                            },
                        )?
                    } else {
                        (name.fc(), vec![])
                    };

                    file.records.push(Record {
                        fc: start_tok.fc.merge(fc),
                        name,
                        fields,
                    });
                }
                TokenKind::Extern => {
                    let start_tok = self.next().unwrap();
                    let ec = CTX
                        .start(start_tok.fc, "extern item")
                        .while_parsing("an extern item");

                    let name = self.parse_identifier(ec)?;

                    let (fc, params) = self.grouped_separated(
                        (TokenKind::ParenOpen, TokenKind::ParenClose),
                        ec.while_parsing("the parameter list of an extern item")
                            .expected("`(`"),
                        TokenKind::Comma,
                        ec.while_parsing("the parameter list of an extern item")
                            .expected("`,` or `)`"),
                        |s| {
                            let ident =
                                s.parse_identifier(ec.while_parsing("an extern item parameter"))?;
                            let (colon_fc, _) = s.expect_tok_and_fc(
                                ec.while_parsing("an extern parameter description")
                                    .expected("`:`"),
                                |t| matches!(t.kind, TokenKind::Colon),
                            )?;
                            let ty = s.parse_type(ec.start(colon_fc, "beginning of type"))?;
                            Ok((ident, ty))
                        },
                    )?;

                    let fc = start_tok.fc.merge(fc);
                    file.externs.push(Extern {
                        fc,
                        name,
                        parameters: params,
                    });
                }
                TokenKind::Gene => {
                    let start_tok = self.next().unwrap();
                    let ec = CTX
                        .start(start_tok.fc, "gene item")
                        .while_parsing("a gene item");

                    let (_, factors) = self.grouped_separated(
                        (TokenKind::ParenOpen, TokenKind::ParenClose),
                        ec.while_parsing("a gene factor list").expected("`(`"),
                        TokenKind::Comma,
                        ec.while_parsing("a gene factor list")
                            .expected("`,` or `)`"),
                        |s| s.parse_binding(ec),
                    )?;

                    let next = {
                        let file = self.file;
                        self.peek().ok_or_else({
                            || Error::UnexpectedEnd(file, ec.while_parsing("gene item"))
                        })?
                    };

                    let when = match &next.kind {
                        TokenKind::When => {
                            let next = self.next().unwrap();
                            let wec = ec
                                .while_parsing("a when clause")
                                .start(next.fc, "when clause");

                            let expr = self.parse_expression(wec)?;

                            Some(expr)
                        }
                        _ => None,
                    };

                    let (end_fc, stmts) = self.grouped(
                        (TokenKind::BraceOpen, TokenKind::BraceClose),
                        ec.while_parsing("a gene statement list").expected("`{`"),
                        |s| s.parse_gene_statement(ec),
                    )?;

                    let fc = start_tok.fc.merge(end_fc);

                    file.genes.push(Gene {
                        fc,
                        factors,
                        when,
                        body: stmts,
                    });
                }
                TokenKind::Rule => {
                    let start_tok = self.next().unwrap();
                    let ec = CTX
                        .start(start_tok.fc, "rule item")
                        .while_parsing("a rule item");

                    let (_, reactants) = self.grouped_separated(
                        (TokenKind::ParenOpen, TokenKind::ParenClose),
                        ec.while_parsing("a rule reactant list").expected("`(`"),
                        TokenKind::Comma,
                        ec.while_parsing("a rule reactant list")
                            .expected("`,` or `)`"),
                        |s| s.parse_binding(ec),
                    )?;

                    self.expect(
                        ec.while_parsing("a rule reaction description")
                            .expected("`->`"),
                        |t| t.kind == TokenKind::ArrowR,
                    )?;

                    let (product_fc, products) = self.parse_product_list(ec)?;

                    let (when, end_fc) = match self.peek() {
                        Some(Token {
                            kind: TokenKind::When,
                            ..
                        }) => {
                            let next = self.next().unwrap();
                            let wec = ec
                                .while_parsing("a when clause")
                                .start(next.fc, "when clause");

                            let expr = self.parse_expression(wec)?;

                            let fc = expr.fc();

                            (Some(expr), fc)
                        }
                        _ => (None, product_fc),
                    };

                    let fc = start_tok.fc.merge(end_fc);
                    file.rules.push(Rule {
                        fc,
                        reactants,
                        products,
                        when,
                    });
                }
                _ => {
                    return Err(Error::UnexpectedToken(
                        t.fc,
                        CTX.while_parsing("a top level item")
                            .expected("`record`, `gene`, `rule` or `extern`"),
                    ))
                }
            }
        }

        Ok(file)
    }

    fn parse_gene_statement(&mut self, pec: ErrorContext) -> Result<GeneStatement> {
        let file = self.file;
        let next = self
            .peek()
            .ok_or_else(|| Error::UnexpectedEnd(file, pec.while_parsing("a gene statement")))?;

        match next.kind {
            TokenKind::Call => {
                let call_tok = self.next().unwrap();
                let ec = CTX.start(call_tok.fc, "call statement");
                let name = self.parse_identifier(ec.while_parsing("a call statement"))?;
                let (end_fc, arguments) = self.grouped_separated(
                    (TokenKind::ParenOpen, TokenKind::ParenClose),
                    ec.while_parsing("a call statement parameter list")
                        .expected("`(`"),
                    TokenKind::Comma,
                    ec.while_parsing("a call statement parameter list")
                        .expected("`,` or `)`"),
                    |s| {
                        let name = s.parse_identifier(ec.while_parsing("a named argument"))?;
                        let (colon_fc, _) = s.expect_tok_and_fc(
                            ec.while_parsing("a named argument").expected("`:`"),
                            |t| matches!(t.kind, TokenKind::Colon),
                        )?;
                        let val = s.parse_expression(
                            CTX.start(colon_fc, "beginning of expression")
                                .while_parsing("an expression"),
                        )?;
                        Ok((name, val))
                    },
                )?;
                let fc = call_tok.fc.merge(end_fc);
                Ok(GeneStatement::Call {
                    fc,
                    name,
                    arguments,
                })
            }
            TokenKind::Express => {
                let expr_tok = self.next().unwrap();
                let prod = self.parse_product(
                    CTX.start(expr_tok.fc, "express statement")
                        .while_parsing("an express statement"),
                )?;
                Ok(GeneStatement::Express(expr_tok.fc, prod))
            }
            _ => Err(Error::UnexpectedToken(
                next.fc,
                pec.while_parsing("a gene statement")
                    .expected("`call` or `express`"),
            )),
        }
    }

    fn parse_product_list(&mut self, pec: ErrorContext) -> Result<(FC, Vec<Product>)> {
        let file = self.file;
        let next = self
            .peek()
            .ok_or_else(|| Error::UnexpectedEnd(file, pec.while_parsing("a product list")))?;
        let start_fc = next.fc;

        if next.kind == TokenKind::Nothing {
            let _ = self.next();
            return Ok((start_fc, vec![]));
        }

        self.separated(
            TokenKind::OpPlus,
            pec.while_parsing("a product list"),
            |s| {
                s.parse_product(
                    CTX.start(start_fc, "product list")
                        .while_parsing("a product list"),
                )
            },
        )
    }

    fn parse_binding(&mut self, pec: ErrorContext) -> Result<Binding> {
        let file = self.file;
        let ec = pec.while_parsing("a binding");

        let next = self
            .peek()
            .ok_or_else(|| Error::UnexpectedEnd(file, ec.expected("a quantity or identifier")))?;
        let start_fc = next.fc;

        let ec = ec.start(next.fc, "binding");

        match &next.kind {
            TokenKind::IntegerLiteral(n) => {
                let n = *n;
                let _ = self.next();
                let attr = BindingAttribute::Quantity(start_fc, n);

                let name = self.parse_identifier(ec)?;

                Ok(Binding {
                    fc: start_fc.merge(name.fc()),
                    name,
                    attr: Some(attr),
                })
            }
            TokenKind::Identifier(_) => {
                let id = self.parse_identifier(ec)?;

                if let Some(next) = self.peek() {
                    if next.kind == TokenKind::Colon {
                        let _ = self.next();

                        let name = self.parse_identifier(ec)?;

                        Ok(Binding {
                            fc: id.fc().merge(name.fc()),
                            name,
                            attr: Some(BindingAttribute::Name(id)),
                        })
                    } else {
                        Ok(Binding {
                            fc: id.fc(),
                            name: id,
                            attr: None,
                        })
                    }
                } else {
                    Ok(Binding {
                        fc: id.fc(),
                        name: id,
                        attr: None,
                    })
                }
            }
            _ => Err(Error::UnexpectedToken(
                next.fc,
                pec.while_parsing("a record binding")
                    .expected("a quantity or identifier"),
            )),
        }
    }

    fn parse_type(&mut self, pec: ErrorContext) -> Result<Type> {
        let id = self.parse_identifier(pec.while_parsing("a type"))?;
        Ok(Type::Named(id))
    }

    fn parse_identifier(&mut self, parent_error_context: ErrorContext) -> Result<Identifier> {
        let ctx = parent_error_context.expected("an identifier");

        let (fc, id) = self.expect_tok_and_fc(ctx, |t| {
            if let TokenKind::Identifier(i) = t.kind {
                Some(i)
            } else {
                None
            }
        })?;
        Ok(Identifier(fc, id.to_string()))
    }

    fn parse_product(&mut self, pec: ErrorContext) -> Result<Product> {
        let quantity = if let Some(Token {
            fc,
            kind: TokenKind::IntegerLiteral(l),
        }) = self.peek()
        {
            let fc = *fc;
            let l = *l;
            let _ = self.next();
            Some((fc, l))
        } else {
            None
        };

        let name = self.parse_identifier(pec.while_parsing("a product"))?;

        let start_fc = if let Some((fc, _)) = &quantity {
            *fc
        } else {
            name.fc()
        };

        let ec = CTX.start(start_fc, "product").while_parsing("a product");

        let (fc, fields) = if self.peek_kind(|t| t == &TokenKind::ParenOpen) {
            self.grouped_separated(
                (TokenKind::ParenOpen, TokenKind::ParenClose),
                ec.while_parsing("the start of product fields")
                    .expected("`(`"),
                TokenKind::Comma,
                ec.while_parsing("a product field list")
                    .expected("`,` or `)`"),
                |s| {
                    let name = s.parse_identifier(ec.while_parsing("a product field"))?;
                    let (colon_fc, _) = s.expect_tok_and_fc(
                        ec.while_parsing("a product field").expected("`:`"),
                        |t| t.kind == TokenKind::Colon,
                    )?;
                    let expr = s.parse_expression(
                        CTX.start(colon_fc, "beginning of expression")
                            .while_parsing("an expression"),
                    )?;
                    Ok((name, expr))
                },
            )?
        } else {
            (name.fc(), vec![])
        };

        Ok(Product {
            fc: start_fc.merge(fc),
            quantity,
            name,
            fields,
        })
    }

    fn parse_expression(&mut self, pec: ErrorContext) -> Result<Expression> {
        let mut expr = self.parse_expression_atom(pec)?;

        while let Some(next) = self.peek() {
            let op = match next.kind {
                TokenKind::OpPlus => (next.fc, InfixOperator::Add),
                TokenKind::OpMinus => (next.fc, InfixOperator::Sub),
                TokenKind::OpStar => (next.fc, InfixOperator::Mul),
                TokenKind::OpSlash => (next.fc, InfixOperator::Div),
                TokenKind::OpEquals => (next.fc, InfixOperator::Eq),
                TokenKind::OpNotEquals => (next.fc, InfixOperator::Neq),
                TokenKind::OpLessThan => (next.fc, InfixOperator::Lt),
                TokenKind::OpLessThanEqual => (next.fc, InfixOperator::Lte),
                TokenKind::OpGreaterThan => (next.fc, InfixOperator::Gt),
                TokenKind::OpGreaterThanEqual => (next.fc, InfixOperator::Gte),
                _ => return Ok(expr),
            };

            let _ = self.next();

            let rhs = self.parse_expression_atom(pec)?;

            expr = Expression::InfixOp {
                op,
                args: Box::new([expr, rhs]),
            };
        }

        Ok(expr)
    }

    fn parse_expression_atom(&mut self, pec: ErrorContext) -> Result<Expression> {
        let file = self.file;

        let next = self
            .peek()
            .ok_or_else(|| Error::UnexpectedEnd(file, pec.while_parsing("an expression atom")))?;
        let start_fc = next.fc;

        let mut expr = match &next.kind {
            TokenKind::Identifier(n) => {
                let n = n.to_string();
                let _ = self.next();
                Expression::Variable(Identifier(start_fc, n))
            }
            TokenKind::IntegerLiteral(i) => {
                let i = *i;
                let _ = self.next();
                Expression::Literal(Literal::Integer(start_fc, i))
            }
            TokenKind::StringLiteral(s) => {
                let s = s.clone();
                let _ = self.next();
                Expression::Literal(Literal::String(start_fc, s))
            }
            TokenKind::BracketOpen => {
                let _ = self.next();
                let name = self.parse_identifier(
                    pec.while_parsing("a type inside a concentration expression"),
                )?;
                self.expect(
                    pec.while_parsing("a concentration expression")
                        .expected("`]`"),
                    |t| t.kind == TokenKind::BracketClose,
                )?;
                Expression::Concentration(name)
            }
            TokenKind::ParenOpen => {
                let _ = self.next();
                let val = self.parse_expression(pec)?;
                self.expect(
                    pec.while_parsing("a nested expression").expected("`)`"),
                    |t| t.kind == TokenKind::ParenClose,
                )?;
                val
            }
            TokenKind::OpMinus => {
                let t = self.next().unwrap();
                let rhs = self.parse_expression_atom(pec)?;
                Expression::PrefixOp {
                    op: (t.fc, PrefixOperator::Neg),
                    expr: Box::new(rhs),
                }
            }
            _ => {
                return Err(Error::UnexpectedToken(
                    start_fc,
                    pec.while_parsing("an expression atom"),
                ))
            }
        };

        while let Some(next) = self.peek() {
            if next.kind == TokenKind::Dot {
                let _ = self.next();

                let name = self.parse_identifier(pec.while_parsing("a filed access expression"))?;

                expr = Expression::FieldAccess {
                    base: Box::new(expr),
                    field_name: name,
                };
            } else {
                break;
            }
        }

        Ok(expr)
    }
}

/// Utilities
impl<'src, I: Iterator<Item = Token<'src>>> Parser<'src, I> {
    fn peek(&mut self) -> Option<&Token<'src>> {
        self.toks.peek()
    }

    fn peek_kind(&mut self, f: impl FnOnce(&TokenKind<'src>) -> bool) -> bool {
        if let Some(tok) = self.peek() {
            f(&tok.kind)
        } else {
            false
        }
    }

    fn next(&mut self) -> Option<Token<'src>> {
        self.toks.next()
    }

    fn expect<R: ExpectRet>(
        &mut self,
        context: ErrorContext,
        f: impl FnOnce(&Token<'src>) -> R,
    ) -> Result<R::Out> {
        match self.toks.peek() {
            Some(tok) => match f(tok).into_result(context, tok.fc) {
                Ok(val) => {
                    let _ = self.toks.next();
                    Ok(val)
                }
                Err(err) => Err(err),
            },
            None => Err(Error::UnexpectedEnd(self.file, context)),
        }
    }

    fn expect_tok_and_fc<R: ExpectRet>(
        &mut self,
        context: ErrorContext,
        f: impl FnOnce(&Token<'src>) -> R,
    ) -> Result<(FC, R::Out)> {
        self.expect(context, |tok| {
            f(tok).into_result(context, tok.fc).map(|r| (tok.fc, r))
        })
    }

    fn grouped<T>(
        &mut self,
        delim: (TokenKind<'src>, TokenKind<'src>),
        start_delim_context: ErrorContext,
        mut f: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<(FC, Vec<T>)> {
        let mut vals = vec![];

        let (start_fc, ()) = self.expect_tok_and_fc(start_delim_context, |t| t.kind == delim.0)?;

        loop {
            if self.peek().map(|t| &t.kind) == Some(&delim.1) {
                let end_fc = &self.next().unwrap().fc;
                let fc = start_fc.merge(end_fc);
                return Ok((fc, vals));
            }

            vals.push(f(self)?);
        }
    }

    fn grouped_separated<T>(
        &mut self,
        delim: (TokenKind<'src>, TokenKind<'src>),
        delim_start_context: ErrorContext,
        separator: TokenKind<'src>,
        separator_or_delim_end_context: ErrorContext,
        mut f: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<(FC, Vec<T>)> {
        let mut vals = vec![];

        let (start_fc, ()) = self.expect_tok_and_fc(delim_start_context, |t| t.kind == delim.0)?;

        loop {
            if self.peek().map(|t| &t.kind) == Some(&delim.1) {
                let end_fc = &self.next().unwrap().fc;
                let fc = start_fc.merge(end_fc);
                return Ok((fc, vals));
            }

            vals.push(f(self)?);

            let (fc, end) = self.expect_tok_and_fc(separator_or_delim_end_context, |tok| {
                if tok.kind == delim.1 {
                    Some(true)
                } else if tok.kind == separator {
                    Some(false)
                } else {
                    None
                }
            })?;

            if end {
                let fc = start_fc.merge(fc);
                return Ok((fc, vals));
            }
        }
    }

    fn separated<T: HasFC>(
        &mut self,
        sep: TokenKind<'src>,
        context: ErrorContext,
        mut f: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<(FC, Vec<T>)> {
        let file = self.file;

        let mut vals = vec![];

        let start_fc = self.peek().ok_or(Error::UnexpectedEnd(file, context))?.fc;

        vals.push(f(self)?);

        loop {
            if let Some(t) = self.peek() {
                if t.kind != sep {
                    let last_fc = vals.last().unwrap().fc();
                    let fc = start_fc.merge(last_fc);
                    return Ok((fc, vals));
                } else {
                    let _ = self.next();

                    vals.push(f(self)?);
                }
            } else {
                let last_fc = vals.last().unwrap().fc();
                let fc = start_fc.merge(last_fc);
                return Ok((fc, vals));
            }
        }
    }
}

trait ExpectRet {
    type Out;

    fn into_result(self, context: ErrorContext, fc: FC) -> Result<Self::Out>;
}

impl<T> ExpectRet for Option<T> {
    type Out = T;

    fn into_result(self, context: ErrorContext, fc: FC) -> Result<Self::Out> {
        match self {
            Some(val) => Ok(val),
            None => Err(Error::UnexpectedToken(fc, context)),
        }
    }
}

impl<T> ExpectRet for Result<T> {
    type Out = T;

    fn into_result(self, _: ErrorContext, _: FC) -> Result<Self::Out> {
        self
    }
}

impl ExpectRet for bool {
    type Out = ();

    fn into_result(self, context: ErrorContext, fc: FC) -> Result<Self::Out> {
        match self {
            true => Ok(()),
            false => Err(Error::UnexpectedToken(fc, context)),
        }
    }
}
