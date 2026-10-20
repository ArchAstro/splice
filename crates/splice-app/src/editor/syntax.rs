//! Colours for a scene's JSON: keys, strings, numbers, words and marks, one
//! line at a time (JSON strings never span lines).

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    Key,
    Str,
    Number,
    Word,
    Mark,
}

/// The coloured pieces of one line, in order, as byte ranges.
pub fn json(line: &str) -> Vec<(Range<usize>, Token)> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'"' => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                i = (i + 1).min(bytes.len());
                // A string followed by a colon is a key.
                let rest = line[i..].trim_start();
                let kind = if rest.starts_with(':') { Token::Key } else { Token::Str };
                out.push((start..i, kind));
            }
            b'-' | b'0'..=b'9' => {
                let start = i;
                i += 1;
                while i < bytes.len() && matches!(bytes[i], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-') {
                    i += 1;
                }
                out.push((start..i, Token::Number));
            }
            b'a'..=b'z' => {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                    i += 1;
                }
                if matches!(&line[start..i], "true" | "false" | "null") {
                    out.push((start..i, Token::Word));
                }
            }
            b'{' | b'}' | b'[' | b']' | b',' | b':' => {
                out.push((i..i + 1, Token::Mark));
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// Colour strings on the line (`"#ef6a55"`), for swatches beside it.
pub fn colors(line: &str) -> Vec<[u8; 4]> {
    json(line)
        .into_iter()
        .filter(|(_, t)| *t == Token::Str)
        .filter_map(|(r, _)| {
            let text = line[r].trim_matches('"');
            text.starts_with('#')
                .then(|| splice_core::motion::color(text))
                .flatten()
        })
        .collect()
}

/// The line a JSON error names (`… at line 12 column 5`), counting from one.
pub fn error_line(error: &str) -> Option<usize> {
    let at = error.find("line ")?;
    error[at + 5..]
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_a_line() {
        let line = r##"  "fill": "#ef6a55", "x": -12.5, "bold": true, "to": [1, 2]"##;
        let kinds: Vec<(&str, Token)> = json(line).into_iter().map(|(r, t)| (&line[r], t)).collect();
        assert_eq!(kinds[0], ("\"fill\"", Token::Key));
        assert_eq!(kinds[1], (":", Token::Mark));
        assert_eq!(kinds[2], ("\"#ef6a55\"", Token::Str));
        assert!(kinds.contains(&("-12.5", Token::Number)));
        assert!(kinds.contains(&("true", Token::Word)));
        assert_eq!(colors(line), vec![[0xef, 0x6a, 0x55, 255]]);
    }

    #[test]
    fn escaped_quotes_stay_in_the_string() {
        let line = r#""text": "say \"hi\"","#;
        let pieces = json(line);
        assert_eq!(&line[pieces[2].0.clone()], r#""say \"hi\"""#);
    }

    #[test]
    fn finds_the_line_of_an_error() {
        assert_eq!(
            error_line("the scene is not valid: expected `,` at line 12 column 5"),
            Some(12)
        );
        assert_eq!(error_line("layers[0]: fill #zz is not a colour"), None);
    }
}
