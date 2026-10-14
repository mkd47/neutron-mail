#!/usr/bin/env bash
# Start a throwaway local mail server for Neutron Mail development/tests.
#   IMAP  : Dovecot 2.4, 127.0.0.1:10993 (TLS) and :10143 (STARTTLS)
#   SMTP  : Python aiosmtpd, 127.0.0.1:10587 (STARTTLS required, AUTH)
#   User  : kaan@localhost / testpass
#   CA    : $NM_TEST_BASE/certs/ca.pem (private test CA; leaf SAN localhost,127.0.0.1)
# Requires: dovecot-imapd (2.4), python3-aiosmtpd, openssl. Runs as your user.
set -euo pipefail
BASE="${NM_TEST_BASE:-/workspace/testmail}"
HERE="$(cd "$(dirname "$0")" && pwd)"
DOVECOT="${DOVECOT:-/usr/sbin/dovecot}"
DOVEADM="${DOVEADM:-/usr/bin/doveadm}"
mkdir -p "$BASE"/{run/state,mail,certs}
if [[ ! -f "$BASE/certs/ca.pem" ]]; then
  # Private test CA + server leaf certificate (mirrors a self-hosted server
  # with its own CA). Clients trust ca.pem explicitly; validation stays on.
  openssl req -x509 -newkey rsa:2048 -nodes -keyout "$BASE/certs/ca.key" -out "$BASE/certs/ca.pem" \
    -days 365 -subj "/CN=Neutron Mail Test CA" \
    -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" 2>/dev/null
  openssl req -newkey rsa:2048 -nodes -keyout "$BASE/certs/key.pem" -out "$BASE/certs/server.csr" \
    -subj "/CN=localhost" 2>/dev/null
  printf "subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n" > "$BASE/certs/leaf.ext"
  openssl x509 -req -in "$BASE/certs/server.csr" -CA "$BASE/certs/ca.pem" -CAkey "$BASE/certs/ca.key" \
    -CAcreateserial -out "$BASE/certs/cert.pem" -days 365 -extfile "$BASE/certs/leaf.ext" 2>/dev/null
  chmod 600 "$BASE/certs/"*.key "$BASE/certs/key.pem"
fi
PW=$("$DOVEADM" pw -s SHA512-CRYPT -p testpass)
echo "kaan@localhost:$PW:$(id -u):$(id -g)::$BASE/mail/kaan" > "$BASE/users"
sed -e "s#@BASE@#$BASE#g" -e "s#@USER@#$(id -un)#g" -e "s#@GROUP@#$(id -gn)#g" \
  "$HERE/dovecot.conf.in" > "$BASE/dovecot.conf"
if [[ -f "$BASE/run/master.pid" ]] && kill -0 "$(cat "$BASE/run/master.pid")" 2>/dev/null; then
  kill "$(cat "$BASE/run/master.pid")"; sleep 2
fi
"$DOVECOT" -c "$BASE/dovecot.conf"
pkill -f "smtp_server.py" 2>/dev/null || true
NM_TEST_BASE="$BASE" nohup python3 "$HERE/smtp_server.py" > "$BASE/smtp.log" 2>&1 &
sleep 2
echo "IMAP 127.0.0.1:10993/10143, SMTP 127.0.0.1:10587, CA $BASE/certs/ca.pem"
