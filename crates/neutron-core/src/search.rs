//! Turn free-form user input into a safe FTS5 MATCH expression.
//!
//! Every token is wrapped in double quotes (with embedded quotes doubled) so
//! FTS5 operators typed by the user (`AND`, `NEAR`, `*`, `:`, parentheses)
//! are treated as literal text. The last token gets a prefix `*` so search
//! works as-you-type.

pub fn fts_query(input: &str) -> Option<String> {
    let tokens: Vec<String> = input
        .split_whitespace()
        .map(|t| t.chars().filter(|c| !c.is_control()).collect::<String>())
        .filter(|t| !t.is_empty())
        .take(16)
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    if tokens.is_empty() {
        return None;
    }
    let mut q = tokens.join(" ");
    q.push('*');
    Some(q)
}

#[cfg(test)]
mod tests {
    use super::fts_query;

    #[test]
    fn quotes_operators() {
        assert_eq!(fts_query("invoice"), Some("\"invoice\"*".into()));
        assert_eq!(fts_query("a OR b"), Some("\"a\" \"OR\" \"b\"*".into()));
        assert_eq!(
            fts_query("say \"hi\""),
            Some("\"say\" \"\"\"hi\"\"\"*".into())
        );
        assert_eq!(fts_query("   "), None);
    }
}
