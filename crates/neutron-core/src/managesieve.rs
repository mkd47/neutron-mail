//! Minimal ManageSieve client (RFC 5804): STARTTLS is mandatory before
//! authentication (no plaintext fallback, certificates verified as for
//! IMAP), then AUTHENTICATE PLAIN, PUTSCRIPT/CHECKSCRIPT, SETACTIVE,
//! LISTSCRIPTS, GETSCRIPT, DELETESCRIPT. All responses, including the
//! unauthenticated greeting, share a 2 MiB wire-byte and 16,384-token limit.

use std::time::Duration;

use base64::Engine as _;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::model::AccountConfig;
use crate::{tls, Error, Result};

pub const DEFAULT_PORT: u16 = 4190;
pub const SCRIPT_NAME: &str = "neutron-mail";
const MAX_LINE: usize = 64 * 1024;
const MAX_LITERAL: usize = 1024 * 1024;
// Enough for a maximum-sized script plus response framing, while bounding
// memory across repeated literals and rows (also before STARTTLS).
const MAX_RESPONSE_BYTES: usize = 2 * MAX_LITERAL;
const MAX_RESPONSE_TOKENS: usize = 16_384;
const MAX_RESPONSE_ROWS: usize = 10_000;
const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Capabilities {
    pub implementation: String,
    pub sieve_extensions: Vec<String>,
    pub starttls: bool,
    pub sasl: Vec<String>,
}

/// One server response: data items before the final line, and the final
/// status with its human-readable text.
#[derive(Debug)]
struct Response {
    items: Vec<Vec<String>>,
    ok: bool,
    text: String,
}

struct ResponseBudget {
    bytes: usize,
    tokens: usize,
}

impl ResponseBudget {
    fn new() -> Self {
        Self {
            bytes: MAX_RESPONSE_BYTES,
            tokens: MAX_RESPONSE_TOKENS,
        }
    }

    fn charge_bytes(&mut self, bytes: usize) -> Result<()> {
        self.bytes = self
            .bytes
            .checked_sub(bytes)
            .ok_or_else(|| err("response too large"))?;
        Ok(())
    }

    fn charge_token(&mut self) -> Result<()> {
        self.tokens = self
            .tokens
            .checked_sub(1)
            .ok_or_else(|| err("too many response tokens"))?;
        Ok(())
    }
}

struct Conn<S> {
    io: BufReader<S>,
}

pub struct Client {
    conn: Conn<tokio_rustls::client::TlsStream<TcpStream>>,
    pub caps: Capabilities,
}

fn err(m: impl Into<String>) -> Error {
    Error::Invalid(format!("ManageSieve: {}", m.into()))
}

async fn timed<T>(f: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(TIMEOUT, f)
        .await
        .map_err(|_| Error::Timeout)?
}

/// Quote a string for the protocol (RFC 5804 §4: quoted, no CR/LF).
fn quote(s: &str) -> Result<String> {
    if s.contains(['\r', '\n', '\0']) || s.len() > 1024 {
        return Err(err("bad string"));
    }
    Ok(format!(
        "\"{}\"",
        s.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

impl<S: AsyncRead + AsyncWrite + Unpin> Conn<S> {
    async fn read_line(&mut self, budget: &mut ResponseBudget) -> Result<String> {
        let limit = MAX_LINE.min(budget.bytes);
        if limit == 0 {
            return Err(err("response too large"));
        }
        let mut buf = Vec::new();
        let n = (&mut self.io)
            .take(limit as u64)
            .read_until(b'\n', &mut buf)
            .await?;
        if n == 0 {
            return Err(err("connection closed"));
        }
        budget.charge_bytes(n)?;
        if !buf.ends_with(b"\n") {
            return Err(err(if limit < MAX_LINE {
                "response too large"
            } else {
                "line too long"
            }));
        }
        while buf.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
            buf.pop();
        }
        String::from_utf8(buf).map_err(|_| err("non-UTF-8 response"))
    }

    /// Tokenize one logical line (quoted strings, atoms, literals).
    async fn read_tokens(&mut self, budget: &mut ResponseBudget) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut line = self.read_line(budget).await?;
        loop {
            let mut chars = line.char_indices().peekable();
            let mut literal: Option<usize> = None;
            while let Some((i, c)) = chars.next() {
                if c != ' ' {
                    budget.charge_token()?;
                }
                match c {
                    ' ' => {}
                    '"' => {
                        let mut s = String::new();
                        let mut esc = false;
                        for (_, c) in chars.by_ref() {
                            if esc {
                                s.push(c);
                                esc = false;
                            } else if c == '\\' {
                                esc = true;
                            } else if c == '"' {
                                break;
                            } else {
                                s.push(c);
                            }
                        }
                        out.push(s);
                    }
                    '{' => {
                        let end = line[i..].find('}').ok_or_else(|| err("bad literal"))? + i;
                        let n: usize = line[i + 1..end]
                            .trim_end_matches('+')
                            .parse()
                            .map_err(|_| err("bad literal"))?;
                        if n > MAX_LITERAL {
                            return Err(err("literal too large"));
                        }
                        literal = Some(n);
                        break;
                    }
                    '(' => {
                        // Response code, e.g. (WARNINGS) or (QUOTA/MAXSIZE).
                        let end = line[i..].find(')').map(|e| e + i).unwrap_or(line.len() - 1);
                        out.push(line[i..=end].to_string());
                        while chars.peek().is_some_and(|(j, _)| *j <= end) {
                            chars.next();
                        }
                    }
                    _ => {
                        let mut s = String::from(c);
                        while let Some((_, c)) = chars.peek() {
                            if *c == ' ' {
                                break;
                            }
                            s.push(*c);
                            chars.next();
                        }
                        out.push(s);
                    }
                }
            }
            match literal {
                Some(n) => {
                    // Reserve before allocation/read: many individually
                    // legal literals must not evade the response limit.
                    budget.charge_bytes(n)?;
                    let mut data = vec![0u8; n];
                    self.io.read_exact(&mut data).await?;
                    out.push(String::from_utf8(data).map_err(|_| err("non-UTF-8 literal"))?);
                    line = self.read_line(budget).await?;
                }
                None => return Ok(out),
            }
        }
    }

    async fn response(&mut self) -> Result<Response> {
        let mut items = Vec::new();
        let mut budget = ResponseBudget::new();
        loop {
            let t = self.read_tokens(&mut budget).await?;
            let head = t
                .first()
                .map(|s| s.to_ascii_uppercase())
                .unwrap_or_default();
            if head == "OK" || head == "NO" || head == "BYE" {
                let text = t
                    .iter()
                    .skip(1)
                    .filter(|x| !x.starts_with('('))
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ");
                return Ok(Response {
                    items,
                    ok: head == "OK",
                    text,
                });
            }
            if items.len() >= MAX_RESPONSE_ROWS {
                return Err(err("response too long"));
            }
            items.push(t);
        }
    }

    async fn command(&mut self, line: &str) -> Result<Response> {
        let w = self.io.get_mut();
        w.write_all(line.as_bytes()).await?;
        w.write_all(b"\r\n").await?;
        w.flush().await?;
        self.response().await
    }
}

impl Client {
    async fn response(&mut self) -> Result<Response> {
        self.conn.response().await
    }
    async fn command(&mut self, line: &str) -> Result<Response> {
        self.conn.command(line).await
    }

    fn parse_caps(items: &[Vec<String>]) -> Capabilities {
        let mut c = Capabilities::default();
        for it in items {
            let key = it
                .first()
                .map(|s| s.to_ascii_uppercase())
                .unwrap_or_default();
            let val = it.get(1).cloned().unwrap_or_default();
            match key.as_str() {
                "IMPLEMENTATION" => c.implementation = val,
                "SIEVE" => {
                    c.sieve_extensions = val.split_whitespace().map(str::to_string).collect()
                }
                "STARTTLS" => c.starttls = true,
                "SASL" => {
                    c.sasl = val
                        .split_whitespace()
                        .map(|s| s.to_ascii_uppercase())
                        .collect()
                }
                _ => {}
            }
        }
        c
    }

    /// Connect, upgrade with STARTTLS (required) and log in.
    pub async fn connect(cfg: &AccountConfig, port: u16, password: &str) -> Result<Self> {
        if cfg.auth.is_oauth() {
            return Err(err(
                "OAuth accounts are not supported for server-side filters",
            ));
        }
        timed(async {
            let tcp = TcpStream::connect((cfg.imap_host.as_str(), port)).await?;
            let mut c = Conn {
                io: BufReader::new(tcp),
            };
            let greet = c.response().await?;
            let caps = Self::parse_caps(&greet.items);
            if !greet.ok {
                return Err(err(greet.text));
            }
            if !caps.starttls {
                return Err(err(
                    "server does not offer STARTTLS; refusing to send the password in plain text",
                ));
            }
            let r = c.command("STARTTLS").await?;
            if !r.ok {
                return Err(err(format!("STARTTLS refused: {}", r.text)));
            }
            // Nothing buffered may survive the upgrade (STARTTLS injection).
            if !c.io.buffer().is_empty() {
                return Err(err("data after STARTTLS response"));
            }
            let tcp = c.io.into_inner();
            let tls = tls::wrap(tcp, &cfg.imap_host, cfg.extra_ca_pem.as_deref()).await?;
            let mut c = Client {
                conn: Conn {
                    io: BufReader::new(tls),
                },
                caps: Capabilities::default(),
            };
            // RFC 5804 §2.2: capabilities are re-sent after TLS.
            let r = c.response().await?;
            c.caps = Self::parse_caps(&r.items);
            if !c.caps.sasl.iter().any(|m| m == "PLAIN") {
                return Err(err("server does not offer AUTHENTICATE PLAIN"));
            }
            let blob = base64::engine::general_purpose::STANDARD
                .encode(format!("\0{}\0{}", cfg.username, password));
            let r = c
                .command(&format!("AUTHENTICATE \"PLAIN\" \"{blob}\""))
                .await?;
            if !r.ok {
                return Err(err(format!("login failed: {}", r.text)));
            }
            Ok(c)
        })
        .await
    }

    async fn literal_command(&mut self, prefix: &str, data: &str) -> Result<Response> {
        if data.len() > MAX_LITERAL {
            return Err(err("script too large"));
        }
        let io = self.conn.io.get_mut();
        io.write_all(format!("{prefix} {{{}+}}\r\n", data.len()).as_bytes())
            .await?;
        io.write_all(data.as_bytes()).await?;
        io.write_all(b"\r\n").await?;
        io.flush().await?;
        self.response().await
    }

    /// Ask the server to compile a script without storing it.
    pub async fn check_script(&mut self, script: &str) -> Result<()> {
        timed(async {
            let r = self.literal_command("CHECKSCRIPT", script).await?;
            if r.ok {
                Ok(())
            } else {
                Err(err(format!("script rejected: {}", r.text)))
            }
        })
        .await
    }

    pub async fn put_script(&mut self, name: &str, script: &str) -> Result<()> {
        timed(async {
            let r = self
                .literal_command(&format!("PUTSCRIPT {}", quote(name)?), script)
                .await?;
            if r.ok {
                Ok(())
            } else {
                Err(err(format!("upload rejected: {}", r.text)))
            }
        })
        .await
    }

    /// Activate `name`, or deactivate all scripts with "".
    pub async fn set_active(&mut self, name: &str) -> Result<()> {
        timed(async {
            let r = self.command(&format!("SETACTIVE {}", quote(name)?)).await?;
            if r.ok {
                Ok(())
            } else {
                Err(err(r.text))
            }
        })
        .await
    }

    /// (name, active) for every script.
    pub async fn list_scripts(&mut self) -> Result<Vec<(String, bool)>> {
        timed(async {
            let r = self.command("LISTSCRIPTS").await?;
            if !r.ok {
                return Err(err(r.text));
            }
            Ok(r.items
                .into_iter()
                .filter_map(|t| {
                    let name = t.first()?.clone();
                    Some((
                        name,
                        t.get(1).is_some_and(|a| a.eq_ignore_ascii_case("ACTIVE")),
                    ))
                })
                .collect())
        })
        .await
    }

    pub async fn get_script(&mut self, name: &str) -> Result<String> {
        timed(async {
            let r = self.command(&format!("GETSCRIPT {}", quote(name)?)).await?;
            if !r.ok {
                return Err(err(r.text));
            }
            Ok(r.items
                .into_iter()
                .next()
                .and_then(|t| t.into_iter().next())
                .unwrap_or_default())
        })
        .await
    }

    pub async fn delete_script(&mut self, name: &str) -> Result<()> {
        timed(async {
            let r = self
                .command(&format!("DELETESCRIPT {}", quote(name)?))
                .await?;
            if r.ok {
                Ok(())
            } else {
                Err(err(r.text))
            }
        })
        .await
    }

    pub async fn logout(mut self) {
        let _ = timed(async { self.command("LOGOUT").await.map(|_| ()) }).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn parse_response(data: Vec<u8>) -> Result<Response> {
        // Exercise the actual response parser over a bounded in-memory
        // transport, including the path used for a pre-TLS greeting.
        let (read, mut write) = tokio::io::duplex(4096);
        let writer = tokio::spawn(async move {
            let _ = write.write_all(&data).await;
        });
        let mut conn = Conn {
            io: BufReader::new(read),
        };
        let result = tokio::time::timeout(Duration::from_secs(2), conn.response())
            .await
            .expect("bounded response parser stalled");
        writer.abort();
        result
    }

    #[tokio::test]
    async fn maximum_script_and_normal_capabilities_are_accepted() {
        let response = parse_response(
            b"\"IMPLEMENTATION\" \"Dovecot\"\r\n\"STARTTLS\"\r\n\"SASL\" \"PLAIN\"\r\nOK\r\n"
                .to_vec(),
        )
        .await
        .unwrap();
        assert!(response.ok);
        assert!(Client::parse_caps(&response.items).starttls);

        let mut script = format!("{{{MAX_LITERAL}}}\r\n").into_bytes();
        script.extend(vec![b'x'; MAX_LITERAL]);
        script.extend_from_slice(b"\r\nOK\r\n");
        let response = parse_response(script).await.unwrap();
        assert!(response.ok);
        assert_eq!(response.items[0][0].len(), MAX_LITERAL);
    }

    #[tokio::test]
    async fn literal_budget_applies_within_one_row_and_across_rows() {
        for separator in [" ", "\r\n"] {
            let mut data = format!("{{{MAX_LITERAL}}}\r\n").into_bytes();
            data.extend(vec![b'x'; MAX_LITERAL]);
            // No second body is sent. The declared length must fail the
            // aggregate budget before allocating or waiting for its bytes.
            data.extend_from_slice(format!("{separator}{{{MAX_LITERAL}}}\r\n").as_bytes());
            let error = parse_response(data).await.unwrap_err().to_string();
            assert!(error.contains("response too large"), "{error}");
        }
    }

    #[tokio::test]
    async fn many_short_rows_share_the_wire_byte_budget() {
        let row = format!("\"{}\"\r\n", "x".repeat(32 * 1024));
        let mut data = row.repeat(MAX_RESPONSE_BYTES / row.len() + 1).into_bytes();
        data.extend_from_slice(b"OK\r\n");
        let error = parse_response(data).await.unwrap_err().to_string();
        assert!(error.contains("response too large"), "{error}");
    }

    #[tokio::test]
    async fn empty_literals_cannot_create_unbounded_token_storage() {
        let mut data = b"{0}\r\n".repeat(MAX_RESPONSE_TOKENS + 1);
        data.extend_from_slice(b"\r\nOK\r\n");
        let error = parse_response(data).await.unwrap_err().to_string();
        assert!(error.contains("too many response tokens"), "{error}");
    }

    #[tokio::test]
    async fn token_budget_is_shared_across_rows() {
        let mut data = b"\"x\" \"y\"\r\n".repeat(MAX_RESPONSE_TOKENS / 2 + 1);
        data.extend_from_slice(b"OK\r\n");
        let error = parse_response(data).await.unwrap_err().to_string();
        assert!(error.contains("too many response tokens"), "{error}");
    }
}
