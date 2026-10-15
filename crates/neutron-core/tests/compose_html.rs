//! HTML composing: compose sanitizer, multipart/alternative with inline
//! images, and quoting the original's HTML safely. Offline.
use mail_parser::{MessageParser, MimeHeaders};
use neutron_core::compose::{prepare, Mode};
use neutron_core::model::{AccountConfig, OutgoingMessage, Security};
use neutron_core::sanitize::sanitize_compose_html;
use neutron_core::smtp::build;

// 1x1 transparent PNG
const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

fn cfg() -> AccountConfig {
    AccountConfig {
        email: "kaan@example.test".into(),
        display_name: "Kaan Durmus".into(),
        username: "kaan".into(),
        imap_host: "imap.example.test".into(),
        imap_port: 993,
        imap_security: Security::Tls,
        smtp_host: "smtp.example.test".into(),
        smtp_port: 587,
        smtp_security: Security::Starttls,
        extra_ca_pem: None,
        auth: Default::default(),
    }
}

#[test]
fn compose_sanitizer_policy() {
    let dirty = format!(
        r#"<style>body{{display:none}}</style><script>alert(1)</script>
<p class="app-shell" id="x" onclick="alert(1)" style="color: red; position: fixed; top: 0; z-index: 99; background-image: url(https://t.example/x.png)">
<b>bold</b> <a href="javascript:alert(1)">bad</a> <a href="https://ok.example/">ok</a>
<img src="https://tracker.example/p.gif"><img src="data:image/svg+xml;base64,PHN2Zz4=">
<img src="data:image/png;base64,{PNG_B64}" alt="dot"></p>
<blockquote type="cite">quoted</blockquote><iframe src="https://x"></iframe>"#
    );
    let c = sanitize_compose_html(&dirty);
    for bad in [
        "<style",
        "<script",
        "alert",
        "class=",
        "id=",
        "onclick",
        "position",
        "z-index",
        "top:",
        "background-image",
        "tracker.example",
        "svg",
        "<iframe",
        "javascript",
    ] {
        assert!(!c.contains(bad), "{bad:?} must be removed: {c}");
    }
    for good in [
        "color: red",
        "<b>bold</b>",
        "href=\"https://ok.example/\"",
        "data:image/png;base64,",
        "<blockquote type=\"cite\">",
    ] {
        assert!(c.contains(good), "{good:?} must be kept: {c}");
    }
    assert_eq!(c.matches("<img").count(), 1, "src-less images removed: {c}");
}

#[test]
fn html_message_is_multipart_alternative_with_cid_images() {
    let html = format!(
        "<p>Hello <b>team</b><script>x()</script></p><p><img src=\"data:image/png;base64,{PNG_B64}\"> and again <img src=\"data:image/png;base64,{PNG_B64}\"></p>"
    );
    let m = OutgoingMessage {
        to: vec!["Ayse <ayse@example.test>".into()],
        subject: "Rich".into(),
        body: "Hello team".into(),
        html_body: Some(html),
        ..Default::default()
    };
    let built = build(&cfg(), &m, &[], false).unwrap();
    let raw = String::from_utf8_lossy(&built.raw).to_string();
    assert!(raw.contains("multipart/alternative"), "{raw}");
    assert!(raw.contains("multipart/related"), "{raw}");
    let msg = MessageParser::default().parse(&built.raw).unwrap();
    assert!(msg.body_text(0).unwrap().contains("Hello team"));
    let h = msg.body_html(0).unwrap();
    assert!(h.contains("<b>team</b>") && !h.contains("script"), "{h}");
    assert!(!h.contains("data:image"), "data URLs converted: {h}");
    // Both references point at one inline part whose Content-ID matches.
    let inline: Vec<_> = msg
        .parts
        .iter()
        .filter(|p| p.content_id().is_some())
        .collect();
    assert_eq!(inline.len(), 1, "identical images share one part");
    let cid = inline[0].content_id().unwrap();
    assert_eq!(h.matches(&format!("cid:{cid}")).count(), 2, "{h}");
    assert_eq!(inline[0].content_type().unwrap().subtype(), Some("png"));
    assert_eq!(inline[0].contents().len(), 70, "decoded PNG bytes");
}

#[test]
fn plain_message_stays_single_part() {
    let m = OutgoingMessage {
        to: vec!["a@example.test".into()],
        subject: "Plain".into(),
        body: "just text".into(),
        ..Default::default()
    };
    let raw = String::from_utf8(build(&cfg(), &m, &[], false).unwrap().raw).unwrap();
    assert!(!raw.contains("multipart/"), "{raw}");
    assert!(raw.contains("text/plain"));
}

#[test]
fn reply_quotes_original_html_safely() {
    let original = format!(
        "From: \"Eve <script>alert(1)</script>\" <eve@example.test>\r\nTo: kaan@example.test\r\nSubject: Plan\r\nMessage-ID: <p1@example.test>\r\nDate: Wed, 14 Oct 2026 10:00:00 +0000\r\nMIME-Version: 1.0\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><head><style>.x{{position:fixed}}</style></head><body><h2 style=\"color:#2d1b69;position:absolute\">Q3 plan</h2><ul><li><i>ship</i></li></ul><img src=\"https://tracker.example/o.gif\"><img src=\"data:image/png;base64,{PNG_B64}\"></body></html>\r\n"
    );
    let d = prepare(original.as_bytes(), 1, Mode::Reply, "kaan@example.test");
    let h = d.html.expect("html quote");
    assert!(h.contains("<blockquote type=\"cite\""), "{h}");
    assert!(
        h.contains("<h2 style=\"color: #2d1b69\">Q3 plan</h2>"),
        "{h}"
    );
    assert!(h.contains("<i>ship</i>"));
    assert!(h.contains("data:image/png"), "inline image kept");
    for bad in ["<script", "<style", "position", "tracker.example"] {
        assert!(!h.contains(bad), "{bad} in {h}");
    }
    assert!(h.contains("Eve &lt;script&gt;"), "sender escaped: {h}");
    assert!(
        d.body.contains("> Q3 plan") || d.body.contains("Q3 plan"),
        "text quote too"
    );

    let f = prepare(original.as_bytes(), 1, Mode::Forward, "kaan@example.test");
    let fh = f.html.unwrap();
    assert!(
        fh.contains("Forwarded message") && fh.contains("<i>ship</i>"),
        "{fh}"
    );
}

#[test]
fn text_original_is_quoted_as_escaped_html() {
    let original = "From: a@example.test\r\nTo: kaan@example.test\r\nSubject: T\r\nMessage-ID: <t1@example.test>\r\n\r\n1 < 2 & <b>not bold</b>\r\nline two\r\n";
    let d = prepare(original.as_bytes(), 1, Mode::Reply, "kaan@example.test");
    let h = d.html.unwrap();
    assert!(
        h.contains("1 &lt; 2 &amp; &lt;b&gt;not bold&lt;/b&gt;<br>line two"),
        "{h}"
    );
}

#[test]
fn text_alternative_keeps_quotes_lists_and_links() {
    use neutron_core::sanitize::html_to_text;
    let html = sanitize_compose_html(
        "<p>Hi <b>Ayse</b> &amp; team,</p><ul><li>one</li><li>two</li></ul><ol><li>first</li></ol>\
<p>See <a href=\"https://example.org/x?a=1&amp;b=2\">the doc</a>.</p>\
<p>On Monday, Elena wrote:</p><blockquote type=\"cite\"><p>Line A<br>Line B</p><blockquote type=\"cite\">older</blockquote></blockquote>",
    );
    let t = html_to_text(&html);
    assert!(t.starts_with("Hi Ayse & team,\n"), "{t}");
    assert!(t.contains("• one\n• two\n"), "{t}");
    assert!(t.contains("1. first"), "{t}");
    assert!(t.contains("the doc <https://example.org/x?a=1&b=2>"), "{t}");
    assert!(t.contains("> Line A\n> Line B"), "{t}");
    assert!(t.contains(">> older"), "{t}");
    assert!(!t.contains('<') || t.contains("<https://"), "no tags: {t}");
}
