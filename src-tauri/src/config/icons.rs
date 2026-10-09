//! User icons stay opaque image data: never expose their SVG markup to the web DOM.
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{
    fs::File,
    io::Read,
    path::{Component, Path},
};

pub(super) const MAX_ICON_BYTES: u64 = 256 * 1024;

pub(super) fn load(dir: &Path, relative: &str) -> Result<String, String> {
    let path = Path::new(relative);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err("icon path must be relative, without '..' or absolute escapes".into());
    }
    let root = dir.canonicalize().map_err(|e| e.to_string())?;
    let target = dir.join(path).canonicalize().map_err(|e| e.to_string())?;
    if !target.starts_with(&root) {
        return Err("icon symlink points outside the config directory".into());
    }
    // Refuse a last-component symlink swapped in after canonicalization, and never
    // block on a FIFO/device masquerading as an icon.
    use std::os::unix::fs::OpenOptionsExt;
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(target)
        .map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("icon is not a regular file".into());
    }
    if metadata.len() > MAX_ICON_BYTES {
        return Err("icon exceeds 256 KB".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_ICON_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_ICON_BYTES {
        return Err("icon exceeds 256 KB".into());
    }
    let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        // Decode scanlines, not a full RGBA allocation. A signature alone accepts
        // truncated/invalid files that would otherwise silently render as broken images.
        let mut reader = png::Decoder::new(std::io::Cursor::new(&bytes))
            .read_info()
            .map_err(|e| format!("invalid PNG: {e}"))?;
        while reader
            .next_row()
            .map_err(|e| format!("invalid PNG: {e}"))?
            .is_some()
        {}
        reader.finish().map_err(|e| format!("invalid PNG: {e}"))?;
        "image/png"
    } else {
        let text = std::str::from_utf8(&bytes).map_err(|_| "expected SVG or PNG content")?;
        // roxmltree bounds entity depth and fan-out per reference, not total expansion, so a
        // 256 KB file could still expand without limit. Exported DOCTYPEs carry no entities.
        if text.contains("<!ENTITY") {
            return Err("SVG entity declarations are not supported".into());
        }
        let svg = roxmltree::Document::parse_with_options(
            text,
            roxmltree::ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            },
        )
        .map_err(|e| format!("invalid SVG: {e}"))?;
        let root = svg.root_element();
        if root.tag_name().name() != "svg"
            || root.tag_name().namespace() != Some("http://www.w3.org/2000/svg")
        {
            return Err("expected SVG or PNG content".into());
        }
        "image/svg+xml"
    };
    Ok(format!("data:{mime};base64,{}", STANDARD.encode(bytes)))
}

#[cfg(test)]
mod tests;
