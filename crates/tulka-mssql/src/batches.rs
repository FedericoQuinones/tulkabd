//! Splits a script into batches on `GO` lines, like `sqlcmd` and SSMS.

/// One batch of a script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    pub text: String,
    /// Line of the editor text where the batch starts (from 1).
    pub start_line: u32,
}

/// Lexical state carried from one line to the next.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Normal,
    /// Inside `'...'`.
    String,
    /// Inside `[...]`.
    Bracket,
    /// Inside `"..."` (a quoted identifier).
    Quoted,
    /// Inside `/* ... */`; T-SQL block comments nest, so this is the depth.
    Block(u32),
}

/// Splits `sql` into batches.
///
/// `GO` separates batches only when it is alone on its line (any case, with
/// optional surrounding blanks and an optional trailing `--` comment) and the
/// line does not start inside a string, a bracketed or quoted identifier or a
/// block comment. Blank batches are dropped. `GO n` is not supported: such a
/// line stays in the batch text.
pub fn split_batches(sql: &str) -> Vec<Batch> {
    let mut batches = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut start_line = 1u32;
    let mut state = State::Normal;

    for (i, line) in sql.split('\n').enumerate() {
        let line_no = i as u32 + 1;
        if state == State::Normal && is_go_line(line) {
            push_batch(&mut batches, &current, start_line);
            current.clear();
            start_line = line_no + 1;
            continue;
        }
        state = scan_line(line, state);
        current.push(line);
    }
    push_batch(&mut batches, &current, start_line);
    batches
}

fn push_batch(batches: &mut Vec<Batch>, lines: &[&str], start_line: u32) {
    let text = lines.join("\n");
    if !text.trim().is_empty() {
        batches.push(Batch { text, start_line });
    }
}

fn is_go_line(line: &str) -> bool {
    let line = line.trim();
    let Some(go) = line.get(..2) else {
        return false;
    };
    if !go.eq_ignore_ascii_case("go") {
        return false;
    }
    let rest = &line[2..];
    rest.is_empty() || rest.starts_with("--") || {
        let after = rest.trim_start();
        after.len() < rest.len() && (after.is_empty() || after.starts_with("--"))
    }
}

/// Returns the state at the end of `line`, given the state at its start.
fn scan_line(line: &str, mut state: State) -> State {
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        state = match state {
            State::Normal => match c {
                '\'' => State::String,
                '[' => State::Bracket,
                '"' => State::Quoted,
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    State::Block(1)
                }
                // A line comment hides the rest of the line.
                '-' if chars.peek() == Some(&'-') => return State::Normal,
                _ => State::Normal,
            },
            // A doubled closing quote is an escape; treating it as close and
            // reopen gives the same state at the end of the line.
            State::String if c == '\'' => State::Normal,
            State::Bracket if c == ']' => State::Normal,
            State::Quoted if c == '"' => State::Normal,
            State::Block(depth) => match c {
                '*' if chars.peek() == Some(&'/') => {
                    chars.next();
                    if depth == 1 {
                        State::Normal
                    } else {
                        State::Block(depth - 1)
                    }
                }
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    State::Block(depth + 1)
                }
                _ => State::Block(depth),
            },
            other => other,
        };
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_go_lines() {
        let b = split_batches("SELECT 1\nGO\nSELECT 2\n go  \nSELECT 3");
        assert_eq!(
            b.iter().map(|x| x.text.trim()).collect::<Vec<_>>(),
            ["SELECT 1", "SELECT 2", "SELECT 3"]
        );
        assert_eq!(
            b.iter().map(|x| x.start_line).collect::<Vec<_>>(),
            [1, 3, 5]
        );
    }
    #[test]
    fn ignores_go_inside_strings_and_comments() {
        let sql = "SELECT 'a\nGO\nb'\n/*\nGO\n*/\nSELECT [x\nGO]";
        assert_eq!(split_batches(sql).len(), 1);
    }
    #[test]
    fn go_with_trailing_comment_splits() {
        assert_eq!(split_batches("SELECT 1\nGO -- fin\nSELECT 2").len(), 2);
    }
    #[test]
    fn drops_empty_batches() {
        assert_eq!(split_batches("GO\n\nGO\nSELECT 1\nGO\n").len(), 1);
    }
    #[test]
    fn word_go_in_identifier_does_not_split() {
        assert_eq!(split_batches("SELECT GOAL FROM t\nWHERE x = 1").len(), 1);
    }
    #[test]
    fn go_count_is_not_a_separator() {
        let b = split_batches("SELECT 1\nGO 5\nSELECT 2");
        assert_eq!(b.len(), 1);
        assert!(b[0].text.contains("GO 5"));
    }
    #[test]
    fn nested_block_comments_and_quoted_identifiers() {
        assert_eq!(split_batches("/* a /* b */\nGO\n*/ SELECT 1").len(), 1);
        assert_eq!(split_batches("SELECT \"x\nGO\ny\"").len(), 1);
        assert_eq!(split_batches("SELECT 'it''s'\nGO\nSELECT 2").len(), 2);
    }
    #[test]
    fn line_comment_hides_quotes_and_crlf_is_accepted() {
        let b = split_batches("SELECT 1 -- don't\r\nGO\r\nSELECT 2\r\n");
        assert_eq!(b.len(), 2);
        assert_eq!(b[1].start_line, 3);
        assert!(split_batches("GO--x\nSELECT 1").len() == 1);
    }
}
