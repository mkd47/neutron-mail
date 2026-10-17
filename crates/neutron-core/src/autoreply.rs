//! Automatic replies ("out of office").
//!
//! Two ways to run, chosen per account:
//! * From this computer: during sync, new INBOX mail is answered by the app.
//!   Works with every IMAP/SMTP server (e.g. 263.net, which offers its own
//!   auto-reply only in webmail), but only while Neutron Mail is running.
//! * On the server: a Sieve `vacation` action (RFC 5230) in the script the
//!   app uploads over ManageSieve, so replies go out with the app closed.
//!
//! Both follow RFC 3834: never answer automated mail, mailing lists,
//! no-reply addresses or yourself; only mail addressed to you; at most once
//! per sender every `days` days; replies carry `Auto-Submitted: auto-replied`.

use serde::{Deserialize, Serialize};

use crate::model::AccountConfig;
use crate::store::Store;
use crate::{smtp, Error, Result};

/// Longest reply text kept (also a sane Sieve script size).
pub const MAX_BODY_BYTES: usize = 16 * 1024;
/// Replies sent per sync at most (a burst of mail can't flood anyone).
const MAX_PER_SYNC: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoReply {
    pub enabled: bool,
    /// Empty: "Automatic reply: <original subject>".
    pub subject: String,
    /// Plain-text message.
    pub body: String,
    /// Optional period (unix seconds, inclusive).
    pub start: Option<i64>,
    pub end: Option<i64>,
    /// Answer each sender at most once every this many days (1–30).
    pub days: u32,
    /// Use the server's Sieve vacation instead of replying from this computer.
    pub on_server: bool,
    /// When it was last switched on: older mail is never answered.
    pub enabled_at: i64,
}

impl Default for AutoReply {
    fn default() -> Self {
        Self {
            enabled: false,
            subject: String::new(),
            body: String::new(),
            start: None,
            end: None,
            days: 4,
            on_server: false,
            enabled_at: 0,
        }
    }
}

impl AutoReply {
    /// Check user input before storing it.
    pub fn validate(&self) -> Result<()> {
        if !(1..=30).contains(&self.days) {
            return Err(Error::Invalid("reply once every 1 to 30 days".into()));
        }
        if self.subject.contains(['\r', '\n']) || self.subject.chars().count() > 200 {
            return Err(Error::Invalid(
                "the subject must be one line of at most 200 characters".into(),
            ));
        }
        if self.body.len() > MAX_BODY_BYTES {
            return Err(Error::Invalid(
                "the reply text is too long (max 16 KB)".into(),
            ));
        }
        if self.enabled && self.body.trim().is_empty() {
            return Err(Error::Invalid("write the reply text first".into()));
        }
        if let (Some(s), Some(e)) = (self.start, self.end) {
            if e < s {
                return Err(Error::Invalid(
                    "the end date is before the start date".into(),
                ));
            }
        }
        Ok(())
    }

    /// Active at `now` (switched on and inside the optional period).
    pub fn active_at(&self, now: i64) -> bool {
        self.enabled && self.start.is_none_or(|s| now >= s) && self.end.is_none_or(|e| now <= e)
    }
}

/// Why a message gets no automatic reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    Inactive,
    OldMessage,
    NoSender,
    FromSelf,
    Automated,
    MailingList,
    NoReplyAddress,
    NotAddressedToMe,
}

/// Who to answer and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Reply-To, else From (lower-case address).
    pub to: String,
    /// Our address the message was sent to (sender of the reply).
    pub via: String,
    pub subject: String,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
}

fn is_noreply(addr: &str) -> bool {
    let local = addr.split('@').next().unwrap_or("").to_ascii_lowercase();
    let squashed: String = local
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    squashed.contains("noreply")
        || squashed.contains("donotreply")
        || local.starts_with("mailer-daemon")
        || local.starts_with("postmaster")
        || local.starts_with("bounce")
        || local.starts_with("owner-")
        || local.ends_with("-request")
        || local == "listserv"
        || local == "majordomo"
}

fn addresses(a: Option<&mail_parser::Address<'_>>) -> Vec<String> {
    a.map(|a| {
        a.iter()
            .filter_map(|x| x.address().map(|e| e.trim().to_ascii_lowercase()))
            .collect()
    })
    .unwrap_or_default()
}

/// Decide whether `raw` gets an automatic reply. `own` are the account's
/// addresses (primary first, then aliases).
pub fn evaluate(
    raw: &[u8],
    cfg: &AutoReply,
    own: &[String],
    now: i64,
) -> std::result::Result<Plan, Skip> {
    if !cfg.active_at(now) {
        return Err(Skip::Inactive);
    }
    let msg = mail_parser::MessageParser::default()
        .parse_headers(raw)
        .ok_or(Skip::NoSender)?;
    let header = |name: &str| -> Option<String> {
        msg.header_raw(name).map(|v| v.trim().to_ascii_lowercase())
    };
    // Mail from before auto-reply was switched on (or the period began) is
    // never answered, e.g. when the app syncs after being closed a while.
    let date = msg.date().map(|d| d.to_timestamp()).unwrap_or(now);
    let since = cfg.start.unwrap_or(0).max(cfg.enabled_at);
    if date < since - 600 {
        return Err(Skip::OldMessage);
    }
    let from = addresses(msg.from());
    let Some(sender) = from.first().cloned() else {
        return Err(Skip::NoSender);
    };
    let own: Vec<String> = own.iter().map(|e| e.trim().to_ascii_lowercase()).collect();
    if own.contains(&sender) {
        return Err(Skip::FromSelf);
    }
    if header("Auto-Submitted").is_some_and(|v| v != "no")
        || header("X-Auto-Response-Suppress")
            .is_some_and(|v| ["all", "oof", "autoreply"].iter().any(|k| v.contains(k)))
        || header("Return-Path").is_some_and(|v| v == "<>")
        || header("X-Neutron-Rule").is_some()
        || header("X-Autoreply").is_some()
        || header("X-Autorespond").is_some()
        || header("Precedence").is_some_and(|v| v == "auto_reply")
        || header("Content-Type").is_some_and(|v| v.starts_with("multipart/report"))
    {
        return Err(Skip::Automated);
    }
    if header("List-Id").is_some()
        || header("List-Unsubscribe").is_some()
        || header("List-Post").is_some()
        || header("Precedence").is_some_and(|v| ["bulk", "list", "junk"].contains(&v.as_str()))
    {
        return Err(Skip::MailingList);
    }
    let reply_to = addresses(msg.reply_to())
        .into_iter()
        .next()
        .unwrap_or(sender.clone());
    if is_noreply(&sender) || is_noreply(&reply_to) {
        return Err(Skip::NoReplyAddress);
    }
    let recipients: Vec<String> = addresses(msg.to())
        .into_iter()
        .chain(addresses(msg.cc()))
        .collect();
    let Some(via) = own.iter().find(|o| recipients.contains(o)).cloned() else {
        return Err(Skip::NotAddressedToMe);
    };
    let original = msg.subject().unwrap_or("").trim();
    let subject = if cfg.subject.trim().is_empty() {
        if original.is_empty() {
            "Automatic reply".to_string()
        } else {
            format!("Automatic reply: {original}")
        }
    } else {
        cfg.subject.trim().to_string()
    };
    let parent = msg.message_id().map(str::to_string);
    let mut references: Vec<String> = match msg.references() {
        mail_parser::HeaderValue::Text(t) => vec![t.to_string()],
        mail_parser::HeaderValue::TextList(l) => l.iter().map(|t| t.to_string()).collect(),
        _ => Vec::new(),
    };
    if let Some(p) = &parent {
        references.push(p.clone());
    }
    Ok(Plan {
        to: reply_to,
        via,
        subject,
        in_reply_to: parent,
        references,
    })
}

/// Answer new INBOX messages (by UID) from this computer, if automatic
/// replies are on and not delegated to the server. Returns replies sent.
pub async fn process(
    store: &Store,
    cfg: &AccountConfig,
    password: &str,
    account_id: i64,
    folder_id: i64,
    uids: &[u32],
) -> Result<usize> {
    let reply = store.auto_reply(account_id)?;
    let now = chrono::Utc::now().timestamp();
    if !reply.active_at(now) || (reply.on_server && store.sieve_settings(account_id)?.active) {
        return Ok(0);
    }
    let identities = store.account_settings(account_id)?.identities;
    let own: Vec<String> = std::iter::once(cfg.email.clone())
        .chain(identities.iter().map(|i| i.email.clone()))
        .collect();
    let mut sent = 0;
    // A message that can't be answered doesn't stop the others; repeated
    // send failures (server down, login refused) end the batch.
    let mut failures = 0;
    let mut first_error = None;
    for id in store.ids_for_uids(folder_id, uids)? {
        if sent >= MAX_PER_SYNC || failures >= 3 {
            break;
        }
        let Ok(plan) = evaluate(&store.raw(id)?, &reply, &own, now) else {
            continue;
        };
        if !store.auto_reply_due(account_id, &plan.to, now, reply.days)? {
            continue;
        }
        let result = async {
            let sender = smtp::with_sender(cfg, &identities, Some(&plan.via))?;
            let built = smtp::build_auto_reply(
                &sender,
                &plan.to,
                &plan.subject,
                &reply.body,
                plan.in_reply_to.as_deref(),
                &plan.references,
            )?;
            smtp::send_built(&sender, password, &built).await
        }
        .await;
        match result {
            Ok(()) => {
                store.record_auto_reply(account_id, &plan.to, now)?;
                sent += 1;
                failures = 0;
                tracing::info!(account = account_id, "automatic reply sent");
            }
            Err(e) => {
                tracing::warn!(account = account_id, "automatic reply failed: {e}");
                failures += 1;
                first_error.get_or_insert(e);
            }
        }
    }
    match first_error {
        Some(e) if sent == 0 => Err(e),
        _ => Ok(sent),
    }
}

/// Sieve `vacation` (RFC 5230) for the server script: the extensions it
/// needs and the script text. Dates use the `date` + `relational`
/// extensions (RFC 5260), in UTC days.
pub fn sieve_block(reply: &AutoReply, own: &[String]) -> (Vec<&'static str>, String) {
    use crate::rules::sieve_str;
    let mut req = vec!["vacation"];
    let addrs: Vec<String> = own.iter().map(|a| sieve_str(a)).collect();
    let mut action = format!("vacation :days {}", reply.days);
    if !reply.subject.trim().is_empty() {
        action.push_str(&format!(" :subject {}", sieve_str(reply.subject.trim())));
    }
    if !addrs.is_empty() {
        action.push_str(&format!(" :addresses [{}]", addrs.join(", ")));
    }
    // Multi-line text (RFC 5228 §2.4.2): a line holding only "." is dot-stuffed.
    let text: String = reply
        .body
        .replace("\r\n", "\n")
        .lines()
        .map(|l| {
            if l.starts_with('.') {
                format!(".{l}")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\r\n");
    action.push_str(&format!(" text:\r\n{text}\r\n.\r\n;"));
    // Dates are picked in this computer's time zone.
    let day = |t: i64| {
        chrono::DateTime::from_timestamp(t, 0)
            .map(|d| {
                d.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d")
                    .to_string()
            })
            .unwrap_or_default()
    };
    let mut tests = Vec::new();
    if let Some(s) = reply.start {
        tests.push(format!("currentdate :value \"ge\" \"date\" \"{}\"", day(s)));
    }
    if let Some(e) = reply.end {
        tests.push(format!("currentdate :value \"le\" \"date\" \"{}\"", day(e)));
    }
    let block = if tests.is_empty() {
        format!("# Automatic reply\n{action}\n")
    } else {
        req.push("date");
        req.push("relational");
        format!(
            "# Automatic reply\nif allof ({}) {{\n  {action}\n}}\n",
            tests.join(", ")
        )
    };
    (req, block)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> AutoReply {
        AutoReply {
            enabled: true,
            body: "I'm away until Monday.".into(),
            enabled_at: 1_000_000,
            ..Default::default()
        }
    }
    const NOW: i64 = 1_000_500;
    fn own() -> Vec<String> {
        vec!["me@example.test".into(), "sales@example.test".into()]
    }
    fn mail(extra: &str) -> Vec<u8> {
        format!(
            "From: Bob <Bob@Example.test>\r\nTo: Me <me@example.test>\r\nSubject: Quote\r\nMessage-ID: <q1@example.test>\r\nDate: Mon, 12 Jan 1970 13:50:00 +0000\r\n{extra}\r\nHi"
        )
        .into_bytes()
    }

    #[test]
    fn other_auto_responders_and_bounces_are_not_answered() {
        for extra in [
            "X-Autoreply: yes",
            "X-Autorespond: on",
            "Precedence: auto_reply",
            "Content-Type: multipart/report; report-type=delivery-status; boundary=x",
        ] {
            assert!(
                matches!(
                    evaluate(&mail(extra), &on(), &own(), NOW),
                    Err(Skip::Automated)
                ),
                "{extra}"
            );
        }
    }

    #[test]
    fn replies_to_a_normal_message_addressed_to_us() {
        let p = evaluate(&mail(""), &on(), &own(), NOW).unwrap();
        assert_eq!(p.to, "bob@example.test");
        assert_eq!(p.via, "me@example.test");
        assert_eq!(p.subject, "Automatic reply: Quote");
        assert_eq!(p.in_reply_to.as_deref(), Some("q1@example.test"));
        let custom = AutoReply {
            subject: "Out of office".into(),
            ..on()
        };
        assert_eq!(
            evaluate(&mail(""), &custom, &own(), NOW).unwrap().subject,
            "Out of office"
        );
    }

    #[test]
    fn reply_goes_to_reply_to_from_the_alias_it_was_sent_to() {
        let raw = b"From: bob@example.test\r\nReply-To: desk@example.test\r\nTo: SALES@example.test\r\nSubject: x\r\nDate: Mon, 12 Jan 1970 13:50:00 +0000\r\n\r\nHi";
        let p = evaluate(raw, &on(), &own(), NOW).unwrap();
        assert_eq!(
            (p.to.as_str(), p.via.as_str()),
            ("desk@example.test", "sales@example.test")
        );
    }

    #[test]
    fn rfc3834_cases_are_never_answered() {
        let cases: [(&str, Skip); 9] = [
            ("Auto-Submitted: auto-replied\r\n", Skip::Automated),
            (
                "X-Auto-Response-Suppress: OOF, AutoReply\r\n",
                Skip::Automated,
            ),
            ("Return-Path: <>\r\n", Skip::Automated),
            ("List-Id: <team.lists.example.test>\r\n", Skip::MailingList),
            (
                "List-Unsubscribe: <mailto:u@example.test>\r\n",
                Skip::MailingList,
            ),
            ("Precedence: bulk\r\n", Skip::MailingList),
            ("Precedence: junk\r\n", Skip::MailingList),
            ("Reply-To: no-reply@example.test\r\n", Skip::NoReplyAddress),
            (
                "Reply-To: mailer-daemon@example.test\r\n",
                Skip::NoReplyAddress,
            ),
        ];
        for (extra, why) in cases {
            assert_eq!(
                evaluate(&mail(extra), &on(), &own(), NOW).unwrap_err(),
                why,
                "{extra}"
            );
        }
        // "Auto-Submitted: no" is an ordinary message.
        assert!(evaluate(&mail("Auto-Submitted: no\r\n"), &on(), &own(), NOW).is_ok());
    }

    #[test]
    fn self_bcc_old_and_inactive_are_skipped() {
        let from_me = b"From: me@example.test\r\nTo: me@example.test\r\nDate: Mon, 12 Jan 1970 13:50:00 +0000\r\n\r\nx";
        assert_eq!(
            evaluate(from_me, &on(), &own(), NOW).unwrap_err(),
            Skip::FromSelf
        );
        let bcc = b"From: bob@example.test\r\nTo: list@example.test\r\nDate: Mon, 12 Jan 1970 13:50:00 +0000\r\n\r\nx";
        assert_eq!(
            evaluate(bcc, &on(), &own(), NOW).unwrap_err(),
            Skip::NotAddressedToMe
        );
        let old = b"From: bob@example.test\r\nTo: me@example.test\r\nDate: Thu, 1 Jan 1970 00:00:00 +0000\r\n\r\nx";
        assert_eq!(
            evaluate(old, &on(), &own(), NOW).unwrap_err(),
            Skip::OldMessage
        );
        assert_eq!(
            evaluate(&mail(""), &AutoReply::default(), &own(), NOW).unwrap_err(),
            Skip::Inactive
        );
        let ended = AutoReply {
            end: Some(NOW - 1),
            ..on()
        };
        assert_eq!(
            evaluate(&mail(""), &ended, &own(), NOW).unwrap_err(),
            Skip::Inactive
        );
        let later = AutoReply {
            start: Some(NOW + 1),
            ..on()
        };
        assert_eq!(
            evaluate(&mail(""), &later, &own(), NOW).unwrap_err(),
            Skip::Inactive
        );
    }

    #[test]
    fn settings_are_validated() {
        assert!(on().validate().is_ok());
        assert!(AutoReply {
            body: " ".into(),
            ..on()
        }
        .validate()
        .is_err());
        assert!(AutoReply { days: 0, ..on() }.validate().is_err());
        assert!(AutoReply {
            subject: "a\r\nBcc: x".into(),
            ..on()
        }
        .validate()
        .is_err());
        assert!(AutoReply {
            start: Some(10),
            end: Some(5),
            ..on()
        }
        .validate()
        .is_err());
        assert!(
            AutoReply::default().validate().is_ok(),
            "off with no text is fine"
        );
    }

    #[test]
    fn sieve_vacation_block_is_escaped_and_dated() {
        let r = AutoReply {
            subject: "Away \"soon\"".into(),
            body: "Back on Monday.\n.\nThanks".into(),
            start: Some(1_767_268_800), // 2026-01-01 12:00 UTC
            end: Some(1_767_787_200),   // 2026-01-07 12:00 UTC
            ..on()
        };
        let (req, block) = sieve_block(&r, &own());
        assert_eq!(req, vec!["vacation", "date", "relational"]);
        assert!(
            block.contains(":days 4 :subject \"Away \\\"soon\\\"\""),
            "{block}"
        );
        assert!(block.contains(":addresses [\"me@example.test\", \"sales@example.test\"]"));
        assert!(
            block.contains("text:\r\nBack on Monday.\r\n..\r\nThanks\r\n.\r\n;"),
            "dot-stuffed: {block}"
        );
        assert!(block.contains("currentdate :value \"ge\" \"date\" \"2026-01-01\""));
        assert!(block.contains("currentdate :value \"le\" \"date\" \"2026-01-07\""));
        let (req, block) = sieve_block(&on(), &own());
        assert_eq!(req, vec!["vacation"]);
        assert!(!block.contains("if allof"));
    }
}
