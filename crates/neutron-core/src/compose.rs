//! Composer prefill for Reply, Reply All, Forward and resuming a draft.
//!
//! Threading headers follow RFC 5322 §3.6.4: a reply carries
//! `In-Reply-To: <parent>` and `References: <parent's References> <parent>`.
//! A forward carries `References: <parent>` only (it starts a new thread
//! but keeps the link), and all of the parent's attachments.

use mail_parser::{HeaderValue, MessageParser};
use serde::Deserialize;

use crate::model::{ComposeDraft, ForwardedAttachment};
use crate::parse::{self, strip_tags};

/// RFC 5322 recommends trimming long References chains; keep the first
/// (thread root) and the most recent ones.
const MAX_REFERENCES: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Reply,
    ReplyAll,
    Forward,
    /// Re-open a saved draft for editing.
    Draft,
}

fn addrs(a: Option<&mail_parser::Address<'_>>) -> Vec<(String, String)> {
    a.map(|a| {
        a.iter()
            .filter_map(|x| {
                let email = x.address()?.trim().to_string();
                (!email.is_empty()).then(|| (x.name().unwrap_or("").trim().to_string(), email))
            })
            .collect()
    })
    .unwrap_or_default()
}

fn fmt_addr((name, email): &(String, String)) -> String {
    if name.is_empty() {
        email.clone()
    } else if name.contains([',', ';', '<', '>', '"', '@']) {
        format!("\"{}\" <{email}>", name.replace('"', ""))
    } else {
        format!("{name} <{email}>")
    }
}

fn ids(v: &HeaderValue<'_>) -> Vec<String> {
    match v {
        HeaderValue::Text(t) => vec![t.to_string()],
        HeaderValue::TextList(l) => l.iter().map(|t| t.to_string()).collect(),
        _ => Vec::new(),
    }
}

fn with_prefix(prefix: &str, subject: &str) -> String {
    let s = subject.trim();
    let lower = s.to_ascii_lowercase();
    let already = match prefix {
        "Re:" => lower.starts_with("re:"),
        _ => lower.starts_with("fwd:") || lower.starts_with("fw:"),
    };
    if already {
        s.to_string()
    } else {
        format!("{prefix} {s}")
    }
}

fn quote(text: &str) -> String {
    text.trim_end()
        .lines()
        .map(|l| {
            if l.starts_with('>') {
                format!(">{l}")
            } else {
                format!("> {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build the prefill. `message_id` is the cache id of the parent message
/// (used to reference its attachments); `self_email` is excluded from
/// Reply All recipients.
pub fn prepare(raw: &[u8], message_id: i64, mode: Mode, self_email: &str) -> ComposeDraft {
    let Some(msg) = MessageParser::default().parse(raw) else {
        return ComposeDraft::default();
    };
    let p = parse::parse(raw);
    let body_text = if p.text.trim().is_empty() {
        p.html_raw.as_deref().map(strip_tags).unwrap_or_default()
    } else {
        p.text.clone()
    };
    let from = addrs(msg.from());
    let reply_to = addrs(msg.reply_to());
    let to = addrs(msg.to());
    let cc = addrs(msg.cc());
    let date = msg
        .date()
        .map(|d| d.to_rfc822())
        .unwrap_or_else(|| "an unknown date".into());
    let sender = from
        .first()
        .map(fmt_addr)
        .unwrap_or_else(|| "someone".into());
    let parent_id = msg.message_id().map(|m| m.to_string());
    let mut references = ids(msg.references());
    if let Some(id) = &parent_id {
        references.push(id.clone());
    }
    if references.len() > MAX_REFERENCES {
        let tail = references.split_off(references.len() - (MAX_REFERENCES - 1));
        references.truncate(1);
        references.extend(tail);
    }
    let me = self_email.trim().to_ascii_lowercase();

    match mode {
        Mode::Reply | Mode::ReplyAll => {
            let primary = if reply_to.is_empty() {
                from.clone()
            } else {
                reply_to
            };
            let mut seen: Vec<String> = vec![me.clone()];
            let mut keep = |list: Vec<(String, String)>| -> Vec<String> {
                list.into_iter()
                    .filter(|a| {
                        let e = a.1.to_ascii_lowercase();
                        if seen.contains(&e) {
                            false
                        } else {
                            seen.push(e);
                            true
                        }
                    })
                    .map(|a| fmt_addr(&a))
                    .collect()
            };
            // Replying to our own sent message: answer its recipients.
            let primary = if primary.iter().all(|a| a.1.eq_ignore_ascii_case(&me)) && !to.is_empty()
            {
                to.clone()
            } else {
                primary
            };
            let to_list = keep(primary);
            let cc_list = if mode == Mode::ReplyAll {
                keep(to.into_iter().chain(cc).collect())
            } else {
                Vec::new()
            };
            ComposeDraft {
                to: to_list,
                cc: cc_list,
                bcc: Vec::new(),
                subject: with_prefix("Re:", &p.subject),
                body: format!("\n\nOn {date}, {sender} wrote:\n{}\n", quote(&body_text)),
                in_reply_to: parent_id,
                references,
                attachments: Vec::new(),
            }
        }
        Mode::Forward => {
            let mut header = format!(
                "\n\n---------- Forwarded message ----------\nFrom: {sender}\nDate: {date}\nSubject: {}\nTo: {}\n",
                p.subject,
                to.iter().map(fmt_addr).collect::<Vec<_>>().join(", ")
            );
            if !cc.is_empty() {
                header.push_str(&format!(
                    "Cc: {}\n",
                    cc.iter().map(fmt_addr).collect::<Vec<_>>().join(", ")
                ));
            }
            ComposeDraft {
                subject: with_prefix("Fwd:", &p.subject),
                body: format!("{header}\n{}\n", body_text.trim_end()),
                in_reply_to: None,
                references: parent_id.into_iter().collect(),
                attachments: carried(&p, message_id),
                ..Default::default()
            }
        }
        Mode::Draft => ComposeDraft {
            to: to.iter().map(fmt_addr).collect(),
            cc: cc.iter().map(fmt_addr).collect(),
            bcc: addrs(msg.bcc()).iter().map(fmt_addr).collect(),
            subject: p.subject.clone(),
            body: body_text,
            in_reply_to: ids(msg.in_reply_to()).into_iter().next(),
            references: ids(msg.references()),
            attachments: carried(&p, message_id),
        },
    }
}

fn carried(p: &parse::ParsedMessage, message_id: i64) -> Vec<ForwardedAttachment> {
    p.attachments
        .iter()
        .map(|a| ForwardedAttachment {
            message_id,
            index: a.index,
            name: a.name.clone(),
            mime: a.mime.clone(),
            size: a.size,
        })
        .collect()
}
