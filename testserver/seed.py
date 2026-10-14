#!/usr/bin/env python3
"""Seed the Dovecot test mailbox over IMAP (APPEND) with sample messages:
plain text, HTML with remote images/scripts, and attachments."""
import imaplib, ssl, time, os
from email.message import EmailMessage
from email.utils import formatdate, make_msgid

ctx = ssl.create_default_context(cafile=os.environ.get("NM_TEST_CA", "/workspace/testmail/certs/ca.pem"))
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
       attach=[("invoice-4711.pdf", b"%PDF-1.4\n% fake test pdf\n", "application/pdf"),
               ("notes.txt", b"Payment due in 30 days.\n", "text/plain")], days_ago=1),
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
