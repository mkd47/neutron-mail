//! Reading messages out of mbox files (Thunderbird, Apple Mail and Gmail
//! Takeout exports) and single .eml files, for import. Messages are
//! streamed one at a time, so large archives don't have to fit in memory.
//!
//! Lines starting with "From " after an empty line (or at the very start)
//! separate messages; ">From " quoting (mboxrd/mboxo) is undone, and line
//! ends become CRLF as IMAP APPEND expects.

use std::io::BufRead;

/// Largest single message imported; bigger ones are skipped.
pub const MAX_MESSAGE_BYTES: usize = 50 * 1024 * 1024;

/// One message from the file: its bytes (CRLF), or `None` when it was too
/// large and skipped.
pub type Item = Option<Vec<u8>>;

pub struct Reader<R> {
    input: R,
    line: Vec<u8>,
    /// A separator line was read that starts the next message.
    pending_start: bool,
    started: bool,
    done: bool,
}

impl<R: BufRead> Reader<R> {
    pub fn new(input: R) -> Self {
        Self {
            input,
            line: Vec::new(),
            pending_start: false,
            started: false,
            done: false,
        }
    }
}

/// True for an mbox file (first line "From …"); false for a single message.
pub fn looks_like_mbox(start: &[u8]) -> bool {
    start.starts_with(b"From ")
}

/// A whole file as one message (.eml), with CRLF line ends.
pub fn single(mut input: impl BufRead) -> std::io::Result<Item> {
    let mut msg = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        if input.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        let body = line
            .strip_suffix(b"\n")
            .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
            .unwrap_or(&line);
        if msg.len() + body.len() + 2 > MAX_MESSAGE_BYTES {
            return Ok(None);
        }
        msg.extend_from_slice(body);
        msg.extend_from_slice(b"\r\n");
    }
    Ok(Some(msg))
}

fn is_separator(line: &[u8]) -> bool {
    line.starts_with(b"From ")
}

/// ">From " → "From ", ">>From " → ">From " (mboxrd); plain mboxo files
/// only ever have one level, so this is right for both.
fn unquote(line: &[u8]) -> &[u8] {
    let gt = line.iter().take_while(|&&b| b == b'>').count();
    if gt > 0 && line[gt..].starts_with(b"From ") {
        &line[1..]
    } else {
        line
    }
}

impl<R: BufRead> Iterator for Reader<R> {
    type Item = std::io::Result<Item>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let mut msg: Vec<u8> = Vec::new();
        let mut too_big = false;
        let mut prev_blank = true;
        let mut have_content = false;
        // The separator of this message was consumed by the previous call.
        let mut in_message = self.pending_start;
        self.pending_start = false;
        loop {
            self.line.clear();
            match self.input.read_until(b'\n', &mut self.line) {
                Ok(0) => {
                    self.done = true;
                    break;
                }
                Ok(_) => {}
                Err(e) => return Some(Err(e)),
            }
            let body = self
                .line
                .strip_suffix(b"\n")
                .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
                .unwrap_or(&self.line);
            if is_separator(body) && (prev_blank || !self.started) {
                self.started = true;
                if in_message && have_content {
                    self.pending_start = true;
                    break;
                }
                in_message = true;
                continue;
            }
            self.started = true;
            in_message = true;
            let text = unquote(body);
            if !too_big {
                if msg.len() + text.len() + 2 > MAX_MESSAGE_BYTES {
                    too_big = true;
                    msg = Vec::new();
                } else {
                    msg.extend_from_slice(text);
                    msg.extend_from_slice(b"\r\n");
                }
            }
            have_content = true;
            prev_blank = body.is_empty();
        }
        if !have_content {
            return None;
        }
        if too_big {
            return Some(Ok(None));
        }
        // The empty line before the next separator belongs to the format.
        if msg.ends_with(b"\r\n\r\n") {
            msg.truncate(msg.len() - 2);
        }
        Some(Ok(Some(msg)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(input: &str) -> Vec<String> {
        Reader::new(input.as_bytes())
            .map(|m| String::from_utf8(m.unwrap().unwrap()).unwrap())
            .collect()
    }

    #[test]
    fn splits_mbox_and_unquotes_from_lines() {
        let mbox = "From a@x Mon Jan  1 00:00:00 2024\nSubject: one\n\nHello\n>From the start\nFrom inside a line is fine\n\nFrom b@x Tue Jan  2 00:00:00 2024\r\nSubject: two\r\n\r\nBye\r\n";
        let m = all(mbox);
        assert_eq!(m.len(), 2);
        assert_eq!(
            m[0],
            "Subject: one\r\n\r\nHello\r\nFrom the start\r\nFrom inside a line is fine\r\n"
        );
        assert_eq!(m[1], "Subject: two\r\n\r\nBye\r\n");
    }

    #[test]
    fn a_single_eml_is_one_message() {
        assert!(!looks_like_mbox(b"Subject: x\r\n"));
        let eml = single("Subject: x\n\nFrom here on\n".as_bytes())
            .unwrap()
            .unwrap();
        assert_eq!(eml, b"Subject: x\r\n\r\nFrom here on\r\n");
        let m = all("Subject: only\n\nBody\n");
        assert_eq!(m, vec!["Subject: only\r\n\r\nBody\r\n"]);
        assert!(all("").is_empty());
    }
}
