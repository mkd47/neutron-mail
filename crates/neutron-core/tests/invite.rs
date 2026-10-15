//! iMIP: an invitation email is parsed from the cache, answered with a
//! REPLY over the local SMTP server, and the delivered REPLY is checked.
//! Needs NEUTRON_IT=1 and testserver/start.sh.

mod common;

use neutron_core::model::Security;
use neutron_core::store::{NewMessage, Store};
use neutron_core::{ical, parse, smtp};

pub fn invite_mail(uid: &str) -> Vec<u8> {
    format!(
        "From: Elena Marsh <elena@example.test>\r\nTo: kaan@localhost\r\nSubject: Invitation: Design review\r\n\
Date: Wed, 14 Oct 2026 10:00:00 +0000\r\nMessage-ID: <inv-{uid}>\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\n\
Content-Type: multipart/alternative; boundary=\"b2\"\r\n\r\n--b2\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\nYou are invited.\r\n--b2\r\n\
Content-Type: text/calendar; charset=utf-8; method=REQUEST\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n--b2--\r\n\
--b1\r\nContent-Type: application/ics; name=\"invite.ics\"\r\nContent-Disposition: attachment; filename=\"invite.ics\"\r\n\r\n\
BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n--b1--\r\n",
        neutron_core::parse::base64(
            format!(
                "BEGIN:VCALENDAR\r\nPRODID:-//Test//EN\r\nVERSION:2.0\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\n\
UID:{uid}\r\nSEQUENCE:1\r\nDTSTAMP:20261014T100000Z\r\nDTSTART;TZID=Europe/Istanbul:20261020T140000\r\n\
DTEND;TZID=Europe/Istanbul:20261020T150000\r\nSUMMARY:Design review\r\nLOCATION:Room 4\r\n\
ORGANIZER;CN=Elena Marsh:mailto:elena@example.test\r\n\
ATTENDEE;CN=Kaan Durmus;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:kaan@localhost\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n"
            )
            .as_bytes()
        )
    )
    .into_bytes()
}

#[tokio::test]
async fn invite_parse_and_send_reply() {
    if !common::enabled() {
        return;
    }
    let cfg = common::config(Security::Tls);
    let uid = common::unique("invite") + "@example.test";
    let store = Store::open_in_memory().unwrap();
    let acc = store.add_account(&cfg).unwrap();
    let fid = store.upsert_folder(acc, "INBOX", Some("inbox")).unwrap();
    let raw = invite_mail(&uid);
    let id = store
        .insert_message(
            fid,
            &NewMessage {
                uid: 1,
                seen: false,
                flagged: false,
                raw: &raw,
            },
        )
        .unwrap();

    let ics = parse::calendar_part(&store.raw(id).unwrap()).expect("calendar part");
    let view = ical::invite_view(&ics, &cfg.email, store.invite_response(id).unwrap()).unwrap();
    assert_eq!(view.method, "REQUEST");
    assert!(view.can_respond && !view.responded);
    assert_eq!(view.my_partstat, "NEEDS-ACTION");
    assert_eq!(view.event.start, 1_792_494_000);

    let reply =
        ical::build_reply(&view.event, &cfg.email, &cfg.display_name, "TENTATIVE", 0).unwrap();
    let built = smtp::build_imip(
        &cfg,
        "elena@example.test",
        "Tentative: Design review",
        "Kaan Durmus has tentatively accepted.",
        &reply,
        "REPLY",
        Some(&format!("<inv-{uid}>")),
    )
    .unwrap();
    smtp::send_built(&cfg, common::PASSWORD, &built)
        .await
        .unwrap();
    store.set_invite_response(id, "TENTATIVE", false).unwrap();
    let again = ical::invite_view(&ics, &cfg.email, store.invite_response(id).unwrap()).unwrap();
    assert!(again.responded);
    assert_eq!(again.my_partstat, "TENTATIVE");

    // The test SMTP server delivers into the Maildir with the envelope recorded.
    let base = std::env::var("NM_TEST_BASE").unwrap_or_else(|_| "/workspace/testmail".into());
    let dir = format!("{base}/mail/kaan/Maildir/new");
    let mut found = None;
    for _ in 0..50 {
        found = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| std::fs::read(e.ok()?.path()).ok())
            .find(|b| String::from_utf8_lossy(b).contains(&built.message_id));
        if found.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let delivered = found.expect("REPLY delivered");
    let text = String::from_utf8_lossy(&delivered);
    assert!(text.contains("X-Test-Envelope-To: elena@example.test"));
    assert!(text.contains(&format!("In-Reply-To: <inv-{uid}>")));
    let ics_back = parse::calendar_part(&delivered).expect("REPLY has text/calendar");
    let cal = ical::parse(&ics_back).unwrap();
    assert_eq!(cal.method, "REPLY");
    assert_eq!(cal.events[0].uid, uid);
    assert_eq!(cal.events[0].attendees[0].partstat, "TENTATIVE");
    let ct_line = text
        .lines()
        .find(|l| {
            l.to_ascii_lowercase()
                .starts_with("content-type: text/calendar")
        })
        .unwrap()
        .to_ascii_lowercase();
    assert!(
        ct_line.contains("method=\"reply\"") || ct_line.contains("method=reply"),
        "{ct_line}"
    );
}

#[test]
fn imip_rejects_bad_input() {
    // No server needed: build only.
    let mut cfg = neutron_core::model::AccountConfig {
        extra_ca_pem: None,
        ..dummy()
    };
    cfg.email = "kaan@localhost".into();
    assert!(smtp::build_imip(&cfg, "a@b.test", "x\r\nBcc: e@f", "t", "i", "REPLY", None).is_err());
    assert!(smtp::build_imip(&cfg, "a@b.test", "x", "t", "i", "RE PLY", None).is_err());
}

fn dummy() -> neutron_core::model::AccountConfig {
    neutron_core::model::AccountConfig {
        email: "kaan@localhost".into(),
        display_name: "Kaan Durmus".into(),
        username: "kaan@localhost".into(),
        imap_host: "localhost".into(),
        imap_port: 993,
        imap_security: Security::Tls,
        smtp_host: "localhost".into(),
        smtp_port: 587,
        smtp_security: Security::Starttls,
        extra_ca_pem: None,
    }
}
