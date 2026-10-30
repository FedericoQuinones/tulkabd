//! The T-SQL lexer shared by the guard and the read-only check: it splits a
//! batch into words, quoted names, strings, variables, temp tables, numbers
//! and punctuation, skipping comments.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Word,
    Quoted,
    Str,
    Var,
    Temp,
    Num,
    Punct,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Token {
    pub(crate) kind: Kind,
    pub(crate) start: usize,
    pub(crate) end: usize,
    /// 0-based line within the batch.
    pub(crate) line: u32,
}

pub(crate) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Splits `text` into tokens, skipping comments. `Err` carries the offset of
/// the unclosed string, identifier or comment.
pub(crate) fn lex(text: &str) -> Result<Vec<Token>, usize> {
    let b = text.as_bytes();
    let mut tokens = Vec::with_capacity(text.len() / 4);
    let mut i = 0;
    let mut line = 0u32;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            i += 1;
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'-' && b.get(i + 1) == Some(&b'-') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let open = i;
            let mut depth = 0u32;
            loop {
                if i >= b.len() {
                    return Err(open);
                }
                if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    i += 2;
                } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    if b[i] == b'\n' {
                        line += 1;
                    }
                    i += 1;
                }
            }
        } else if c == b'\'' || c == b'"' || c == b'[' {
            let close = if c == b'[' { b']' } else { c };
            let start = i;
            let start_line = line;
            i += 1;
            loop {
                if i >= b.len() {
                    return Err(start);
                }
                if b[i] == close {
                    if b.get(i + 1) == Some(&close) {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                if b[i] == b'\n' {
                    line += 1;
                }
                i += 1;
            }
            let kind = if c == b'\'' { Kind::Str } else { Kind::Quoted };
            tokens.push(Token {
                kind,
                start,
                end: i,
                line: start_line,
            });
        } else if c == b'@' || c == b'#' {
            let start = i;
            while i < b.len() && (b[i] == c) {
                i += 1;
            }
            i = advance_while(text, i, is_word_char);
            let kind = if c == b'@' { Kind::Var } else { Kind::Temp };
            tokens.push(Token {
                kind,
                start,
                end: i,
                line,
            });
        } else if c.is_ascii_digit() {
            let start = i;
            i = number_end(b, i);
            tokens.push(Token {
                kind: Kind::Num,
                start,
                end: i,
                line,
            });
        } else {
            let ch = text[i..].chars().next().unwrap();
            if is_word_char(ch) {
                let start = i;
                i = advance_while(text, i, is_word_char);
                // `N'...'` is a Unicode string literal.
                if matches!(&text[start..i], "N" | "n") && b.get(i) == Some(&b'\'') {
                    let mut j = i + 1;
                    let start_line = line;
                    loop {
                        if j >= b.len() {
                            return Err(start);
                        }
                        if b[j] == b'\'' {
                            if b.get(j + 1) == Some(&b'\'') {
                                j += 2;
                                continue;
                            }
                            j += 1;
                            break;
                        }
                        if b[j] == b'\n' {
                            line += 1;
                        }
                        j += 1;
                    }
                    i = j;
                    tokens.push(Token {
                        kind: Kind::Str,
                        start,
                        end: i,
                        line: start_line,
                    });
                    continue;
                }
                tokens.push(Token {
                    kind: Kind::Word,
                    start,
                    end: i,
                    line,
                });
            } else {
                i += ch.len_utf8();
                tokens.push(Token {
                    kind: Kind::Punct,
                    start: i - ch.len_utf8(),
                    end: i,
                    line,
                });
            }
        }
    }
    Ok(tokens)
}

/// End of the numeric literal at `i`: digits, an optional fraction and
/// exponent, or hex digits after `0x`. A keyword glued to the number, as in
/// `1DROP`, starts a new token.
fn number_end(b: &[u8], mut i: usize) -> usize {
    if b[i] == b'0' && matches!(b.get(i + 1), Some(b'x' | b'X')) {
        i += 2;
        while i < b.len() && b[i].is_ascii_hexdigit() {
            i += 1;
        }
        return i;
    }
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if b.get(i) == Some(&b'.') && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        if b.get(j).is_some_and(u8::is_ascii_digit) {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    i
}

fn advance_while(text: &str, mut i: usize, f: impl Fn(char) -> bool) -> usize {
    for ch in text[i..].chars() {
        if !f(ch) {
            break;
        }
        i += ch.len_utf8();
    }
    i
}

/// The source text of token `t` within `text`.
pub(crate) fn src<'a>(text: &'a str, t: &Token) -> &'a str {
    &text[t.start..t.end]
}
