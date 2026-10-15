//! Mail rules: conditions, actions, evaluation, safety limits and Sieve
//! (RFC 5228) export.
//!
//! Rules are ordered and global; an `account` condition limits a rule to
//! one account. They run on new INBOX mail during sync and on demand
//! ("Run now"). Rules marked `server_side` are exported to the account's
//! ManageSieve server instead (when it has one and the rule is
//! expressible in Sieve).

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const MAX_RULES: usize = 200;
pub const MAX_CONDITIONS: usize = 20;
pub const MAX_ACTIONS: usize = 10;
pub const MAX_PATTERN_CHARS: usize = 500;
/// Compiled-program size limits: `regex` matches in linear time, so these
/// bound both memory and the time per byte of input.
pub const REGEX_SIZE_LIMIT: usize = 256 * 1024;
pub const REGEX_DFA_LIMIT: usize = 2 * 1024 * 1024;
/// Only this much of the body text is searched.
pub const MAX_BODY_SCAN: usize = 256 * 1024;
/// Automatic forwards allowed per account per hour (storm guard).
pub const MAX_FORWARDS_PER_HOUR: i64 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    From,
    To,
    Cc,
    Subject,
    Body,
    Header,
    Size,
    HasAttachment,
    Account,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Contains,
    Equals,
    Regex,
    StartsWith,
    /// Size only: larger than.
    Greater,
    /// Size only: smaller than.
    Less,
    /// Has attachment only.
    Is,
    IsNot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Condition {
    pub field: Field,
    pub op: Op,
    #[serde(default)]
    pub value: String,
    /// Header name for `Field::Header`.
    #[serde(default)]
    pub header: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Move {
        folder: String,
    },
    Copy {
        folder: String,
    },
    MarkRead,
    Flag,
    Label {
        keyword: String,
    },
    /// Move to Trash.
    Delete,
    Forward {
        to: String,
    },
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// true = all conditions must match, false = any.
    #[serde(default = "yes")]
    pub match_all: bool,
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
    /// Run on the server (Sieve) instead of locally, when possible.
    #[serde(default)]
    pub server_side: bool,
}

fn yes() -> bool {
    true
}

// ---- validation ---------------------------------------------------------

pub fn compile_regex(p: &str) -> Result<regex::Regex> {
    if p.chars().count() > MAX_PATTERN_CHARS {
        return Err(Error::Invalid(format!(
            "regex longer than {MAX_PATTERN_CHARS} characters"
        )));
    }
    regex::RegexBuilder::new(p)
        .case_insensitive(true)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_DFA_LIMIT)
        .nest_limit(50)
        .build()
        .map_err(|e| Error::Invalid(format!("invalid regex: {}", first_line(&e.to_string()))))
}

fn first_line(s: &str) -> String {
    s.lines()
        .last()
        .unwrap_or(s)
        .trim()
        .chars()
        .take(200)
        .collect()
}

/// Parse "1500", "100K", "2M" to bytes.
pub fn parse_size(v: &str) -> Result<u64> {
    let v = v.trim();
    let (num, mul) = match v.chars().last().map(|c| c.to_ascii_uppercase()) {
        Some('K') => (&v[..v.len() - 1], 1024),
        Some('M') => (&v[..v.len() - 1], 1024 * 1024),
        _ => (v, 1),
    };
    num.trim()
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(mul))
        .filter(|n| *n <= 1 << 40)
        .ok_or_else(|| Error::Invalid(format!("\"{v}\" is not a size (e.g. 500K or 2M)")))
}

fn valid_header_name(h: &str) -> bool {
    !h.is_empty() && h.len() <= 76 && h.bytes().all(|b| b.is_ascii_graphic() && b != b':')
}

pub fn valid_address(a: &str) -> bool {
    let a = a.trim();
    a.len() <= 254
        && !a.chars().any(|c| {
            c.is_control() || c.is_whitespace() || matches!(c, '<' | '>' | ',' | '"' | '\\')
        })
        && a.split_once('@')
            .is_some_and(|(l, d)| !l.is_empty() && d.contains('.') && !d.starts_with('.'))
}

pub fn validate(r: &Rule) -> Result<()> {
    let bad = |m: String| Err(Error::Invalid(format!("rule \"{}\": {m}", r.name)));
    if r.name.trim().is_empty()
        || r.name.chars().count() > 120
        || r.name.chars().any(char::is_control)
    {
        return Err(Error::Invalid(
            "every rule needs a name (max 120 characters)".into(),
        ));
    }
    if r.conditions.is_empty() || r.conditions.len() > MAX_CONDITIONS {
        return bad(format!("needs 1 to {MAX_CONDITIONS} conditions"));
    }
    if r.actions.is_empty() || r.actions.len() > MAX_ACTIONS {
        return bad(format!("needs 1 to {MAX_ACTIONS} actions"));
    }
    if r.actions.iter().all(|a| *a == Action::Stop) {
        return bad("needs an action besides \"stop\"".into());
    }
    for c in &r.conditions {
        if c.value.chars().any(|ch| ch == '\r' || ch == '\n') || c.value.chars().count() > 1000 {
            return bad("condition values must be a single line (max 1000 characters)".into());
        }
        let text_op = matches!(c.op, Op::Contains | Op::Equals | Op::Regex | Op::StartsWith);
        match c.field {
            Field::Size => {
                if !matches!(c.op, Op::Greater | Op::Less) {
                    return bad("size uses \"larger than\" or \"smaller than\"".into());
                }
                parse_size(&c.value)?;
            }
            Field::HasAttachment => {
                if !matches!(c.op, Op::Is | Op::IsNot) {
                    return bad("\"has attachment\" uses is / is not".into());
                }
            }
            Field::Header if !valid_header_name(&c.header) => {
                return bad(format!("\"{}\" is not a header name", c.header));
            }
            _ if !text_op => return bad("use contains, equals, starts with or regex".into()),
            _ => {}
        }
        if text_op && c.value.is_empty() {
            return bad("condition values can't be empty".into());
        }
        if c.op == Op::Regex {
            if let Err(e) = compile_regex(&c.value) {
                return bad(e.to_string());
            }
        }
    }
    for a in &r.actions {
        match a {
            Action::Move { folder } | Action::Copy { folder } => {
                if folder.trim().is_empty() || folder.chars().any(char::is_control) {
                    return bad("choose a folder".into());
                }
                if folder.eq_ignore_ascii_case("INBOX") {
                    // Rules run on INBOX: filing into it again would loop.
                    return bad("can't move or copy into the Inbox".into());
                }
            }
            Action::Label { keyword } if !crate::imap::valid_keyword(keyword) => {
                return bad(format!(
                    "\"{keyword}\" is not a valid label (letters, digits, $ _ - .)"
                ));
            }
            Action::Forward { to } if !valid_address(to) => {
                return bad(format!("\"{to}\" is not an email address"));
            }
            _ => {}
        }
    }
    if r.actions
        .iter()
        .filter(|a| matches!(a, Action::Forward { .. }))
        .count()
        > 3
    {
        return bad("at most 3 forwards per rule".into());
    }
    Ok(())
}

pub fn validate_all(rules: &[Rule]) -> Result<()> {
    if rules.len() > MAX_RULES {
        return Err(Error::Invalid(format!("at most {MAX_RULES} rules")));
    }
    rules.iter().try_for_each(validate)
}

// ---- evaluation ---------------------------------------------------------

/// What a rule sees of a message.
#[derive(Debug, Clone, Default)]
pub struct MessageFacts {
    /// Full header text ("Name <addr>, …") and bare addresses.
    pub from: (String, Vec<String>),
    pub to: (String, Vec<String>),
    pub cc: (String, Vec<String>),
    pub subject: String,
    pub body: String,
    /// (lower-case name, decoded value)
    pub headers: Vec<(String, String)>,
    pub size: u64,
    pub has_attachment: bool,
    pub account_email: String,
    pub message_id: String,
    pub auto_submitted: bool,
}

impl MessageFacts {
    pub fn from_raw(raw: &[u8], account_email: &str) -> Self {
        use mail_parser::{HeaderValue, MessageParser};
        let p = crate::parse::parse(raw);
        let msg = MessageParser::default().parse(raw);
        let addrs = |a: Option<&mail_parser::Address<'_>>| -> Vec<String> {
            a.map(|a| {
                a.iter()
                    .filter_map(|x| x.address().map(|e| e.to_ascii_lowercase()))
                    .collect()
            })
            .unwrap_or_default()
        };
        let mut headers = Vec::new();
        let mut auto_submitted = false;
        let (mut from, mut to, mut cc) = (Vec::new(), Vec::new(), Vec::new());
        if let Some(m) = &msg {
            from = addrs(m.from());
            to = addrs(m.to());
            cc = addrs(m.cc());
            for h in m.headers() {
                let name = h.name().to_ascii_lowercase();
                let value = match h.value() {
                    HeaderValue::Text(t) => t.to_string(),
                    HeaderValue::TextList(l) => l.join(", "),
                    _ => m
                        .header_raw(h.name())
                        .map(|s| s.trim().to_string())
                        .unwrap_or_default(),
                };
                if (name == "auto-submitted" && !value.trim().eq_ignore_ascii_case("no"))
                    || name == "x-neutron-rule"
                {
                    auto_submitted = true;
                }
                if headers.len() < 500 {
                    headers.push((name, value.chars().take(4000).collect()));
                }
            }
        }
        let mut body = p.text.clone();
        if body.trim().is_empty() {
            if let Some(h) = &p.html_raw {
                body = crate::parse::strip_tags(h);
            }
        }
        if body.len() > MAX_BODY_SCAN {
            let mut cut = MAX_BODY_SCAN;
            while !body.is_char_boundary(cut) {
                cut -= 1;
            }
            body.truncate(cut);
        }
        let from_text = if p.from_name.is_empty() {
            p.from_addr.clone()
        } else {
            format!("{} <{}>", p.from_name, p.from_addr)
        };
        Self {
            from: (from_text, from),
            to: (p.to.clone(), to),
            cc: (p.cc.clone(), cc),
            subject: p.subject.clone(),
            body,
            headers,
            size: raw.len() as u64,
            has_attachment: !p.attachments.is_empty(),
            account_email: account_email.to_ascii_lowercase(),
            message_id: p.message_id.clone(),
            auto_submitted,
        }
    }
}

fn text_match(op: Op, hay: &str, needle: &str) -> Result<bool> {
    let h = hay.to_lowercase();
    let n = needle.to_lowercase();
    Ok(match op {
        Op::Contains => h.contains(&n),
        Op::Equals => h.trim() == n.trim(),
        Op::StartsWith => h.trim_start().starts_with(&n),
        Op::Regex => compile_regex(needle)?.is_match(hay),
        _ => false,
    })
}

/// Address fields: `contains` searches the whole header (names too);
/// equals / starts with / regex test each address.
fn addr_match(op: Op, (text, addrs): &(String, Vec<String>), v: &str) -> Result<bool> {
    if op == Op::Contains {
        return text_match(op, text, v);
    }
    for a in addrs {
        if text_match(op, a, v)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn condition_matches(c: &Condition, m: &MessageFacts) -> Result<bool> {
    Ok(match c.field {
        Field::From => addr_match(c.op, &m.from, &c.value)?,
        Field::To => addr_match(c.op, &m.to, &c.value)?,
        Field::Cc => addr_match(c.op, &m.cc, &c.value)?,
        Field::Subject => text_match(c.op, &m.subject, &c.value)?,
        Field::Body => text_match(c.op, &m.body, &c.value)?,
        Field::Header => {
            let name = c.header.to_ascii_lowercase();
            let mut any = false;
            for (_, v) in m.headers.iter().filter(|(n, _)| *n == name) {
                if text_match(c.op, v, &c.value)? {
                    any = true;
                    break;
                }
            }
            any
        }
        Field::Size => {
            let n = parse_size(&c.value)?;
            match c.op {
                Op::Greater => m.size > n,
                Op::Less => m.size < n,
                _ => false,
            }
        }
        Field::HasAttachment => match c.op {
            Op::IsNot => !m.has_attachment,
            _ => m.has_attachment,
        },
        Field::Account => text_match(c.op, &m.account_email, &c.value)?,
    })
}

/// Per-condition results and the overall verdict (for "Test").
pub fn explain(r: &Rule, m: &MessageFacts) -> Result<(bool, Vec<bool>)> {
    let each: Vec<bool> = r
        .conditions
        .iter()
        .map(|c| condition_matches(c, m))
        .collect::<Result<_>>()?;
    let hit = if r.match_all {
        each.iter().all(|x| *x)
    } else {
        each.iter().any(|x| *x)
    };
    Ok((hit, each))
}

pub fn matches(r: &Rule, m: &MessageFacts) -> Result<bool> {
    Ok(explain(r, m)?.0)
}

/// The merged effect of all matching rules on one message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub rule_ids: Vec<i64>,
    pub mark_read: bool,
    pub flag: bool,
    pub keywords: Vec<String>,
    pub copies: Vec<String>,
    /// Final destination (first move wins; Delete = the Trash folder).
    pub move_to: Option<String>,
    pub delete: bool,
    pub forwards: Vec<String>,
    /// Actions dropped by a guard, with the reason (shown in Test).
    pub skipped: Vec<String>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.rule_ids.is_empty()
    }
}

/// Evaluate rules in order (stopping at `Stop`). `source` is the folder
/// the message is in; copying/moving into it is skipped (loop guard).
/// Forwards are dropped for automatic/forwarded mail and mail from
/// ourselves (forward-storm guard).
pub fn plan<'a>(
    rules: impl IntoIterator<Item = &'a Rule>,
    m: &MessageFacts,
    source: &str,
) -> Result<Plan> {
    let mut p = Plan::default();
    for r in rules.into_iter().filter(|r| r.enabled) {
        if !matches(r, m)? {
            continue;
        }
        p.rule_ids.push(r.id);
        let mut stop = false;
        for a in &r.actions {
            match a {
                Action::Move { folder } => {
                    if folder == source {
                        p.skipped.push(format!("move to {folder}: already there"));
                    } else if p.move_to.is_none() {
                        p.move_to = Some(folder.clone());
                    }
                }
                Action::Copy { folder } => {
                    if folder == source {
                        p.skipped
                            .push(format!("copy to {folder}: same folder (loop guard)"));
                    } else if !p.copies.contains(folder) {
                        p.copies.push(folder.clone());
                    }
                }
                Action::MarkRead => p.mark_read = true,
                Action::Flag => p.flag = true,
                Action::Label { keyword } => {
                    if !p.keywords.contains(keyword) {
                        p.keywords.push(keyword.clone());
                    }
                }
                Action::Delete => {
                    if p.move_to.is_none() {
                        p.delete = true;
                    }
                }
                Action::Forward { to } => {
                    let to = to.trim().to_ascii_lowercase();
                    if m.auto_submitted {
                        p.skipped.push(format!(
                            "forward to {to}: message was sent automatically (loop guard)"
                        ));
                    } else if m.from.1.contains(&m.account_email) || to == m.account_email {
                        p.skipped
                            .push(format!("forward to {to}: own address (loop guard)"));
                    } else if !p.forwards.contains(&to) {
                        p.forwards.push(to);
                    }
                }
                Action::Stop => stop = true,
            }
        }
        if stop {
            break;
        }
    }
    if p.move_to.is_some() {
        p.delete = false;
    }
    p.copies.retain(|c| Some(c) != p.move_to.as_ref());
    Ok(p)
}

// ---- Sieve export -------------------------------------------------------

fn sieve_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars().filter(|c| *c != '\r' && *c != '\n') {
        if c == '"' || c == '\\' {
            o.push('\\');
        }
        o.push(c);
    }
    o.push('"');
    o
}

/// Escape Sieve `:matches` wildcards.
fn sieve_wild(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '*' | '?' | '\\' => vec!['\\', c],
            c => vec![c],
        })
        .collect()
}

/// Regexes that mean the same in Rust `regex` and POSIX ERE (Sieve regex).
fn portable_regex(p: &str) -> bool {
    let mut chars = p.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(n) if n.is_ascii_punctuation() => {}
                _ => return false, // \d \w \s \b … differ
            }
        }
    }
    !p.contains("(?") && !p.contains("*?") && !p.contains("+?") && !p.contains("??")
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SieveExport {
    pub script: String,
    /// Rules included in the script.
    pub exported: Vec<i64>,
    /// (rule id, reason) for server-side rules that stay local.
    pub local_only: Vec<(i64, String)>,
}

fn sieve_test(
    c: &Condition,
    account_email: &str,
    req: &mut std::collections::BTreeSet<&'static str>,
) -> std::result::Result<String, String> {
    let v = &c.value;
    let addr_hdr = match c.field {
        Field::From => Some("from"),
        Field::To => Some("to"),
        Field::Cc => Some("cc"),
        _ => None,
    };
    let regex_ok = |req: &mut std::collections::BTreeSet<&'static str>| {
        if portable_regex(v) {
            req.insert("regex");
            Ok(())
        } else {
            Err("regex uses syntax the server's POSIX regex doesn't share".to_string())
        }
    };
    Ok(match (c.field, addr_hdr) {
        (_, Some(h)) => match c.op {
            Op::Contains => format!("header :contains {} {}", sieve_str(h), sieve_str(v)),
            Op::Equals => format!("address :all :is {} {}", sieve_str(h), sieve_str(v.trim())),
            Op::StartsWith => format!(
                "address :all :matches {} {}",
                sieve_str(h),
                sieve_str(&format!("{}*", sieve_wild(v)))
            ),
            Op::Regex => {
                regex_ok(req)?;
                format!("address :all :regex {} {}", sieve_str(h), sieve_str(v))
            }
            _ => return Err("unsupported operator".into()),
        },
        (Field::Subject | Field::Header, _) => {
            let h = if c.field == Field::Subject {
                "subject"
            } else {
                c.header.as_str()
            };
            match c.op {
                Op::Contains => format!("header :contains {} {}", sieve_str(h), sieve_str(v)),
                Op::Equals => format!("header :is {} {}", sieve_str(h), sieve_str(v.trim())),
                Op::StartsWith => format!(
                    "header :matches {} {}",
                    sieve_str(h),
                    sieve_str(&format!("{}*", sieve_wild(v)))
                ),
                Op::Regex => {
                    regex_ok(req)?;
                    format!("header :regex {} {}", sieve_str(h), sieve_str(v))
                }
                _ => return Err("unsupported operator".into()),
            }
        }
        (Field::Body, _) => {
            req.insert("body");
            match c.op {
                Op::Contains => format!("body :text :contains {}", sieve_str(v)),
                Op::Regex => {
                    regex_ok(req)?;
                    format!("body :text :regex {}", sieve_str(v))
                }
                _ => return Err("body supports only contains/regex on the server".into()),
            }
        }
        (Field::Size, _) => {
            let n = parse_size(v).map_err(|e| e.to_string())?;
            match c.op {
                Op::Greater => format!("size :over {n}"),
                Op::Less => format!("size :under {n}"),
                _ => return Err("unsupported operator".into()),
            }
        }
        (Field::HasAttachment, _) => {
            req.insert("mime");
            let t = "header :mime :anychild :contains \"Content-Disposition\" \"attachment\""
                .to_string();
            if c.op == Op::IsNot {
                format!("not {t}")
            } else {
                t
            }
        }
        (Field::From | Field::To | Field::Cc, None) => unreachable!("address fields have a header"),
        (Field::Account, _) => {
            // Resolved now: the script belongs to one account.
            let m = MessageFacts {
                account_email: account_email.to_ascii_lowercase(),
                ..Default::default()
            };
            if condition_matches(c, &m).map_err(|e| e.to_string())? {
                "true".into()
            } else {
                "false".into()
            }
        }
    })
}

/// Generate the Sieve script for one account from the rules marked
/// `server_side`. `trash` is the account's Trash folder (for Delete).
pub fn to_sieve(rules: &[Rule], account_email: &str, trash: &str) -> SieveExport {
    use std::collections::BTreeSet;
    let mut req: BTreeSet<&'static str> = BTreeSet::new();
    let mut body = String::new();
    let mut exported = Vec::new();
    let mut local_only = Vec::new();
    for r in rules.iter().filter(|r| r.enabled && r.server_side) {
        if validate(r).is_err() {
            local_only.push((r.id, "rule is invalid".into()));
            continue;
        }
        let mut rreq = BTreeSet::new();
        let tests: std::result::Result<Vec<String>, String> = r
            .conditions
            .iter()
            .map(|c| sieve_test(c, account_email, &mut rreq))
            .collect();
        let tests = match tests {
            Ok(t) => t,
            Err(e) => {
                local_only.push((r.id, e));
                continue;
            }
        };
        // Flags must be set before fileinto/keep so they land on the copy.
        let mut flags = Vec::new();
        let mut file = Vec::new();
        let mut stop = false;
        for a in &r.actions {
            match a {
                Action::MarkRead => flags.push("\\\\Seen".to_string()),
                Action::Flag => flags.push("\\\\Flagged".to_string()),
                Action::Label { keyword } => flags.push(keyword.clone()),
                Action::Move { folder } => {
                    rreq.insert("fileinto");
                    file.push(format!("fileinto {};", sieve_str(folder)));
                }
                Action::Copy { folder } => {
                    rreq.insert("fileinto");
                    rreq.insert("copy");
                    file.push(format!("fileinto :copy {};", sieve_str(folder)));
                }
                Action::Delete => {
                    rreq.insert("fileinto");
                    file.push(format!("fileinto {};", sieve_str(trash)));
                }
                Action::Forward { to } => {
                    rreq.insert("copy");
                    file.push(format!("redirect :copy {};", sieve_str(to.trim())));
                }
                Action::Stop => stop = true,
            }
        }
        if !flags.is_empty() {
            rreq.insert("imap4flags");
        }
        req.extend(rreq);
        let joined = match tests.len() {
            1 => tests[0].clone(),
            _ => format!(
                "{}({})",
                if r.match_all { "allof" } else { "anyof" },
                tests.join(",\n      ")
            ),
        };
        let name: String = r.name.chars().filter(|c| !c.is_control()).collect();
        body.push_str(&format!("\n# Rule {}: {}\nif {} {{\n", r.id, name, joined));
        if !flags.is_empty() {
            let list: Vec<String> = flags.iter().map(|f| format!("\"{f}\"")).collect();
            body.push_str(&format!("  addflag [{}];\n", list.join(", ")));
        }
        for f in &file {
            body.push_str(&format!("  {f}\n"));
        }
        if !flags.is_empty()
            && !r
                .actions
                .iter()
                .any(|a| matches!(a, Action::Move { .. } | Action::Delete))
        {
            // Keep the INBOX copy with the flags.
            body.push_str("  keep;\n");
        }
        if stop {
            body.push_str("  stop;\n");
        }
        body.push_str("}\n");
        exported.push(r.id);
    }
    let mut script = String::from(
        "# Generated by Neutron Mail. Edit rules in the app; changes here are overwritten.\n",
    );
    if !req.is_empty() {
        let list: Vec<String> = req.iter().map(|r| format!("\"{r}\"")).collect();
        script.push_str(&format!("require [{}];\n", list.join(", ")));
    }
    script.push_str(&body);
    SieveExport {
        script,
        exported,
        local_only,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cond(field: Field, op: Op, v: &str) -> Condition {
        Condition {
            field,
            op,
            value: v.into(),
            header: String::new(),
        }
    }
    fn rule(conds: Vec<Condition>, actions: Vec<Action>) -> Rule {
        Rule {
            id: 1,
            name: "r".into(),
            enabled: true,
            match_all: true,
            conditions: conds,
            actions,
            server_side: false,
        }
    }
    const MSG: &[u8] = b"From: Elena Marsh <elena@example.org>\r\nTo: kaan@localhost, team@lists.example\r\nCc: ops@example.org\r\nSubject: [Invoice] Q4 2026\r\nList-Id: <billing.example.org>\r\nMessage-ID: <m1@example.org>\r\nContent-Type: text/plain\r\n\r\nPlease pay invoice 4711 by Friday.\r\n";

    #[test]
    fn conditions() {
        let m = MessageFacts::from_raw(MSG, "kaan@localhost");
        let t = |c: Condition| condition_matches(&c, &m).unwrap();
        assert!(t(cond(Field::From, Op::Contains, "elena marsh")));
        assert!(t(cond(Field::From, Op::Equals, "ELENA@example.org")));
        assert!(!t(cond(Field::From, Op::Equals, "elena")));
        assert!(t(cond(Field::From, Op::StartsWith, "elena@")));
        assert!(t(cond(Field::To, Op::Regex, r"^team@lists\.")));
        assert!(t(cond(Field::Cc, Op::Equals, "ops@example.org")));
        assert!(t(cond(Field::Subject, Op::StartsWith, "[invoice]")));
        assert!(t(cond(Field::Body, Op::Regex, r"invoice \d{4}")));
        assert!(t(Condition {
            header: "List-Id".into(),
            ..cond(Field::Header, Op::Contains, "billing")
        }));
        assert!(!t(Condition {
            header: "X-Missing".into(),
            ..cond(Field::Header, Op::Contains, "x")
        }));
        assert!(t(cond(Field::Size, Op::Less, "10K")));
        assert!(!t(cond(Field::Size, Op::Greater, "1M")));
        assert!(t(cond(Field::HasAttachment, Op::IsNot, "")));
        assert!(t(cond(Field::Account, Op::Equals, "kaan@localhost")));
        let mut r = rule(
            vec![
                cond(Field::Subject, Op::Contains, "nope"),
                cond(Field::From, Op::Contains, "elena"),
            ],
            vec![Action::Flag],
        );
        assert!(!matches(&r, &m).unwrap());
        r.match_all = false;
        assert_eq!(explain(&r, &m).unwrap(), (true, vec![false, true]));
    }

    #[test]
    fn validation_and_regex_limits() {
        let ok = rule(
            vec![cond(Field::Subject, Op::Contains, "x")],
            vec![Action::MarkRead],
        );
        assert!(validate(&ok).is_ok());
        let mut r = ok.clone();
        r.conditions[0] = cond(Field::Subject, Op::Regex, "(unclosed");
        assert!(validate(&r)
            .unwrap_err()
            .to_string()
            .contains("invalid regex"));
        r.conditions[0] = cond(
            Field::Subject,
            Op::Regex,
            &"a".repeat(MAX_PATTERN_CHARS + 1),
        );
        assert!(validate(&r).is_err());
        // Huge counted repetition blows the compiled-size limit instead of memory.
        r.conditions[0] = cond(Field::Subject, Op::Regex, r"(\w{1000}){1000}");
        assert!(validate(&r).is_err());
        // Catastrophic-backtracking patterns are linear in `regex`.
        r.conditions[0] = cond(Field::Body, Op::Regex, "(a+)+$");
        validate(&r).unwrap();
        let m = MessageFacts {
            body: "a".repeat(100_000) + "!",
            ..Default::default()
        };
        let t = std::time::Instant::now();
        assert!(!matches(&r, &m).unwrap());
        assert!(t.elapsed() < std::time::Duration::from_secs(2));
        for bad in [
            rule(
                vec![cond(Field::Size, Op::Contains, "1")],
                vec![Action::Flag],
            ),
            rule(
                vec![cond(Field::Size, Op::Greater, "lots")],
                vec![Action::Flag],
            ),
            rule(
                vec![cond(Field::Subject, Op::Contains, "")],
                vec![Action::Flag],
            ),
            rule(
                vec![cond(Field::Subject, Op::Contains, "a\r\nb")],
                vec![Action::Flag],
            ),
            rule(
                vec![Condition {
                    header: "Bad Header:".into(),
                    ..cond(Field::Header, Op::Contains, "x")
                }],
                vec![Action::Flag],
            ),
            rule(
                vec![cond(Field::Subject, Op::Contains, "x")],
                vec![Action::Stop],
            ),
            rule(
                vec![cond(Field::Subject, Op::Contains, "x")],
                vec![Action::Move {
                    folder: "INBOX".into(),
                }],
            ),
            rule(
                vec![cond(Field::Subject, Op::Contains, "x")],
                vec![Action::Label {
                    keyword: "has space".into(),
                }],
            ),
            rule(
                vec![cond(Field::Subject, Op::Contains, "x")],
                vec![Action::Forward {
                    to: "a@b.c\r\nBcc: x@y.z".into(),
                }],
            ),
            rule(vec![], vec![Action::Flag]),
        ] {
            assert!(validate(&bad).is_err(), "{bad:?}");
        }
        assert_eq!(parse_size("2M").unwrap(), 2 * 1024 * 1024);
    }

    #[test]
    fn plan_order_stop_and_guards() {
        let m = MessageFacts::from_raw(MSG, "kaan@localhost");
        let any = || vec![cond(Field::From, Op::Contains, "elena")];
        let rules = vec![
            Rule {
                id: 1,
                ..rule(
                    any(),
                    vec![
                        Action::Label {
                            keyword: "$billing".into(),
                        },
                        Action::Copy {
                            folder: "INBOX".into(),
                        },
                    ],
                )
            },
            Rule {
                id: 2,
                ..rule(
                    any(),
                    vec![
                        Action::Move {
                            folder: "Invoices".into(),
                        },
                        Action::MarkRead,
                        Action::Forward {
                            to: "Acct@Example.net".into(),
                        },
                        Action::Stop,
                    ],
                )
            },
            Rule {
                id: 3,
                ..rule(any(), vec![Action::Delete])
            },
        ];
        let p = plan(&rules, &m, "INBOX").unwrap();
        assert_eq!(p.rule_ids, vec![1, 2]);
        assert_eq!(p.move_to.as_deref(), Some("Invoices"));
        assert!(p.mark_read && !p.delete);
        assert_eq!(p.keywords, vec!["$billing"]);
        assert!(p.copies.is_empty() && p.skipped[0].contains("loop guard"));
        assert_eq!(p.forwards, vec!["acct@example.net"]);
        // Auto-forwarded mail is never forwarded again.
        let fwd = [b"Auto-Submitted: auto-forwarded\r\n".as_slice(), MSG].concat();
        let p = plan(
            &rules,
            &MessageFacts::from_raw(&fwd, "kaan@localhost"),
            "INBOX",
        )
        .unwrap();
        assert!(p.forwards.is_empty() && p.skipped.iter().any(|s| s.contains("automatically")));
        // Disabled rules don't run.
        let mut off = rules.clone();
        off[1].enabled = false;
        let p = plan(&off, &m, "INBOX").unwrap();
        assert!(p.delete && p.move_to.is_none());
    }

    #[test]
    fn sieve_export() {
        let mut r1 = rule(
            vec![
                cond(Field::From, Op::Equals, "elena@example.org"),
                cond(Field::Subject, Op::StartsWith, "[Invoice*]"),
            ],
            vec![
                Action::MarkRead,
                Action::Label {
                    keyword: "$billing".into(),
                },
                Action::Move {
                    folder: "Rechnungen/2026 \"Q4\"".into(),
                },
                Action::Stop,
            ],
        );
        r1.server_side = true;
        let mut r2 = rule(
            vec![cond(Field::Body, Op::Regex, r"\d+")],
            vec![Action::Flag],
        );
        r2.id = 2;
        r2.server_side = true;
        let mut r3 = rule(
            vec![
                cond(Field::Size, Op::Greater, "5M"),
                cond(Field::HasAttachment, Op::Is, ""),
            ],
            vec![
                Action::Copy {
                    folder: "大文件".into(),
                },
                Action::Forward {
                    to: "b@example.net".into(),
                },
            ],
        );
        r3.id = 3;
        r3.server_side = true;
        r3.match_all = false;
        let local = Rule {
            id: 4,
            ..rule(
                vec![cond(Field::Subject, Op::Contains, "x")],
                vec![Action::Delete],
            )
        };
        let e = to_sieve(&[r1, r2, r3, local], "kaan@localhost", "Trash");
        assert_eq!(e.exported, vec![1, 3]);
        assert_eq!(e.local_only.len(), 1);
        assert_eq!(e.local_only[0].0, 2);
        let s = &e.script;
        assert!(
            s.contains(r#"require ["copy", "fileinto", "imap4flags", "mime"];"#),
            "{s}"
        );
        assert!(s.contains(r#"address :all :is "from" "elena@example.org""#));
        assert!(
            s.contains(r#"header :matches "subject" "[Invoice\\*]*""#),
            "{s}"
        );
        assert!(s.contains(r#"addflag ["\\Seen", "$billing"];"#), "{s}");
        assert!(s.contains(r#"fileinto "Rechnungen/2026 \"Q4\"";"#));
        assert!(s.contains("anyof(size :over 5242880"));
        assert!(s.contains(r#"fileinto :copy "大文件";"#));
        assert!(s.contains(r#"redirect :copy "b@example.net";"#));
        assert!(!s.contains("Rule 4"));
        // Injection: a rule name with a newline can't break out of the comment.
        let mut evil = rule(
            vec![cond(Field::Subject, Op::Contains, "\" ; discard; \"")],
            vec![Action::Flag],
        );
        evil.name = "x\ndiscard;".into();
        evil.server_side = true;
        assert!(to_sieve(std::slice::from_ref(&evil), "k@x.test", "Trash")
            .exported
            .is_empty());
        evil.name = "x # discard;".into();
        let s = to_sieve(&[evil], "k@x.test", "Trash").script;
        assert!(!s.lines().any(|l| l.trim() == "discard;"), "{s}");
        assert!(s.contains(r#""\" ; discard; \"""#));
    }
}
