//! A minimal, generic S-expression parser for KiCad's `.kicad_pcb` format.
//!
//! KiCad's board file is Lisp-like S-expressions: `(kicad_pcb (version
//! 20241229) (layers (0 "F.Cu" signal) ...) ...)`. This module only knows
//! about *syntax* (lists, quoted strings, bare atoms) — [`crate::parser`]
//! is where KiCad-specific meaning gets attached. Splitting these lets the
//! syntax layer be trivially testable on its own and keeps the semantic
//! layer from having to also worry about escaping/tokenizing.

#[derive(Debug, Clone, PartialEq)]
pub enum Sexpr {
    List(Vec<Sexpr>),
    /// A bare symbol or a de-escaped quoted string — the distinction isn't
    /// preserved, since every caller in `crate::parser` already knows from
    /// context which one to expect.
    Atom(String),
}

impl Sexpr {
    #[must_use]
    pub fn as_list(&self) -> Option<&[Sexpr]> {
        match self {
            Sexpr::List(items) => Some(items),
            Sexpr::Atom(_) => None,
        }
    }

    #[must_use]
    pub fn as_atom(&self) -> Option<&str> {
        match self {
            Sexpr::Atom(s) => Some(s),
            Sexpr::List(_) => None,
        }
    }

    /// For a list whose first element is a symbol (e.g. `(segment ...)`),
    /// that symbol — the element's "tag".
    #[must_use]
    pub fn head(&self) -> Option<&str> {
        self.as_list()?.first()?.as_atom()
    }

    /// The first direct child list tagged `name`, e.g. `find("layer")` on a
    /// `(footprint ... (layer "F.Cu") ...)` list.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&Sexpr> {
        self.as_list()?
            .iter()
            .find(|child| child.head() == Some(name))
    }

    /// All direct child lists tagged `name`, in document order.
    pub fn find_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Sexpr> {
        self.as_list()
            .unwrap_or(&[])
            .iter()
            .filter(move |child| child.head() == Some(name))
    }

    /// The atom at position `index` in this list (position 0 is the tag
    /// itself), e.g. `(net 1 "GND")`'s `atom(1)` is `"1"`, `atom(2)` is
    /// `"GND"`.
    #[must_use]
    pub fn atom(&self, index: usize) -> Option<&str> {
        self.as_list()?.get(index)?.as_atom()
    }
}

#[derive(Debug)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "S-expression parse error: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

/// Parses a single top-level S-expression (KiCad files are exactly one:
/// `(kicad_pcb ...)`). Trailing whitespace after the closing paren is
/// tolerated; anything else after it is an error.
///
/// # Errors
///
/// Returns an error on unterminated lists/strings, an unterminated escape
/// sequence, or non-whitespace content after the root expression closes.
pub fn parse(input: &str) -> Result<Sexpr, ParseError> {
    let mut chars = input.char_indices().peekable();
    let expr = parse_expr(input, &mut chars)?;
    skip_whitespace(&mut chars);
    if chars.peek().is_some() {
        return Err(ParseError("trailing content after root expression".into()));
    }
    Ok(expr)
}

type Chars<'a> = std::iter::Peekable<std::str::CharIndices<'a>>;

fn skip_whitespace(chars: &mut Chars) {
    while let Some(&(_, c)) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else {
            break;
        }
    }
}

fn parse_expr(input: &str, chars: &mut Chars) -> Result<Sexpr, ParseError> {
    skip_whitespace(chars);
    match chars.peek() {
        Some(&(_, '(')) => parse_list(input, chars),
        Some(&(_, '"')) => parse_quoted(chars),
        Some(&(_, _)) => Ok(parse_bare_atom(input, chars)),
        None => Err(ParseError("unexpected end of input".into())),
    }
}

fn parse_list(input: &str, chars: &mut Chars) -> Result<Sexpr, ParseError> {
    chars.next(); // consume '('
    let mut items = Vec::new();
    loop {
        skip_whitespace(chars);
        match chars.peek() {
            Some(&(_, ')')) => {
                chars.next();
                return Ok(Sexpr::List(items));
            }
            Some(_) => items.push(parse_expr(input, chars)?),
            None => return Err(ParseError("unterminated list".into())),
        }
    }
}

fn parse_quoted(chars: &mut Chars) -> Result<Sexpr, ParseError> {
    chars.next(); // consume opening '"'
    let mut s = String::new();
    loop {
        match chars.next() {
            Some((_, '"')) => return Ok(Sexpr::Atom(s)),
            Some((_, '\\')) => match chars.next() {
                Some((_, '"')) => s.push('"'),
                Some((_, '\\')) => s.push('\\'),
                Some((_, other)) => s.push(other),
                None => return Err(ParseError("unterminated escape in string".into())),
            },
            Some((_, c)) => s.push(c),
            None => return Err(ParseError("unterminated string".into())),
        }
    }
}

fn parse_bare_atom(input: &str, chars: &mut Chars) -> Sexpr {
    let start = chars.peek().expect("caller checked non-empty").0;
    let mut end = input.len();
    while let Some(&(i, c)) = chars.peek() {
        if c.is_whitespace() || c == '(' || c == ')' {
            end = i;
            break;
        }
        chars.next();
    }
    Sexpr::Atom(input[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flat_list() {
        let expr = parse("(net 1 \"GND\")").unwrap();
        assert_eq!(expr.head(), Some("net"));
        assert_eq!(expr.atom(1), Some("1"));
        assert_eq!(expr.atom(2), Some("GND"));
    }

    #[test]
    fn parses_nested_lists() {
        let expr = parse("(kicad_pcb (layers (0 \"F.Cu\" signal)))").unwrap();
        let layers_list = expr.find("layers").unwrap();
        let first_entry = layers_list.as_list().unwrap()[1].clone();
        assert_eq!(first_entry.atom(0), Some("0"));
        assert_eq!(first_entry.atom(1), Some("F.Cu"));
        assert_eq!(first_entry.atom(2), Some("signal"));
    }

    #[test]
    fn find_all_returns_every_match_in_order() {
        let expr = parse("(board (net 0 \"\") (net 1 \"A\") (net 2 \"B\"))").unwrap();
        let names: Vec<&str> = expr
            .find_all("net")
            .map(|n| n.atom(2).unwrap_or(""))
            .collect();
        assert_eq!(names, vec!["", "A", "B"]);
    }

    #[test]
    fn handles_empty_quoted_string() {
        let expr = parse("(net 0 \"\")").unwrap();
        assert_eq!(expr.atom(2), Some(""));
    }

    #[test]
    fn handles_escaped_quote_in_string() {
        let expr = parse("(descr \"a \\\"quoted\\\" word\")").unwrap();
        assert_eq!(expr.atom(1), Some("a \"quoted\" word"));
    }

    #[test]
    fn rejects_trailing_content() {
        assert!(parse("(a) (b)").is_err());
    }

    #[test]
    fn rejects_unterminated_list() {
        assert!(parse("(a (b)").is_err());
    }
}
