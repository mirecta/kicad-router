//! Parses the condition-expression mini-language `.kicad_dru` rules embed
//! as plain text (`crate::dru::Rule::condition`) — e.g. `A.NetClass ==
//! 'DDR4_CMD' && A.fromTo('IC14-*','IC13-*')` — into a structured [`Expr`]
//! AST. Syntax only, same phasing as `crate::dru` itself: **evaluating**
//! an `Expr` against real board items is deliberately not attempted here.
//!
//! `docs/DECISIONS.md` ADR-0002 flags real semantic ambiguity that an
//! evaluator would need to resolve first — what item(s) the `A`/`B`
//! subject actually binds to for a given constraint type, and whether
//! `insideArea` means fully-inside vs. `intersectsArea`'s touches (both
//! predicates are parsed here since ADR-0002 expects both to exist, but
//! neither's semantics are decided by this module). Building that
//! evaluator responsibly means verifying those against real KiCad DRC
//! behaviour, not guessing at it alongside a syntax parser.
//!
//! Grammar (grounded against every condition string in the real
//! `vme-wren.kicad_dru` demo file — see this module's tests):
//!
//! ```text
//! expr       := or_expr
//! or_expr    := and_expr ( '||' and_expr )*
//! and_expr   := predicate ( '&&' predicate )*
//! predicate  := subject '.' 'NetClass' '==' string
//!             | subject '.' name '(' string (',' string)* ')'
//! subject    := identifier   (only "A" observed in practice; "B" is a
//!                             plausible KiCad DRC subject too, per how
//!                             pairwise constraints like clearance name
//!                             their two items, so it's accepted
//!                             syntactically without being guessed at as
//!                             a *new* predicate name)
//! string     := '...'
//! ```
//!
//! No parenthesized grouping or negation (`!`) is supported — neither
//! appears in the grounding file, and inventing a precedence/associativity
//! rule for a form never observed would be exactly the kind of guess this
//! codebase avoids.

#[derive(Debug, Clone, PartialEq)]
pub enum Predicate {
    NetClassEq(String),
    InDiffPair(String),
    IntersectsArea(String),
    InsideArea(String),
    FromTo(String, String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Predicate {
        subject: String,
        predicate: Predicate,
    },
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ParseExprError {
    #[error("unexpected end of expression")]
    UnexpectedEnd,
    #[error("unexpected token '{0}'")]
    UnexpectedToken(String),
    #[error("unknown predicate '{0}'")]
    UnknownPredicate(String),
    #[error("'{0}' expects {1} string argument(s), got {2}")]
    WrongArity(String, usize, usize),
    #[error("trailing content after a complete expression: '{0}'")]
    TrailingContent(String),
}

/// Parses `text` (a `.kicad_dru` rule's `(condition "...")` content) into
/// an [`Expr`].
///
/// # Errors
///
/// Returns an error if `text` isn't valid syntax for the grammar this
/// module documents — an unknown predicate name, a predicate called with
/// the wrong number of string arguments, a missing operator, or anything
/// left over after a complete expression parses.
pub fn parse_condition(text: &str) -> Result<Expr, ParseExprError> {
    let tokens = tokenize(text)?;
    let mut pos = 0;
    let expr = parse_or(&tokens, &mut pos)?;
    if pos != tokens.len() {
        return Err(ParseExprError::TrailingContent(
            tokens[pos..]
                .iter()
                .map(Token::describe)
                .collect::<Vec<_>>()
                .join(" "),
        ));
    }
    Ok(expr)
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Str(String),
    Dot,
    LParen,
    RParen,
    Comma,
    EqEq,
    AndAnd,
    OrOr,
}

impl Token {
    fn describe(&self) -> String {
        match self {
            Token::Ident(s) => s.clone(),
            Token::Str(s) => format!("'{s}'"),
            Token::Dot => ".".to_string(),
            Token::LParen => "(".to_string(),
            Token::RParen => ")".to_string(),
            Token::Comma => ",".to_string(),
            Token::EqEq => "==".to_string(),
            Token::AndAnd => "&&".to_string(),
            Token::OrOr => "||".to_string(),
        }
    }
}

fn tokenize(text: &str) -> Result<Vec<Token>, ParseExprError> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();

    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '.' {
            chars.next();
            tokens.push(Token::Dot);
        } else if c == '(' {
            chars.next();
            tokens.push(Token::LParen);
        } else if c == ')' {
            chars.next();
            tokens.push(Token::RParen);
        } else if c == ',' {
            chars.next();
            tokens.push(Token::Comma);
        } else if c == '=' {
            chars.next();
            if chars.next_if_eq(&'=').is_some() {
                tokens.push(Token::EqEq);
            } else {
                return Err(ParseExprError::UnexpectedToken("=".to_string()));
            }
        } else if c == '&' {
            chars.next();
            if chars.next_if_eq(&'&').is_some() {
                tokens.push(Token::AndAnd);
            } else {
                return Err(ParseExprError::UnexpectedToken("&".to_string()));
            }
        } else if c == '|' {
            chars.next();
            if chars.next_if_eq(&'|').is_some() {
                tokens.push(Token::OrOr);
            } else {
                return Err(ParseExprError::UnexpectedToken("|".to_string()));
            }
        } else if c == '\'' {
            chars.next();
            let mut s = String::new();
            loop {
                match chars.next() {
                    Some('\'') => break,
                    Some(other) => s.push(other),
                    None => return Err(ParseExprError::UnexpectedEnd),
                }
            }
            tokens.push(Token::Str(s));
        } else if c.is_alphanumeric() || c == '_' {
            let mut s = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_alphanumeric() || c == '_' {
                    s.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
            tokens.push(Token::Ident(s));
        } else {
            return Err(ParseExprError::UnexpectedToken(c.to_string()));
        }
    }

    Ok(tokens)
}

fn peek(tokens: &[Token], pos: usize) -> Option<&Token> {
    tokens.get(pos)
}

fn parse_or(tokens: &[Token], pos: &mut usize) -> Result<Expr, ParseExprError> {
    let mut expr = parse_and(tokens, pos)?;
    while peek(tokens, *pos) == Some(&Token::OrOr) {
        *pos += 1;
        let rhs = parse_and(tokens, pos)?;
        expr = Expr::Or(Box::new(expr), Box::new(rhs));
    }
    Ok(expr)
}

fn parse_and(tokens: &[Token], pos: &mut usize) -> Result<Expr, ParseExprError> {
    let mut expr = parse_predicate(tokens, pos)?;
    while peek(tokens, *pos) == Some(&Token::AndAnd) {
        *pos += 1;
        let rhs = parse_predicate(tokens, pos)?;
        expr = Expr::And(Box::new(expr), Box::new(rhs));
    }
    Ok(expr)
}

fn expect(tokens: &[Token], pos: &mut usize, expected: &Token) -> Result<(), ParseExprError> {
    match peek(tokens, *pos) {
        Some(t) if t == expected => {
            *pos += 1;
            Ok(())
        }
        Some(t) => Err(ParseExprError::UnexpectedToken(t.describe())),
        None => Err(ParseExprError::UnexpectedEnd),
    }
}

fn expect_string(tokens: &[Token], pos: &mut usize) -> Result<String, ParseExprError> {
    match peek(tokens, *pos) {
        Some(Token::Str(s)) => {
            let s = s.clone();
            *pos += 1;
            Ok(s)
        }
        Some(t) => Err(ParseExprError::UnexpectedToken(t.describe())),
        None => Err(ParseExprError::UnexpectedEnd),
    }
}

fn parse_predicate(tokens: &[Token], pos: &mut usize) -> Result<Expr, ParseExprError> {
    let subject = match peek(tokens, *pos) {
        Some(Token::Ident(s)) => {
            let s = s.clone();
            *pos += 1;
            s
        }
        Some(t) => return Err(ParseExprError::UnexpectedToken(t.describe())),
        None => return Err(ParseExprError::UnexpectedEnd),
    };
    expect(tokens, pos, &Token::Dot)?;
    let name = match peek(tokens, *pos) {
        Some(Token::Ident(s)) => {
            let s = s.clone();
            *pos += 1;
            s
        }
        Some(t) => return Err(ParseExprError::UnexpectedToken(t.describe())),
        None => return Err(ParseExprError::UnexpectedEnd),
    };

    let predicate = if name == "NetClass" {
        expect(tokens, pos, &Token::EqEq)?;
        Predicate::NetClassEq(expect_string(tokens, pos)?)
    } else {
        expect(tokens, pos, &Token::LParen)?;
        let mut args = vec![expect_string(tokens, pos)?];
        while peek(tokens, *pos) == Some(&Token::Comma) {
            *pos += 1;
            args.push(expect_string(tokens, pos)?);
        }
        expect(tokens, pos, &Token::RParen)?;

        match name.as_str() {
            "inDiffPair" => one_arg(args, &name).map(Predicate::InDiffPair)?,
            "intersectsArea" => one_arg(args, &name).map(Predicate::IntersectsArea)?,
            "insideArea" => one_arg(args, &name).map(Predicate::InsideArea)?,
            "fromTo" => {
                let [a, b] = two_args(args, &name)?;
                Predicate::FromTo(a, b)
            }
            other => return Err(ParseExprError::UnknownPredicate(other.to_string())),
        }
    };

    Ok(Expr::Predicate { subject, predicate })
}

fn one_arg(args: Vec<String>, name: &str) -> Result<String, ParseExprError> {
    let len = args.len();
    let mut args = args;
    if len != 1 {
        return Err(ParseExprError::WrongArity(name.to_string(), 1, len));
    }
    Ok(args.remove(0))
}

fn two_args(args: Vec<String>, name: &str) -> Result<[String; 2], ParseExprError> {
    let len = args.len();
    let mut args = args;
    if len != 2 {
        return Err(ParseExprError::WrongArity(name.to_string(), 2, len));
    }
    let b = args.remove(1);
    let a = args.remove(0);
    Ok([a, b])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn predicate(subject: &str, predicate: Predicate) -> Expr {
        Expr::Predicate {
            subject: subject.to_string(),
            predicate,
        }
    }

    #[test]
    fn parses_a_single_or_of_two_intersects_area_calls() {
        let expr = parse_condition("A.intersectsArea('underFPGA') || A.intersectsArea('underDDR')")
            .unwrap();
        assert_eq!(
            expr,
            Expr::Or(
                Box::new(predicate(
                    "A",
                    Predicate::IntersectsArea("underFPGA".to_string())
                )),
                Box::new(predicate(
                    "A",
                    Predicate::IntersectsArea("underDDR".to_string())
                )),
            )
        );
    }

    #[test]
    fn parses_in_diff_pair_wildcard() {
        let expr = parse_condition("A.inDiffPair('*')").unwrap();
        assert_eq!(expr, predicate("A", Predicate::InDiffPair("*".to_string())));
    }

    #[test]
    fn parses_net_class_equality_with_trailing_whitespace() {
        let expr = parse_condition("A.NetClass == 'zse_50r' ").unwrap();
        assert_eq!(
            expr,
            predicate("A", Predicate::NetClassEq("zse_50r".to_string()))
        );
    }

    #[test]
    fn parses_net_class_and_from_to_with_spacing_variations() {
        let expr =
            parse_condition("A.NetClass == 'DDR4_CMD' && A.fromTo('IC14-*','IC13-*' )").unwrap();
        assert_eq!(
            expr,
            Expr::And(
                Box::new(predicate(
                    "A",
                    Predicate::NetClassEq("DDR4_CMD".to_string())
                )),
                Box::new(predicate(
                    "A",
                    Predicate::FromTo("IC14-*".to_string(), "IC13-*".to_string())
                )),
            )
        );
    }

    #[test]
    fn parses_every_real_condition_string_in_the_vme_wren_demo_file() {
        let conditions = [
            "A.intersectsArea('underFPGA') || A.intersectsArea('underDDR')",
            "A.inDiffPair('*')",
            "A.NetClass == 'zse_50r' ",
            "A.NetClass == 'DDR4_CMD' && A.fromTo('IC14-*','IC13-*' )",
            "A.NetClass == 'DDR4_CMD' && A.fromTo('IC13-*','IC5-*')",
            "A.NetClass == 'DDR4_BYTE0' && A.fromTo('IC14-*','IC13-*' )",
            "A.NetClass == 'DDR4_BYTE1' && A.fromTo('IC14-*','IC13-*' )",
            "A.NetClass == 'DDR4_BYTE2' && A.fromTo('IC14-*','IC5-*' )",
            "A.NetClass == 'DDR4_BYTE3' && A.fromTo('IC14-*','IC5-*' )",
        ];
        for condition in conditions {
            parse_condition(condition)
                .unwrap_or_else(|e| panic!("failed to parse {condition:?}: {e}"));
        }
    }

    #[test]
    fn accepts_insidearea_even_though_unobserved_in_the_demo_file() {
        // ADR-0002 expects this predicate to exist alongside
        // intersectsArea; not seeing it in the one grounding file doesn't
        // mean rejecting it here.
        let expr = parse_condition("A.insideArea('BuckStage')").unwrap();
        assert_eq!(
            expr,
            predicate("A", Predicate::InsideArea("BuckStage".to_string()))
        );
    }

    #[test]
    fn accepts_subject_b_syntactically() {
        let expr = parse_condition("B.NetClass == 'GND'").unwrap();
        assert_eq!(
            expr,
            predicate("B", Predicate::NetClassEq("GND".to_string()))
        );
    }

    #[test]
    fn rejects_an_unknown_predicate_name() {
        let err = parse_condition("A.someNewThing('x')").unwrap_err();
        assert_eq!(
            err,
            ParseExprError::UnknownPredicate("someNewThing".to_string())
        );
    }

    #[test]
    fn rejects_from_to_called_with_one_argument() {
        let err = parse_condition("A.fromTo('IC14-*')").unwrap_err();
        assert_eq!(err, ParseExprError::WrongArity("fromTo".to_string(), 2, 1));
    }

    #[test]
    fn rejects_from_to_called_with_three_arguments() {
        let err = parse_condition("A.fromTo('a','b','c')").unwrap_err();
        assert_eq!(err, ParseExprError::WrongArity("fromTo".to_string(), 2, 3));
    }

    #[test]
    fn rejects_dangling_and() {
        assert!(parse_condition("A.inDiffPair('*') &&").is_err());
    }

    #[test]
    fn rejects_trailing_content_after_a_complete_expression() {
        let err = parse_condition("A.inDiffPair('*') A.inDiffPair('*')").unwrap_err();
        assert!(matches!(err, ParseExprError::TrailingContent(_)));
    }

    #[test]
    fn rejects_net_class_missing_equality_operator() {
        assert!(parse_condition("A.NetClass 'x'").is_err());
    }

    #[test]
    fn rejects_unterminated_string_literal() {
        assert!(parse_condition("A.NetClass == 'x").is_err());
    }
}
