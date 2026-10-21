//! Clicked file links are argv data, never shell source or a system-opener fallback.

use alacritty_embed::embed::SessionKey;

/// Captured before disk work; rechecked on main before launching, independently of selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FileOrigin {
    pub key: SessionKey,
    pub item: String,
    pub cwd: String,
    pub project: String,
}

/// Runs off-main. URL decoding happens exactly once through Tauri's existing URL library.
/// Query/fragment metadata is not a filename and does not become Neovim commands.
pub(super) fn validate(uri: &str, cwd: &str) -> Result<Vec<String>, String> {
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
    let metadata =
        std::fs::metadata(&path).map_err(|error| format!("Cannot open {path}: {error}."))?;
    if !metadata.is_file() {
        return Err(format!("Cannot open {path}: not a regular file."));
    }
    crate::launch::validate_directory(cwd)?;
    // Absolute paths plus -- also protect names containing +commands, dashes or shell syntax.
    Ok(vec!["--".into(), path])
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
