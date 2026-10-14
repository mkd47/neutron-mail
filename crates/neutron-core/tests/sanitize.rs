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
    assert_eq!(blocked, 4, "{h}");
    for bad in ["tracker.test", "x.test", "cdn.test", "evil.test", "style="] {
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
