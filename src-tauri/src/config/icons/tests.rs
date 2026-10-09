use super::*;
use std::{fs, os::unix::fs::symlink};
const SVG: &str =
    "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 10 10'><path d='M0 0h10v10z'/></svg>";

#[test]
fn content_not_extension_and_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mark.bin");
    fs::write(&path, SVG).unwrap();
    assert!(load(dir.path(), "mark.bin")
        .unwrap()
        .starts_with("data:image/svg+xml;base64,"));
    let padded = format!("{SVG}{}", " ".repeat(MAX_ICON_BYTES as usize - SVG.len()));
    fs::write(&path, padded).unwrap();
    assert!(load(dir.path(), "mark.bin").is_ok());
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(MAX_ICON_BYTES + 1)
        .unwrap();
    assert!(load(dir.path(), "mark.bin").unwrap_err().contains("256 KB"));
}

#[test]
fn accepts_exported_svg_with_doctype() {
    let dir = tempfile::tempdir().unwrap();
    let svg = format!("<?xml version='1.0'?>\n<!DOCTYPE svg PUBLIC '-//W3C//DTD SVG 1.1//EN' 'http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd'>\n{SVG}");
    fs::write(dir.path().join("export.svg"), &svg).unwrap();
    assert_eq!(
        load(dir.path(), "export.svg").unwrap(),
        format!(
            "data:image/svg+xml;base64,{}",
            STANDARD.encode(svg.as_bytes())
        )
    );
}

#[test]
fn rejects_entity_expansion() {
    let dir = tempfile::tempdir().unwrap();
    let svg = format!(
        "<!DOCTYPE svg [<!ENTITY a \"{}\"><!ENTITY b \"{}\">]><svg xmlns='http://www.w3.org/2000/svg'>{}</svg>",
        "A".repeat(1000),
        "&a;".repeat(255),
        "&b;".repeat(1000),
    );
    fs::write(dir.path().join("bomb.svg"), svg).unwrap();
    assert!(load(dir.path(), "bomb.svg").unwrap_err().contains("entity"));
}

#[test]
fn rejects_escape_missing_and_invalid_images() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("mark.svg"), SVG).unwrap();
    symlink(
        outside.path().join("mark.svg"),
        dir.path().join("escape.svg"),
    )
    .unwrap();
    for path in ["../mark.svg", "/mark.svg", "escape.svg", "missing.svg"] {
        assert!(load(dir.path(), path).is_err(), "{path}");
    }
    for bytes in [
        b"<svg".as_slice(),
        b"<html/>".as_slice(),
        b"\x89PNG\r\n\x1a\n".as_slice(),
    ] {
        fs::write(dir.path().join("invalid.png"), bytes).unwrap();
        assert!(load(dir.path(), "invalid.png").is_err());
    }
    fs::write(dir.path().join("real.svg"), SVG).unwrap();
    symlink("real.svg", dir.path().join("inside.svg")).unwrap();
    assert!(load(dir.path(), "inside.svg").is_ok());
}

#[test]
fn validates_png_pixels_and_rejects_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mark.svg");
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[255, 255, 255, 255])
            .unwrap();
    }
    fs::write(&path, &bytes).unwrap();
    assert!(load(dir.path(), "mark.svg")
        .unwrap()
        .starts_with("data:image/png;base64,"));
    fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
    assert!(load(dir.path(), "mark.svg").is_err());
}
