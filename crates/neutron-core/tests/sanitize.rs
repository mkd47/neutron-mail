use neutron_core::sanitize::{sanitize_html, wrap_document, MESSAGE_CSP};

fn clean(s: &str) -> (String, usize) {
    let r = sanitize_html(s);
    (r.html.to_lowercase(), r.remote_images_blocked)
}

#[test]
fn removes_scripts_and_event_handlers() {
    let (h, _) = clean(
        r#"<p onclick="alert(1)" onmouseover=x>hi</p><script>alert('x')</script><svg onload=alert(1)>"#,
    );
    assert!(h.contains("hi"));
    for bad in [
        "<script",
        "alert",
        "onclick",
        "onmouseover",
        "onload",
        "<svg",
    ] {
        assert!(!h.contains(bad), "{bad} survived: {h}");
    }
}

#[test]
fn removes_active_and_embedding_elements() {
    let (h, _) = clean(
        r#"<iframe src="https://evil.test"></iframe><object data=x></object><embed src=x>
        <form action="https://evil.test"><input name=p><button>go</button></form>
        <meta http-equiv="refresh" content="0;url=https://evil.test"><base href="https://evil.test/">
        <link rel=stylesheet href="https://evil.test/a.css">"#,
    );
    for bad in [
        "<iframe",
        "<object",
        "<embed",
        "<form",
        "<input",
        "<meta",
        "<base",
        "<link",
        "evil.test",
    ] {
        assert!(!h.contains(bad), "{bad} survived: {h}");
    }
}

#[test]
fn strips_dangerous_urls() {
    let (h, _) = clean(
        r#"<a href="javascript:alert(1)">a</a><a href="JaVaScRiPt:alert(1)">b</a>
        <a href="data:text/html,<script>alert(1)</script>">c</a><a href="vbscript:x">d</a>
        <a href="/relative">e</a><a href="https://ok.example/">ok</a><a href="mailto:a@b.c">m</a>"#,
    );
    assert!(!h.contains("javascript:"));
    assert!(!h.contains("vbscript:"));
    assert!(!h.contains("data:text"));
    assert!(!h.contains("href=\"/relative\""));
    assert!(h.contains("href=\"https://ok.example/\""));
    assert!(h.contains("href=\"mailto:a@b.c\""));
    assert!(h.contains("noopener noreferrer nofollow"));
}

#[test]
fn blocks_remote_images_and_css_loads() {
    let (h, blocked) = clean(
        r#"<img src="https://tracker.test/p.gif" width=1>
        <img src="http://x.test/a.png"><img srcset="https://x.test/a.png 2x">
        <img src="//cdn.test/b.png"><div style="background:url(https://evil.test/bg.png)">s</div>
        <style>body{background:url(https://evil.test/x.png)}</style>"#,
    );
    // 4 <img> + 2 CSS url() loads.
    assert_eq!(blocked, 6, "{h}");
    for bad in ["tracker.test", "x.test", "cdn.test", "evil.test"] {
        assert!(!h.contains(bad), "{bad} survived: {h}");
    }
}

#[test]
fn keeps_inline_raster_images_but_not_svg() {
    let (h, blocked) = clean(
        r#"<img src="data:image/png;base64,iVBORw0KGgo=" alt=a>
        <img src="data:image/svg+xml;base64,PHN2ZyBvbmxvYWQ9YWxlcnQoMSk+" alt=b>"#,
    );
    assert!(h.contains("data:image/png"));
    assert!(!h.contains("svg+xml"));
    assert_eq!(blocked, 1);
}

#[test]
fn keeps_basic_formatting() {
    let (h, _) = clean("<h1>T</h1><p><b>bold</b> <i>it</i></p><table><tr><td>c</td></tr></table><ul><li>x</li></ul>");
    for ok in ["<h1>", "<b>", "<i>", "<table>", "<td>", "<li>"] {
        assert!(h.contains(ok), "{ok} lost: {h}");
    }
}

#[test]
fn document_wrapper_has_strict_csp() {
    let d = wrap_document("<p>x</p>", true);
    assert!(d.contains(MESSAGE_CSP));
    assert!(MESSAGE_CSP.contains("default-src 'none'"));
    assert!(!MESSAGE_CSP.contains("script-src"));
}

#[test]
fn keeps_safe_css_and_layout() {
    let input = r##"<style>.card{border-radius:8px;background:#0a66c2;color:#fff} @import url("https://evil.test/a.css"); .x{font-weight:bold}</style>
        <table bgcolor="#f3f2ef" width="600" cellpadding="0" align="center"><tr><td class="card" style="padding:12px;color:#191919">Hi</td></tr></table>
        <font color="red" face="Arial">old school</font><center>c</center>"##;
    let s = sanitize_html(input);
    let h = &s.html;
    assert!(
        h.contains(".card{border-radius:8px;background:#0a66c2"),
        "{h}"
    );
    assert!(h.contains("style=\"padding:12px;color:#191919\""), "{h}");
    assert!(h.contains("bgcolor=\"#f3f2ef\""), "{h}");
    assert!(h.contains("class=\"card\""), "{h}");
    assert!(h.contains("<font color=\"red\""), "{h}");
    assert!(!h.contains("@import") && !h.contains("evil.test"), "{h}");
}

#[test]
fn drops_dangerous_css() {
    for bad in [
        r#"<div style="width:expression(alert(1))">x</div>"#,
        r#"<div style="-moz-binding:url(https://e.test/x.xml#x)">x</div>"#,
        r#"<div style="background:url(javascript:alert(1))">x</div>"#,
        r#"<style>div{behavior:url(x.htc)}</style>"#,
    ] {
        let h = sanitize_html(bad).html;
        for needle in ["expression", "binding", "javascript", "behavior", "e.test"] {
            assert!(!h.contains(needle), "{needle} survived in {h}");
        }
    }
}

#[test]
fn remote_images_substituted_only_when_fetched() {
    use std::collections::HashMap;
    let input = r#"<img src="https://cdn.example.org/logo.png"><img src="https://other.example.org/p.gif"><div style="background:url('https://cdn.example.org/bg.png')">x</div>"#;
    let s = sanitize_html(input);
    assert_eq!(s.remote_urls.len(), 3);
    assert_eq!(s.remote_images_blocked, 3);
    let mut m = HashMap::new();
    m.insert(
        "https://cdn.example.org/logo.png".to_string(),
        "data:image/png;base64,iVBORw0KGgo=".to_string(),
    );
    m.insert(
        "https://cdn.example.org/bg.png".to_string(),
        "data:image/png;base64,iVBORw0KGgo=".to_string(),
    );
    let t = neutron_core::sanitize::sanitize_html_with(input, &m);
    assert_eq!(t.remote_images_loaded, 2);
    assert_eq!(t.remote_images_blocked, 1);
    assert!(
        !t.html.contains("cdn.example.org") && !t.html.contains("other.example.org"),
        "{}",
        t.html
    );
    assert!(t
        .html
        .contains("src=\"data:image/png;base64,iVBORw0KGgo=\""));
}

#[test]
fn style_tag_cannot_break_out() {
    let h =
        sanitize_html("<style>a{}</style><script>alert(1)</script><style>b{}</sty</style>").html;
    assert!(!h.contains("<script"), "{h}");
}

#[test]
fn coloured_mail_stays_on_a_light_page_in_dark_mode() {
    use neutron_core::sanitize::sets_own_colors;
    for coloured in [
        r##"<p style="color: #222">Dear Musa</p>"##,
        r##"<td style="BACKGROUND-COLOR:#fff">x</td>"##,
        r##"<table bgcolor="#f3f2ef"><tr><td>x</td></tr></table>"##,
        r##"<font color="#1f3864">Signature</font>"##,
        r##"<style>p{color :black}</style><p>x</p>"##,
    ] {
        assert!(sets_own_colors(coloured), "{coloured}");
        assert!(
            wrap_document(coloured, true).contains("html{background:#ffffff"),
            "{coloured}"
        );
    }
    let plain = "<p>Hello <b>there</b></p>";
    assert!(!sets_own_colors(plain));
    assert!(wrap_document(plain, true).contains("html{background:#1c1b22"));
    assert!(wrap_document(plain, false).contains("html{background:#ffffff"));
}
