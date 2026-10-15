#!/usr/bin/env python3
"""Local test SMTP submission server for Neutron Mail development.

Listens on 127.0.0.1:10587 with STARTTLS required before AUTH, accepts
user kaan@localhost / testpass, and delivers every accepted message to kaan@localhost through
Dovecot LMTP (127.0.0.1:10024, so Sieve filters run), falling back to the
Maildir (INBOX/new) so it can be read back over IMAP.
NOT for production use.
"""
import os, ssl, sys, time, socket, asyncio
from aiosmtpd.controller import Controller
from aiosmtpd.smtp import AuthResult, LoginPassword

BASE = os.environ.get("NM_TEST_BASE", "/workspace/testmail")
MAILDIR = os.path.join(BASE, "mail/kaan/Maildir")
USER, PASS = b"kaan@localhost", b"testpass"


def auth(server, session, envelope, mechanism, auth_data):
    if isinstance(auth_data, LoginPassword) and auth_data.login == USER and auth_data.password == PASS:
        return AuthResult(success=True)
    return AuthResult(success=False, handled=False)


LMTP_PORT = int(os.environ.get("NM_LMTP_PORT", "10024"))


def lmtp_deliver(data):
    """Deliver through Dovecot LMTP so the user's active Sieve script runs."""
    import smtplib
    with smtplib.LMTP("127.0.0.1", LMTP_PORT, timeout=10) as l:
        l.sendmail("smtp-test@localhost", [USER.decode()], data)


class Deliver:
    async def handle_DATA(self, server, session, envelope):
        trace = ("X-Test-Envelope-From: %s\r\nX-Test-Envelope-To: %s\r\n"
                 % (envelope.mail_from, ", ".join(envelope.rcpt_tos))).encode()
        data = trace + (envelope.original_content or envelope.content)
        try:
            await asyncio.get_running_loop().run_in_executor(None, lmtp_deliver, data)
            return "250 OK delivered via LMTP"
        except OSError:
            pass  # no LMTP listener: drop straight into the Maildir
        os.makedirs(os.path.join(MAILDIR, "new"), exist_ok=True)
        os.makedirs(os.path.join(MAILDIR, "tmp"), exist_ok=True)
        os.makedirs(os.path.join(MAILDIR, "cur"), exist_ok=True)
        name = f"{time.time_ns()}.{os.getpid()}.{socket.gethostname()}"
        tmp = os.path.join(MAILDIR, "tmp", name)
        # Like a real MTA's Received/X-Original-To trace headers: record the
        # SMTP envelope so tests can verify Bcc handling (envelope only).
        trace = ("X-Test-Envelope-From: %s\r\nX-Test-Envelope-To: %s\r\n"
                 % (envelope.mail_from, ", ".join(envelope.rcpt_tos))).encode()
        with open(tmp, "wb") as f:
            f.write(trace + (envelope.original_content or envelope.content))
        os.rename(tmp, os.path.join(MAILDIR, "new", name))
        return "250 OK delivered"


def main():
    ctx = ssl.create_default_context(ssl.Purpose.CLIENT_AUTH)
    ctx.load_cert_chain(os.path.join(BASE, "certs/cert.pem"), os.path.join(BASE, "certs/key.pem"))
    port = int(os.environ.get("NM_SMTP_PORT", "10587"))
    c = Controller(Deliver(), hostname="127.0.0.1", port=port, tls_context=ctx,
                   require_starttls=True, authenticator=auth, auth_required=True,
                   auth_require_tls=True)
    c.start()
    print(f"test SMTP on 127.0.0.1:{port}", flush=True)
    try:
        while True:
            time.sleep(3600)
    finally:
        c.stop()


if __name__ == "__main__":
    main()
