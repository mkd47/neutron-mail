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
    if out.chars().count() > MAX_NAME_CHARS {
        // Truncate the stem but keep the extension visible.
        let ext = extension(&out).map(|e| format!(".{e}")).unwrap_or_default();
        let keep = MAX_NAME_CHARS.saturating_sub(ext.chars().count());
        out = out.chars().take(keep).collect::<String>() + &ext;
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
];

/// Whether opening this attachment could execute code. Checks the file
/// extension, the declared MIME type and the content's magic bytes (ELF,
/// PE/MZ, shebang), so a renamed executable is still caught.
pub fn is_risky(name: &str, mime: &str, data: &[u8]) -> bool {
    let ext_risky =
        extension(&safe_filename(name)).is_some_and(|e| RISKY_EXTENSIONS.contains(&e.as_str()));
    let mime_risky = RISKY_MIME.contains(&mime.to_ascii_lowercase().as_str());
    let magic_risky =
        data.starts_with(b"\x7fELF") || data.starts_with(b"MZ") || data.starts_with(b"#!");
    ext_risky || mime_risky || magic_risky
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
    fn risk() {
        assert!(is_risky("setup.sh", "text/plain", b"echo"));
        assert!(is_risky("invoice.pdf.exe", "application/pdf", b"%PDF"));
        assert!(is_risky("invoice.pdf", "application/pdf", b"MZ\x90\0"));
        assert!(is_risky("photo.jpg", "image/jpeg", b"\x7fELF..."));
        assert!(is_risky("x", "application/x-desktop", b""));
        assert!(!is_risky("invoice.pdf", "application/pdf", b"%PDF-1.4"));
        assert!(!is_risky("notes.txt", "text/plain", b"hello"));
    }
}
