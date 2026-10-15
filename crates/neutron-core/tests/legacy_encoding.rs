use encoding_rs::{Encoding, BIG5, GBK, SHIFT_JIS};
use neutron_core::parse;

fn encoded_mail(charset: &str, encoding: &'static Encoding, text: &str, transfer: &str) -> Vec<u8> {
    let (bytes, _, errors) = encoding.encode(text);
    assert!(!errors);
    let body = match transfer {
        "base64" => parse::base64(&bytes),
        "quoted-printable" => bytes.iter().map(|b| format!("={b:02X}")).collect(),
        _ => unreachable!(),
    };
    format!("From: =?{charset}?B?{}?= <sender@example.test>\r\nSubject: =?{charset}?B?{}?=\r\nContent-Type: text/plain; charset={charset}\r\nContent-Transfer-Encoding: {transfer}\r\n\r\n{body}", parse::base64(&bytes), parse::base64(&bytes)).into_bytes()
}

#[test]
fn labelled_multibyte_headers_and_bodies_decode() {
    for (charset, encoding, text) in [
        ("GB2312", GBK, "春秋航空采购通知"),
        ("GBK", GBK, "航材管理系统"),
        ("Shift_JIS", SHIFT_JIS, "日本語メール"),
        ("Big5", BIG5, "採購通知"),
    ] {
        for transfer in ["base64", "quoted-printable"] {
            let p = parse::parse(&encoded_mail(charset, encoding, text, transfer));
            assert_eq!(p.subject, text, "{charset}/{transfer}");
            assert_eq!(p.from_name, text);
            assert_eq!(p.text, text);
            assert_eq!(p.preview, text);
        }
    }
}

#[test]
fn raw_gbk_display_headers_and_unlabelled_body_recover() {
    let text = "春秋航空采购通知";
    let (bytes, _, _) = GBK.encode(text);
    let mut raw = b"From: ".to_vec();
    raw.extend_from_slice(&bytes);
    raw.extend_from_slice(b" <sender@example.test>\r\nSubject: ");
    raw.extend_from_slice(&bytes);
    raw.extend_from_slice(b"\r\nContent-Type: text/plain\r\n\r\n");
    raw.extend_from_slice(&bytes);
    let p = parse::parse(&raw);
    assert_eq!(p.subject, text);
    assert_eq!(p.from_name, text);
    assert_eq!(p.text, text);
}

#[test]
fn mislabeled_utf8_base64_html_recovers_without_changing_attachment_bytes() {
    let text = "<p>采购通知</p>";
    let (bytes, _, _) = GBK.encode(text);
    let binary = [0xff, 0x00, 0xb2, 0xc9, 0xb9, 0xba];
    let raw = format!("MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n--x\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=a.bin\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n--x--\r\n",parse::base64(&bytes),parse::base64(&binary));
    let p = parse::parse(raw.as_bytes());
    assert_eq!(p.html_raw.as_deref(), Some(text));
    assert!(p.preview.contains("采购通知"));
    assert_eq!(
        parse::attachment_content(raw.as_bytes(), 0).unwrap().data,
        binary
    );
}

#[test]
fn valid_utf8_and_explicit_western_charset_are_preserved() {
    let p = parse::parse("Subject: Zürich 日本 😀\r\n\r\nTürkçe 中文 😀".as_bytes());
    assert_eq!(p.subject, "Zürich 日本 😀");
    assert_eq!(p.text, "Türkçe 中文 😀");
    let p = parse::parse(b"Content-Type: text/plain; charset=iso-8859-1\r\n\r\nCaf\xe9");
    assert_eq!(p.text, "Café");
}
