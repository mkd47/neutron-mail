#!/usr/bin/env python3
"""APPEND a calendar invitation (iMIP REQUEST) to the test INBOX.
Usage: seed_invite.py [days_from_now] [hour_local]"""
import base64, imaplib, os, ssl, sys, time, datetime
BASE = os.environ.get("NM_TEST_BASE", "/workspace/testmail")
days = int(sys.argv[1]) if len(sys.argv) > 1 else 3
hour = int(sys.argv[2]) if len(sys.argv) > 2 else 14
start = (datetime.datetime.now().astimezone() + datetime.timedelta(days=days)).replace(hour=hour, minute=0, second=0, microsecond=0)
end = start + datetime.timedelta(hours=1)
f = lambda d: d.astimezone(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
uid = f"invite-{time.time_ns()}@example.test"
ics = (f"BEGIN:VCALENDAR\r\nPRODID:-//Example Corp//Calendar//EN\r\nVERSION:2.0\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\n"
       f"UID:{uid}\r\nSEQUENCE:0\r\nDTSTAMP:{f(datetime.datetime.now())}\r\nDTSTART:{f(start)}\r\nDTEND:{f(end)}\r\n"
       "SUMMARY:Q4 roadmap review\r\nLOCATION:Meeting room Bosphorus / video call\r\n"
       "DESCRIPTION:Let's go through the Q4 roadmap.\r\n"
       "ORGANIZER;CN=Elena Marsh:mailto:elena@example.test\r\n"
       "ATTENDEE;CN=Kaan Durmus;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:kaan@localhost\r\n"
       "ATTENDEE;CN=Ayşe Yılmaz;PARTSTAT=ACCEPTED:mailto:ayse@example.test\r\n"
       "END:VEVENT\r\nEND:VCALENDAR\r\n")
msg = ("From: Elena Marsh <elena@example.test>\r\nTo: Kaan Durmus <kaan@localhost>\r\n"
       f"Subject: Invitation: Q4 roadmap review @ {start:%a %d %b %H:%M}\r\n"
       f"Date: {time.strftime('%a, %d %b %Y %H:%M:%S %z')}\r\nMessage-ID: <{uid}>\r\nMIME-Version: 1.0\r\n"
       "Content-Type: multipart/alternative; boundary=\"alt\"\r\n\r\n--alt\r\n"
       "Content-Type: text/plain; charset=utf-8\r\n\r\nElena Marsh has invited you to Q4 roadmap review.\r\n--alt\r\n"
       "Content-Type: text/calendar; charset=utf-8; method=REQUEST\r\nContent-Transfer-Encoding: base64\r\n\r\n"
       + base64.encodebytes(ics.encode()).decode().replace("\n", "\r\n") + "--alt--\r\n")
ctx = ssl.create_default_context(cafile=f"{BASE}/certs/ca.pem")
m = imaplib.IMAP4_SSL("localhost", 10993, ssl_context=ctx)
m.login("kaan@localhost", "testpass")
print(m.append("INBOX", None, None, msg.encode()))
print(uid)
