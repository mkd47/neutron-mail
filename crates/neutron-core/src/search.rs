//! Search input: filters (`from:`, `to:`, `subject:`, `has:attachment`,
//! `is:unread`, `before:`, `after:` …) plus free text, which becomes a safe
//! FTS5 MATCH expression.
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

/// A parsed search: free text plus filters. Unknown `word:` prefixes are
/// kept as text, so searching for "re:" or a time like "10:30" still works.
#[derive(Debug, Default, PartialEq)]
pub struct Query {
    pub text: String,
    pub from: Vec<String>,
    pub to: Vec<String>,
    pub subject: Vec<String>,
    pub has_attachment: bool,
    pub unread: Option<bool>,
    pub starred: Option<bool>,
    /// Unix time bounds: `after:` is inclusive (start of that local day),
    /// `before:` exclusive (start of that local day).
    pub after: Option<i64>,
    pub before: Option<i64>,
}

impl Query {
    pub fn has_filters(&self) -> bool {
        !self.from.is_empty()
            || !self.to.is_empty()
            || !self.subject.is_empty()
            || self.has_attachment
            || self.unread.is_some()
            || self.starred.is_some()
            || self.after.is_some()
            || self.before.is_some()
    }
}

/// Split into words, keeping `"quoted phrases"` (also after `from:`) whole.
fn words(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in input.chars().filter(|c| !c.is_control()) {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn local_day_start(s: &str) -> Option<i64> {
    use chrono::TimeZone;
    let d = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    chrono::Local
        .from_local_datetime(&d.and_hms_opt(0, 0, 0)?)
        .earliest()
        .map(|t| t.timestamp())
}

pub fn parse(input: &str) -> Query {
    let mut q = Query::default();
    let mut text = Vec::new();
    for w in words(input).into_iter().take(24) {
        let Some((key, value)) = w.split_once(':') else {
            text.push(w);
            continue;
        };
        let v = value.trim().to_string();
        let handled = match (
            key.to_ascii_lowercase().as_str(),
            v.to_ascii_lowercase().as_str(),
        ) {
            (_, "") => false,
            ("from", _) => {
                q.from.push(v.clone());
                true
            }
            ("to" | "cc", _) => {
                q.to.push(v.clone());
                true
            }
            ("subject", _) => {
                q.subject.push(v.clone());
                true
            }
            ("has", "attachment" | "attachments") => {
                q.has_attachment = true;
                true
            }
            ("is", "unread") => {
                q.unread = Some(true);
                true
            }
            ("is", "read") => {
                q.unread = Some(false);
                true
            }
            ("is", "starred" | "flagged") => {
                q.starred = Some(true);
                true
            }
            ("after" | "since", _) => local_day_start(&v).map(|t| q.after = Some(t)).is_some(),
            ("before", _) => local_day_start(&v).map(|t| q.before = Some(t)).is_some(),
            _ => false,
        };
        if !handled {
            text.push(w);
        }
    }
    q.text = text.join(" ");
    q
}

#[cfg(test)]
mod tests {
    use super::{fts_query, parse};

    #[test]
    fn parses_filters_and_keeps_the_rest_as_text() {
        let q = parse(
            r#"from:"Ayşe Yılmaz" to:bob subject:invoice has:attachment is:unread budget 10:30 re:"#,
        );
        assert_eq!(q.from, vec!["Ayşe Yılmaz"]);
        assert_eq!(q.to, vec!["bob"]);
        assert_eq!(q.subject, vec!["invoice"]);
        assert!(q.has_attachment);
        assert_eq!(q.unread, Some(true));
        assert_eq!(q.text, "budget 10:30 re:");
        let d = parse("after:2026-01-01 before:2026-02-01 is:starred");
        assert!(d.after.unwrap() < d.before.unwrap());
        assert_eq!(d.starred, Some(true));
        assert!(d.text.is_empty() && d.has_filters());
        assert_eq!(parse("after:yesterday").text, "after:yesterday");
        assert!(!parse("plain words").has_filters());
    }

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
