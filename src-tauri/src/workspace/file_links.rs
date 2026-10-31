//! Clicked file links are validated path data, routed by bounded content inspection off-main.

use alacritty_embed::embed::SessionKey;
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

const PREFIX_LEN: usize = 8 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FileTarget {
    Neovim(String),
    System(String),
    Reveal(String),
}

#[derive(Debug, PartialEq, Eq)]
enum Content {
    Text,
    Data,
    Recognized,
    Launchable,
}

/// Captured before disk work; rechecked on main before launching, independently of selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FileOrigin {
    pub key: SessionKey,
    pub item: String,
    pub cwd: String,
    pub project: String,
}

/// Decodes a `file:` URI into a local path string. Decoding happens exactly once through Tauri's
/// existing URL library; query/fragment metadata is not a filename and never becomes an argument.
fn local_file_path(uri: &str) -> Result<String, String> {
    let invalid = || "This file link is not a valid local file URI.".to_owned();
    let bytes = uri.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        // The URL parser otherwise silently trims controls/spaces or normalizes backslashes.
        // Encoded spaces and backslashes remain valid literal Unix filename characters.
        if *byte <= b' ' || *byte == 0x7f || *byte == b'\\' {
            return Err(invalid());
        }
        if *byte == b'%' {
            let escaped = bytes.get(index + 1..index + 3).ok_or_else(invalid)?;
            if !escaped.iter().all(u8::is_ascii_hexdigit) || escaped == b"00" {
                return Err(invalid());
            }
        }
    }
    let mut url = tauri::Url::parse(uri).map_err(|_| invalid())?;
    if url.scheme() != "file"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.host_str().is_some_and(|host| !is_local_host(host))
        || !uri
            .split_once(':')
            .is_some_and(|(_, rest)| rest.starts_with('/'))
    {
        return Err(invalid());
    }
    // Unix conversion recognizes localhost, not this machine's actual hostname.
    // Keep an explicit localhost host: url 2.5.8 leaves stale host state on set_host(None).
    if url.host_str().is_some() {
        url.set_host(Some("localhost")).map_err(|_| invalid())?;
    }
    let path = url.to_file_path().map_err(|_| invalid())?;
    let path = path
        .into_os_string()
        .into_string()
        .map_err(|_| "This file link's filename cannot be represented as UTF-8.".to_owned())?;
    if path.contains('\0') {
        return Err(invalid());
    }
    Ok(path)
}

/// Runs off-main: decodes the link, then classifies the file by bounded content inspection.
pub(super) fn validate(uri: &str, cwd: &str) -> Result<FileTarget, String> {
    let path = local_file_path(uri)?;
    let metadata =
        std::fs::metadata(&path).map_err(|error| format!("Cannot open {path}: {error}."))?;
    if !metadata.is_file() {
        return Err(format!("Cannot open {path}: not a regular file."));
    }
    crate::launch::validate_directory(cwd)?;
    let initial_executable = metadata.permissions().mode() & 0o111 != 0;
    let canonical = std::fs::canonicalize(&path).ok();
    let launcher = launcher_path(std::path::Path::new(&path))
        || canonical.as_deref().is_some_and(launcher_path);
    let script =
        script_path(std::path::Path::new(&path)) || canonical.as_deref().is_some_and(script_path);
    let extension = canonical
        .as_deref()
        .unwrap_or(std::path::Path::new(&path))
        .extension()
        .filter(|extension| !extension.is_empty());
    let uncertain_target = |path| {
        if initial_executable || launcher || script {
            FileTarget::Reveal(path)
        } else {
            FileTarget::System(path)
        }
    };
    // Nonblocking open plus descriptor metadata avoids hanging on a replacement FIFO or
    // classifying a special file that replaced the initially validated regular file.
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(&path)
    {
        Ok(file) => file,
        Err(_) => {
            if std::fs::metadata(&path).is_ok_and(|metadata| !metadata.is_file()) {
                return Err(format!("Cannot open {path}: not a regular file."));
            }
            return Ok(uncertain_target(path));
        }
    };
    let executable = match file.metadata() {
        Ok(metadata) if !metadata.is_file() => {
            return Err(format!("Cannot open {path}: not a regular file."));
        }
        Err(_) => return Ok(uncertain_target(path)),
        Ok(metadata) => initial_executable || metadata.permissions().mode() & 0o111 != 0,
    };
    // Text scripts remain editor data even when executable. Recognized launchers
    // must never reach a default handler that could execute them on a simple click.
    Ok(match inspect_content(&mut file).unwrap_or(Content::Data) {
        Content::Text if !launcher => {
            if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("svg")) {
                FileTarget::System(path)
            } else {
                FileTarget::Neovim(path)
            }
        }
        Content::Launchable => FileTarget::Reveal(path),
        Content::Recognized if !launcher && !script && extension.is_some() => {
            FileTarget::System(path)
        }
        _ if executable || launcher || script => FileTarget::Reveal(path),
        _ => FileTarget::System(path),
    })
}

fn inspect_content(reader: &mut impl Read) -> io::Result<Content> {
    // Four lookahead bytes complete a Unicode scalar crossing the prefix boundary.
    // Fill short reads; a short successful read alone does not mean end-of-file.
    let mut bytes = [0; PREFIX_LEN + 4];
    let mut len = 0;
    while len < bytes.len() {
        match reader.read(&mut bytes[len..]) {
            Ok(0) => break,
            Ok(count) => len += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    let bytes = &bytes[..len];
    let sample = &bytes[..len.min(PREFIX_LEN)];
    // Standard Shell Link header, absent from infer: renamed shortcuts still launch
    // their target through OS associations, even without executable permission.
    if sample.starts_with(b"\x4c\0\0\0\x01\x14\x02\0") {
        return Ok(Content::Launchable);
    }
    // infer's "msi" matcher is only the generic Compound File signature, also
    // shared by Office documents. Require the standard MSI root CLSID inside
    // the sampled directory area (the desktop MIME rule starts at byte 592).
    let msi_clsid = b"\x84\x10\x0c\0\0\0\0\0\xc0\0\0\0\0\0\0\x46";
    if sample.starts_with(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1")
        && sample.get(592..).is_some_and(|directory| {
            directory
                .windows(msi_clsid.len())
                .any(|value| value == msi_clsid)
        })
    {
        return Ok(Content::Launchable);
    }
    let kind = infer::get(sample);
    // Weak printable signatures (BM, MAC, ID3, MZ, PEM, ...) must not override
    // actual source/text. Printable document formats are deliberate exceptions.
    if text_content(bytes)
        && !kind.is_some_and(|kind| matches!(kind.extension(), "pdf" | "ps" | "rtf"))
    {
        return Ok(Content::Text);
    }
    // Malformed/binary interpreter payloads remain code even if infer recognizes a script.
    if sample.starts_with(b"#!") {
        return Ok(Content::Launchable);
    }
    if let Some(kind) = kind {
        // App also includes certificates and disk images, not just executable code.
        if kind.matcher_type() == infer::MatcherType::App
            && matches!(
                kind.extension(),
                "wasm" | "elf" | "exe" | "dll" | "class" | "bc" | "mach" | "dex" | "dey" | "obj"
            )
        {
            return Ok(Content::Launchable);
        }
        if kind.extension() == "zip"
            && sample
                .windows(b"META-INF/MANIFEST.MF".len())
                .any(|window| window.eq_ignore_ascii_case(b"META-INF/MANIFEST.MF"))
        {
            return Ok(Content::Launchable);
        }
        if kind.matcher_type() != infer::MatcherType::App {
            return Ok(Content::Recognized);
        }
    }
    Ok(Content::Data)
}

fn launcher_path(path: &std::path::Path) -> bool {
    // Safety-specific OS launcher associations, not an editor extension allowlist.
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| {
            [
                "jar", "jnlp", "desktop", "appimage", "apk", "msi", "lnk", "url", "kdelnk",
                "terminal", "fileloc", "inetloc", "webloc",
            ]
            .iter()
            .any(|launcher| extension.eq_ignore_ascii_case(launcher))
        })
}

fn script_path(path: &std::path::Path) -> bool {
    // Only unreadable/non-text script payloads reveal; readable scripts stay editable.
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| {
            ["bat", "cmd", "command", "tool", "py", "pyw", "pyc", "pyo"]
                .iter()
                .any(|script| extension.eq_ignore_ascii_case(script))
        })
}

fn text_content(bytes: &[u8]) -> bool {
    let mut end = bytes.len().min(PREFIX_LEN);
    let sample = &bytes[..end];
    if sample.starts_with(&[0xff, 0xfe]) || sample.starts_with(&[0xfe, 0xff]) {
        let little_endian = sample[0] == 0xff;
        let unit = |pair: &[u8; 2]| {
            if little_endian {
                u16::from_le_bytes(*pair)
            } else {
                u16::from_be_bytes(*pair)
            }
        };
        if !end.is_multiple_of(2) {
            return false;
        }
        if end > 2 && (0xd800..=0xdbff).contains(&unit(&[bytes[end - 2], bytes[end - 1]])) {
            end += 2;
        }
        let Some(sample) = bytes.get(2..end) else {
            return false;
        };
        return char::decode_utf16(sample.as_chunks::<2>().0.iter().map(unit))
            .all(|character| character.is_ok_and(text_character));
    }
    let error = match std::str::from_utf8(sample) {
        Ok(text) => return editable_text(text),
        Err(error) => error,
    };
    if error.error_len().is_some() {
        return false;
    }
    let start = error.valid_up_to();
    let width = match bytes[start] {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return false,
    };
    end = start + width;
    bytes
        .get(..end)
        .and_then(|sample| std::str::from_utf8(sample).ok())
        .is_some_and(editable_text)
}

fn editable_text(text: &str) -> bool {
    text.chars().all(text_character)
}

fn text_character(character: char) -> bool {
    !character.is_control()
        || matches!(character, '\t' | '\r' | '\n' | '\u{c}' | '\u{1b}' | '\u{8}')
}

fn is_local_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let mut hostname = [0u8; 256];
    // gethostname writes into this initialized, bounded buffer; reject truncation/failure.
    if unsafe { libc::gethostname(hostname.as_mut_ptr().cast(), hostname.len()) } != 0 {
        return false;
    }
    hostname
        .iter()
        .position(|byte| *byte == 0)
        .is_some_and(|end| host.as_bytes().eq_ignore_ascii_case(&hostname[..end]))
}

#[cfg(test)]
mod tests;
