//! Attachment safety: filename sanitizing (no path traversal, no hidden
//! bidi tricks), executable detection and size limits.

/// Max size of a single outgoing attachment.
pub const MAX_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;
/// Max total size of all attachments in one outgoing message (most
/// providers reject messages above ~25 MB after base64 expansion anyway).
pub const MAX_TOTAL_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;
/// Max number of attachments per outgoing message.
pub const MAX_ATTACHMENTS: usize = 50;
const MAX_NAME_CHARS: usize = 120;
// Linux filesystems limit each path component by bytes, not Unicode chars.
const MAX_NAME_BYTES: usize = 255;

/// Create `name` in `dir` without ever replacing an existing file: if the
/// name is taken, "name (2).ext", "name (3).ext"… is used instead. `name`
/// must already be a [`safe_filename`]. Returns the path written.
pub fn write_new_file(
    dir: &std::path::Path,
    name: &str,
    data: &[u8],
) -> std::io::Result<std::path::PathBuf> {
    use std::io::Write;
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 1..=999 {
        let candidate = if n == 1 {
            name.to_string()
        } else {
            format!("{stem} ({n}){ext}")
        };
        let path = dir.join(&candidate);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                f.write_all(data)?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "too many files with this name",
    ))
}

/// Turn an attacker-controlled attachment name into a safe single path
/// component: strips directories (`/`, `\`), `..`, control and bidi
/// override characters, characters invalid on common filesystems and
/// leading dots (hidden files). Never returns an empty name.
pub fn safe_filename(name: &str) -> String {
    // Keep only the last path component, whichever separator is used.
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let mut out: String = base
        .chars()
        .filter(|c| !c.is_control() && !is_bidi_or_invisible(*c))
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c => c,
        })
        .collect();
    out = out.trim().trim_start_matches('.').trim().to_string();
    if out.is_empty() {
        out = "attachment".into();
    }
    // Windows reserved device names (harmless on Linux, but files may be
    // copied elsewhere).
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")))
    {
        out.insert(0, '_');
    }
    if out.chars().count() > MAX_NAME_CHARS || out.len() > MAX_NAME_BYTES {
        // Keep both display length and the filesystem byte limit bounded,
        // without splitting UTF-8 or hiding the extension from the user.
        let ext = extension(&out).map(|e| format!(".{e}")).unwrap_or_default();
        let keep_chars = MAX_NAME_CHARS.saturating_sub(ext.chars().count());
        let keep_bytes = MAX_NAME_BYTES.saturating_sub(ext.len());
        out = out
            .char_indices()
            .take(keep_chars)
            .take_while(|(offset, c)| offset + c.len_utf8() <= keep_bytes)
            .map(|(_, c)| c)
            .collect::<String>()
            + &ext;
    }
    out
}

fn is_bidi_or_invisible(c: char) -> bool {
    matches!(c,
        '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' | '\u{061C}')
}

fn extension(name: &str) -> Option<String> {
    let (_, ext) = name.rsplit_once('.')?;
    (!ext.is_empty() && ext.len() <= 12).then(|| ext.to_ascii_lowercase())
}

const RISKY_EXTENSIONS: &[&str] = &[
    // Linux / cross-platform
    "sh",
    "bash",
    "zsh",
    "csh",
    "ksh",
    "fish",
    "run",
    "bin",
    "elf",
    "appimage",
    "desktop",
    "deb",
    "rpm",
    "flatpak",
    "flatpakref",
    "snap",
    "jar",
    "py",
    "pyc",
    "pl",
    "rb",
    "php",
    "js",
    "mjs",
    "jse",
    "so",
    "ko",
    "out",
    // Windows
    "exe",
    "com",
    "bat",
    "cmd",
    "scr",
    "msi",
    "msp",
    "pif",
    "cpl",
    "dll",
    "hta",
    "vbs",
    "vbe",
    "wsf",
    "wsh",
    "ps1",
    "psm1",
    "reg",
    "lnk",
    "inf",
    "gadget",
    "application",
    // macOS
    "app",
    "command",
    "pkg",
    "dmg",
    // Office macro formats
    "docm",
    "xlsm",
    "pptm",
    // Active documents. Opening these with the default app hands the raw
    // bytes to a browser, which runs scripts. SVG is included because it
    // is XML that can carry script; XHTML, XSLT and MHTML are the same class.
    "html",
    "htm",
    "shtml",
    "shtm",
    "xhtml",
    "xht",
    "svg",
    "svgz",
    "xml",
    "xsl",
    "xslt",
    "mht",
    "mhtml",
];

const RISKY_MIME: &[&str] = &[
    "application/x-executable",
    "application/x-sharedlib",
    "application/x-elf",
    "application/x-sh",
    "application/x-shellscript",
    "text/x-shellscript",
    "application/x-msdownload",
    "application/x-msdos-program",
    "application/x-ms-installer",
    "application/x-msi",
    "application/vnd.microsoft.portable-executable",
    "application/x-desktop",
    "application/java-archive",
    "application/x-python-code",
    "text/x-python",
    "application/javascript",
    "text/javascript",
    "application/x-perl",
    "text/html",
    "application/xhtml+xml",
    "application/vnd.wap.xhtml+xml",
    "image/svg+xml",
    "application/svg+xml",
    "text/xml",
    "application/xml",
    "application/xslt+xml",
    "text/xsl",
    "application/x-httpd-php",
];

/// Whether opening this attachment could execute code. Checks the file
/// extension, the declared MIME type (parameters ignored) and the content's
/// magic bytes (ELF, PE/MZ, shebang, or an HTML/SVG document), so a renamed
/// executable or active document is still caught.
pub fn is_risky(name: &str, mime: &str, data: &[u8]) -> bool {
    let ext_risky =
        extension(&safe_filename(name)).is_some_and(|e| RISKY_EXTENSIONS.contains(&e.as_str()));
    let mime_risky = RISKY_MIME.contains(&mime_essence(mime).as_str());
    let magic_risky = data.starts_with(b"\x7fELF")
        || data.starts_with(b"MZ")
        || data.starts_with(b"#!")
        || looks_like_active_markup(data);
    ext_risky || mime_risky || magic_risky
}

/// `text/html; charset=utf-8` and `TEXT/HTML` both compare as `text/html`.
fn mime_essence(mime: &str) -> String {
    mime.split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// HTML, XHTML and SVG are active even when the filename and the declared
/// type are innocuous: file managers sniff the bytes and open a browser.
/// Only the start of the file is inspected, after a BOM, whitespace, an XML
/// declaration and comments, which is how those documents are usually wrapped.
fn looks_like_active_markup(data: &[u8]) -> bool {
    let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    let head = &data[..data.len().min(1024)];
    let mut rest = String::from_utf8_lossy(head).to_ascii_lowercase();
    loop {
        let trimmed = rest.trim_start_matches(|c: char| c.is_ascii_whitespace());
        if trimmed.len() != rest.len() {
            rest = trimmed.to_string();
        }
        if is_active_markup_start(&rest) {
            return true;
        }
        if let Some(after) = rest.strip_prefix("<?xml") {
            if let Some(end) = after.find("?>") {
                rest = after[end + 2..].to_string();
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix("<!--") {
            if let Some(end) = after.find("-->") {
                rest = after[end + 3..].to_string();
                continue;
            }
        }
        return false;
    }
}

fn is_active_markup_start(s: &str) -> bool {
    if s.starts_with("<!doctype html") {
        return true;
    }
    for tag in ["<html", "<svg", "<script"] {
        if let Some(after) = s.strip_prefix(tag) {
            if after.starts_with([' ', '\t', '\n', '\r', '>', '/']) {
                return true;
            }
        }
    }
    false
}

/// Content-type guess for an outgoing file (by extension; the UI never
/// supplies a MIME type).
pub fn mime_for(name: &str) -> &'static str {
    match extension(name).as_deref() {
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("txt" | "log" | "md") => "text/plain",
        Some("csv") => "text/csv",
        Some("html" | "htm") => "text/html",
        Some("zip") => "application/zip",
        Some("ics") => "text/calendar",
        Some("vcf") => "text/vcard",
        Some("json") => "application/json",
        Some("odt") => "application/vnd.oasis.opendocument.text",
        Some("ods") => "application/vnd.oasis.opendocument.spreadsheet",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
}

/// Why [`read_capped`] refused a file.
#[derive(Debug)]
pub enum CappedReadError {
    /// The file is a regular file larger than the cap, including a file that
    /// grew past the cap while it was being read.
    TooLarge,
    /// The opened descriptor is not a regular file (directory, fifo, device).
    NotRegularFile,
    Io(std::io::Error),
}

impl std::fmt::Display for CappedReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => write!(f, "file is too large"),
            Self::NotRegularFile => write!(f, "not a regular file"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CappedReadError {}

impl From<std::io::Error> for CappedReadError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Read a regular file, refusing anything over `max` bytes.
///
/// The file is opened `O_NONBLOCK` so a fifo or device swapped onto `path`
/// cannot stall the caller, then checked with `fstat` on that descriptor.
/// The read itself stops after `max + 1` bytes: a metadata length from before
/// the read is not a limit, because the file can grow in between.
pub fn read_capped(path: &std::path::Path, max: u64) -> Result<Vec<u8>, CappedReadError> {
    let mut file = open_for_capped_read(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(CappedReadError::NotRegularFile);
    }
    if meta.len() > max {
        return Err(CappedReadError::TooLarge);
    }
    read_limited(&mut file, max)
}

/// Stop after `max + 1` bytes so a reader that never reaches EOF (a file
/// still being appended, or `/dev/zero` if the type check were skipped)
/// cannot exhaust memory.
fn read_limited(reader: &mut impl std::io::Read, max: u64) -> Result<Vec<u8>, CappedReadError> {
    use std::io::Read;
    let mut data = Vec::new();
    reader.take(max.saturating_add(1)).read_to_end(&mut data)?;
    if data.len() as u64 > max {
        return Err(CappedReadError::TooLarge);
    }
    Ok(data)
}

#[cfg(unix)]
fn open_for_capped_read(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_for_capped_read(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_and_tricks() {
        assert_eq!(safe_filename("../../etc/passwd"), "passwd");
        assert_eq!(safe_filename("..\\..\\Windows\\evil.exe"), "evil.exe");
        assert_eq!(safe_filename(".."), "attachment");
        assert_eq!(safe_filename("/"), "attachment");
        assert_eq!(safe_filename(".bashrc"), "bashrc");
        assert_eq!(safe_filename("inv\u{202E}fdp.exe"), "invfdp.exe");
        assert_eq!(safe_filename("a\0b\nc.txt"), "abc.txt");
        assert_eq!(safe_filename("CON.txt"), "_CON.txt");
        assert_eq!(safe_filename("what?.pdf"), "what_.pdf");
        let long = "x".repeat(300) + ".pdf";
        let s = safe_filename(&long);
        assert!(s.ends_with(".pdf") && s.chars().count() == 120);
    }

    #[test]
    fn unicode_filenames_fit_the_filesystem_and_keep_extensions() {
        for (name, ext) in [
            ("报".repeat(90) + ".pdf", ".pdf"),
            ("😀".repeat(100) + ".png", ".png"),
            ("资料".repeat(100) + ".desktop", ".desktop"),
        ] {
            let safe = safe_filename(&name);
            assert!(safe.len() <= MAX_NAME_BYTES);
            assert!(safe.chars().count() <= MAX_NAME_CHARS);
            assert!(safe.ends_with(ext));
            // Exercise the real path component limit as well as the string
            // policy. tempfile is already a dev dependency of this crate.
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(&safe), b"attachment").unwrap();
        }
        assert_eq!(safe_filename("账单.pdf"), "账单.pdf");
        assert!(is_risky(
            &("报".repeat(120) + ".desktop"),
            "text/plain",
            b""
        ));
    }

    #[test]
    fn saving_never_overwrites_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_new_file(dir.path(), "report.pdf", b"one").unwrap();
        let b = write_new_file(dir.path(), "report.pdf", b"two").unwrap();
        let c = write_new_file(dir.path(), "README", b"x").unwrap();
        let d = write_new_file(dir.path(), "README", b"y").unwrap();
        assert_eq!(a.file_name().unwrap(), "report.pdf");
        assert_eq!(b.file_name().unwrap(), "report (2).pdf");
        assert_eq!(d.file_name().unwrap(), "README (2)");
        assert_eq!(std::fs::read(&a).unwrap(), b"one");
        assert_eq!(std::fs::read(&b).unwrap(), b"two");
        assert_eq!(std::fs::read(&c).unwrap(), b"x");
    }

    #[test]
    fn risk() {
        assert!(is_risky("setup.sh", "text/plain", b"echo"));
        assert!(is_risky("invoice.pdf.exe", "application/pdf", b"%PDF"));
        assert!(is_risky("invoice.pdf", "application/pdf", b"MZ\x90\0"));
        assert!(is_risky("photo.jpg", "image/jpeg", b"\x7fELF..."));
        assert!(is_risky("x", "application/x-desktop", b""));
        assert!(!is_risky("invoice.pdf", "application/pdf", b"%PDF-1.4"));
        assert!(!is_risky("notes.txt", "text/plain", b"hello"));
    }

    #[test]
    fn active_documents_require_confirmation() {
        for name in [
            "page.html",
            "page.HTM",
            "page.xhtml",
            "page.xht",
            "page.shtml",
            "logo.svg",
            "logo.svgz",
            "note.xml",
            "style.xsl",
            "style.xslt",
            "saved.mht",
            "saved.mhtml",
            "nested/invoice.html",
        ] {
            assert!(
                is_risky(name, "application/octet-stream", b"not markup"),
                "{name}"
            );
        }
        // Declared type, including parameters and odd case, even with a safe name.
        assert!(is_risky("notes.bin", "text/html", b"hello"));
        assert!(is_risky("notes.bin", "text/html; charset=utf-8", b"hello"));
        assert!(is_risky("notes.bin", "TEXT/HTML; charset=UTF-8", b"hello"));
        assert!(is_risky("logo.bin", "image/svg+xml", b"<svg/>"));
        assert!(is_risky("page.bin", "application/xhtml+xml", b""));
        assert!(is_risky("sheet.bin", "application/xml", b"<data/>"));
        // Renamed: the bytes are what a file manager would sniff.
        assert!(is_risky(
            "invoice.pdf",
            "application/pdf",
            b"<!DOCTYPE html><script>alert(1)</script>"
        ));
        assert!(is_risky(
            "photo.jpg",
            "image/jpeg",
            b"\xef\xbb\xbf  \n<html><script>x</script></html>"
        ));
        assert!(is_risky(
            "image.png",
            "image/png",
            b"<?xml version=\"1.0\"?>\n<!-- logo -->\n<svg xmlns=\"http://www.w3.org/2000/svg\" onload=\"alert(1)\"/>"
        ));
        assert!(!is_risky(
            "notes.txt",
            "text/plain",
            b"see <html> only later in the file"
        ));
        assert!(!is_risky(
            "invoice.pdf",
            "application/pdf",
            b"%PDF-1.7\n<html>"
        ));
        assert!(!is_risky("data.csv", "text/csv", b"a,b\n1,2\n"));
    }

    #[test]
    fn capped_read_stops_at_the_byte_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grow.bin");
        std::fs::write(&path, vec![7u8; 8]).unwrap();
        assert_eq!(read_capped(&path, 8).unwrap(), vec![7u8; 8]);
        assert!(matches!(
            read_capped(&path, 7).unwrap_err(),
            CappedReadError::TooLarge
        ));
        let empty = dir.path().join("empty.bin");
        std::fs::write(&empty, b"").unwrap();
        assert_eq!(read_capped(&empty, 4).unwrap(), b"");

        // A reader that never ends must not be copied forever. This is the
        // case a metadata check misses: the file grows while it is read.
        struct NeverEnds;
        impl std::io::Read for NeverEnds {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                buf.fill(b'x');
                Ok(buf.len())
            }
        }
        assert!(matches!(
            super::read_limited(&mut NeverEnds, 32).unwrap_err(),
            CappedReadError::TooLarge
        ));
    }

    #[cfg(unix)]
    #[test]
    fn special_files_are_rejected_without_blocking() {
        let err = read_capped(std::path::Path::new("/dev/zero"), 64).unwrap_err();
        assert!(matches!(err, CappedReadError::NotRegularFile), "{err}");
        let dir = tempfile::tempdir().unwrap();
        let err = read_capped(dir.path(), 64).unwrap_err();
        assert!(matches!(err, CappedReadError::NotRegularFile), "{err}");

        let pipe = dir.path().join("pipe");
        let status = std::process::Command::new("mkfifo")
            .arg(&pipe)
            .status()
            .unwrap();
        assert!(status.success());
        let err = read_capped(&pipe, 64).unwrap_err();
        assert!(matches!(err, CappedReadError::NotRegularFile), "{err}");

        let real = dir.path().join("real.bin");
        std::fs::write(&real, b"abc").unwrap();
        let link = dir.path().join("link.bin");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(read_capped(&link, 64).unwrap(), b"abc");
        let zero = dir.path().join("zero");
        std::os::unix::fs::symlink("/dev/zero", &zero).unwrap();
        let err = read_capped(&zero, 64).unwrap_err();
        assert!(matches!(err, CappedReadError::NotRegularFile), "{err}");
    }
}
