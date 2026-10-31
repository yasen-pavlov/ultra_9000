use super::*;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::path::Path;

fn route(path: &Path) -> FileTarget {
    validate(
        tauri::Url::from_file_path(path).unwrap().as_str(),
        path.parent().unwrap().to_str().unwrap(),
    )
    .unwrap()
}

fn utf16(units: impl IntoIterator<Item = u16>, little_endian: bool) -> Vec<u8> {
    let mut bytes = if little_endian {
        vec![0xff, 0xfe]
    } else {
        vec![0xfe, 0xff]
    };
    for unit in units {
        bytes.extend_from_slice(&if little_endian {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    bytes
}

#[test]
fn text_code_config_and_resources_route_by_content_not_extension() {
    let dir = tempfile::tempdir().unwrap();
    for (name, content) in [
        ("main.rs", "fn main() { println!(\"雪\"); }\n"),
        ("ultra.toml", "[launch]\ndefault_tool = \"shell\"\n"),
        ("README.md", "# Read me\n\nUnicode: café 雪 🦀\n"),
        ("Dockerfile", "FROM scratch\nCOPY app /app\n"),
        ("Makefile", "all:\n\tprintf 'done\\n'\n"),
        ("script", "#!/bin/sh\nprintf 'hello\\n'\n"),
        ("archive.zip", "This is text, not an archive.\n"),
        ("image.png", "This is text, not an image.\n"),
        ("resource.xml", "<?xml version=\"1.0\"?><theme></theme>\n"),
        ("theme.json", "{\"background\":\"#181818\"}\n"),
        ("empty", ""),
        ("bom.txt", "\u{feff}Unicode text\n"),
        ("whitespace", "\t\r\n\u{c}"),
        (
            "server.pem",
            "-----BEGIN CERTIFICATE-----\nY2VydA==\n-----END CERTIFICATE-----\n",
        ),
        (
            "id_ed25519",
            "-----BEGIN OPENSSH PRIVATE KEY-----\na2V5\n-----END OPENSSH PRIVATE KEY-----\n",
        ),
        ("notes.txt", "MAC addresses: check router\n"),
        ("cars.csv", "BMW,price\n3,100\n"),
        ("metadata.txt", "ID3 tags are described here\n"),
        ("labels.txt", "MZ labels are ordinary text\n"),
        ("macros.txt", "BC macro documentation\n"),
        ("build.log", "\u{1b}[31merror\u{1b}[0m\n"),
        ("manual.txt", "b\u{8}bo\u{8}ol\u{8}ld\u{8}d\n"),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, content).unwrap();
        assert_eq!(
            route(&path),
            FileTarget::Neovim(path.to_str().unwrap().to_owned()),
            "{name}"
        );
    }
}

#[test]
fn svg_names_are_images_but_svg_looking_source_remains_text() {
    let dir = tempfile::tempdir().unwrap();
    for little_endian in [true, false] {
        let path = dir.path().join(format!("image-{little_endian}.SVG"));
        fs::write(&path, utf16("<svg/>".encode_utf16(), little_endian)).unwrap();
        assert_eq!(
            route(&path),
            FileTarget::System(path.to_str().unwrap().to_owned())
        );
        let source = dir.path().join(format!("source-{little_endian}.txt"));
        fs::write(&source, utf16("<svg/>".encode_utf16(), little_endian)).unwrap();
        assert_eq!(
            route(&source),
            FileTarget::Neovim(source.to_str().unwrap().to_owned())
        );
    }
    for (name, content) in [
        ("image.svg", "<svg/>"),
        ("ordinary.svg", "not XML, still an image by name"),
        ("Icon.svelte", "<svg {...$$props} viewBox=\"0 0 24 24\"/>"),
        ("image.svg.j2", "<svg>{{ path }}</svg>"),
        ("image.txt", "<?xml version=\"1.0\"?><svg/>"),
        ("partial.html", "<svg><path/></svg>"),
        ("resource.xml", "<?xml version=\"1.0\"?><theme/>"),
        ("code.rs", "<T as Trait>::method();\n"),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, content).unwrap();
        let path_string = path.to_str().unwrap().to_owned();
        assert_eq!(
            route(&path),
            if name.ends_with(".svg") {
                FileTarget::System(path_string)
            } else {
                FileTarget::Neovim(path_string)
            },
            "{name}"
        );
    }
}

#[test]
fn literal_filename_characters_survive_both_routes_without_command_interpretation() {
    let dir = tempfile::tempdir().unwrap();
    for (name, content, text) in [
        (
            "+command --flag;$(touch injected)'\"%#?&=+{cwd}\\雪\n\r\t",
            b"text".as_slice(),
            true,
        ),
        ("literal%2Fand%00.txt", b"\0\xff".as_slice(), false),
        ("trailing space ", b"text".as_slice(), true),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, content).unwrap();
        let path_string = path.to_str().unwrap().to_owned();
        assert_eq!(
            route(&path),
            if text {
                FileTarget::Neovim(path_string)
            } else {
                FileTarget::System(path_string)
            }
        );
    }
    assert!(!dir.path().join("injected").exists());
}

#[test]
fn actual_zip_archives_and_printable_pdf_override_misleading_text_names() {
    let zip = stored_zip_fixture();
    let empty_zip = b"PK\x05\x06\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";
    // A complete, entirely printable PDF with an empty page tree.
    let mut pdf = String::from("%PDF-1.4\n");
    let catalog = pdf.len();
    pdf.push_str("1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let pages = pdf.len();
    pdf.push_str("2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
    let xref = pdf.len();
    pdf.push_str(&format!(
        "xref\n0 3\n0000000000 65535 f \n{catalog:010} 00000 n \n{pages:010} 00000 n \ntrailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
    ));
    let dir = tempfile::tempdir().unwrap();
    for (name, content) in [
        ("archive.rs", zip.as_slice()),
        ("empty-archive.txt", empty_zip.as_slice()),
        ("document.md", pdf.as_bytes()),
        ("postscript.txt", b"%!PS-Adobe-3.0\nshowpage\n".as_slice()),
        ("richtext.txt", b"{\\rtf1\\ansi text}".as_slice()),
        ("image.txt", b"\x89PNG\r\n\x1a\n".as_slice()),
        ("bitmap.txt", b"BM\x46\0\0\0\0\0\0\0\x36\0\0\0".as_slice()),
        ("audio.txt", b"ID3\x04\0\0\0\0\0\x10".as_slice()),
        ("ape.txt", b"MAC \x96\x0f\0\0".as_slice()),
        ("truncated-elf.txt", b"\x7fELF\x02\x01\x01\0".as_slice()),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, content).unwrap();
        assert_eq!(
            route(&path),
            FileTarget::System(path.to_str().unwrap().to_owned()),
            "{name}"
        );
    }
}

#[test]
fn controls_invalid_and_unknown_encodings_route_to_system() {
    let dir = tempfile::tempdir().unwrap();
    for (index, content) in [
        b"\0".as_slice(),
        b"hello\x01world",
        b"hello\x0bworld",
        b"hello\x7fworld",
        "hello\u{85}world".as_bytes(),
        b"caf\xe9",
        b"\xef\xbb",
        b"\xf0\x9f\xa6",
        b"\xff\xfe\0\0a\0\0\0", // UTF-32 is not guessed as UTF-16.
        b"\0\0\xfe\xff\0\0\0a",
        b"h\0e\0l\0l\0o\0", // UTF-16 without a BOM is ambiguous.
    ]
    .into_iter()
    .enumerate()
    {
        let path = dir.path().join(format!("invalid-{index}.txt"));
        fs::write(&path, content).unwrap();
        assert_eq!(
            route(&path),
            FileTarget::System(path.to_str().unwrap().to_owned())
        );
    }
}

#[test]
fn utf8_scalars_crossing_the_sample_boundary_remain_text_but_malformed_tails_do_not() {
    let dir = tempfile::tempdir().unwrap();
    for scalar in ["é", "雪", "🦀"] {
        for included in 1..scalar.len() {
            let mut content = vec![b'a'; PREFIX_LEN - included];
            content.extend_from_slice(scalar.as_bytes());
            content.extend_from_slice(b"\nmore text");
            let path = dir
                .path()
                .join(format!("unicode-{}-{included}", scalar.len()));
            fs::write(&path, &content).unwrap();
            assert_eq!(
                route(&path),
                FileTarget::Neovim(path.to_str().unwrap().to_owned())
            );
            content.truncate(PREFIX_LEN);
            fs::write(&path, &content).unwrap();
            assert_eq!(
                route(&path),
                FileTarget::System(path.to_str().unwrap().to_owned())
            );
            content.extend_from_slice(b"not a continuation");
            fs::write(&path, content).unwrap();
            assert_eq!(
                route(&path),
                FileTarget::System(path.to_str().unwrap().to_owned())
            );
        }
    }
}

#[test]
fn bom_utf16_requires_valid_scalars_and_text_controls_in_both_byte_orders() {
    let dir = tempfile::tempdir().unwrap();
    for little_endian in [true, false] {
        for (index, units, text) in [
            (0, vec![], true),
            (
                1,
                "café 雪 🦀\r\n\t\u{c}\u{1b}[31m_\u{8}x"
                    .encode_utf16()
                    .collect(),
                true,
            ),
            (2, vec![0xd800], false),
            (3, vec![0xdc00], false),
            (4, vec![0xd800, 0x61], false),
            (5, vec![0x61, 0, 0x62], false),
            (6, vec![0x1b, 0x8], true),
            (7, vec![0x85], false),
            // The high surrogate occupies the last sampled code unit.
            (
                8,
                [vec![0x61; (PREFIX_LEN - 2) / 2 - 1], vec![0xd83e, 0xdd80]].concat(),
                true,
            ),
        ] {
            let content = utf16(units, little_endian);
            let path = dir
                .path()
                .join(format!("utf16-{little_endian}-{index}.txt"));
            fs::write(&path, &content).unwrap();
            let path_string = path.to_str().unwrap().to_owned();
            assert_eq!(
                route(&path),
                if text {
                    FileTarget::Neovim(path_string)
                } else {
                    FileTarget::System(path_string)
                }
            );
        }
        let odd_path = dir.path().join(format!("odd-utf16-{little_endian}"));
        let odd = if little_endian {
            [0xff, 0xfe, 0x61]
        } else {
            [0xfe, 0xff, 0]
        };
        fs::write(&odd_path, odd).unwrap();
        assert_eq!(
            route(&odd_path),
            FileTarget::System(odd_path.to_str().unwrap().to_owned())
        );
    }
}

#[test]
fn inspection_fills_short_reads_retries_interrupts_and_stays_bounded() {
    struct ShortReads<'a> {
        bytes: &'a [u8],
        interrupted: bool,
        read: usize,
    }
    impl Read for ShortReads<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(io::ErrorKind::Interrupted.into());
            }
            let count = buffer.len().min(self.bytes.len()).min(3);
            buffer[..count].copy_from_slice(&self.bytes[..count]);
            self.bytes = &self.bytes[count..];
            self.read += count;
            Ok(count)
        }
    }
    let mut short = ShortReads {
        bytes: b"abcde\0binary",
        interrupted: false,
        read: 0,
    };
    assert_eq!(inspect_content(&mut short).unwrap(), Content::Data);
    let content = vec![b'a'; PREFIX_LEN * 2];
    let mut short = ShortReads {
        bytes: &content,
        interrupted: false,
        read: 0,
    };
    assert_eq!(inspect_content(&mut short).unwrap(), Content::Text);
    assert_eq!(short.read, PREFIX_LEN + 4);
}

#[test]
fn inspection_read_failure_never_claims_text() {
    struct FailsAfterText(bool);
    impl Read for FailsAfterText {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.0 {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            self.0 = true;
            buffer[..4].copy_from_slice(b"text");
            Ok(4)
        }
    }
    assert!(inspect_content(&mut FailsAfterText(false)).is_err());
}

#[test]
fn localhost_and_encoded_separators_decode_once_without_losing_reserved_characters() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("nested");
    fs::create_dir(&nested).unwrap();
    let path = nested.join("file #?%2F.txt");
    fs::write(&path, "file").unwrap();
    let uri = tauri::Url::from_file_path(&path).unwrap().to_string();
    let expected = FileTarget::Neovim(path.to_str().unwrap().to_owned());
    for uri in [
        uri.clone(),
        uri.replacen("file:///", "file://localhost/", 1),
        uri.replacen("file:///", "file://LOCALHOST/", 1),
        uri.replace("/nested/", "%2Fnested%2F"),
        format!("{uri}?line=12&column=3#L12"),
        format!("{uri}#L12;execute-dangerous-command"),
    ] {
        assert_eq!(
            validate(&uri, dir.path().to_str().unwrap()).unwrap(),
            expected
        );
    }
}

#[test]
fn this_machines_hostname_is_local_but_other_hosts_remain_remote() {
    let mut hostname = [0u8; 256];
    assert_eq!(
        unsafe { libc::gethostname(hostname.as_mut_ptr().cast(), hostname.len()) },
        0
    );
    let end = hostname.iter().position(|byte| *byte == 0).unwrap();
    let host = std::str::from_utf8(&hostname[..end]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("local file");
    fs::write(&path, "local").unwrap();
    let mut uri = tauri::Url::from_file_path(&path).unwrap();
    for host in [host.to_owned(), host.to_ascii_uppercase()] {
        uri.set_host(Some(&host)).unwrap();
        assert_eq!(
            validate(uri.as_str(), dir.path().to_str().unwrap()).unwrap(),
            FileTarget::Neovim(path.to_str().unwrap().to_owned())
        );
    }
    uri.set_host(Some(&format!("not-this-machine.{host}")))
        .unwrap();
    assert!(validate(uri.as_str(), dir.path().to_str().unwrap()).is_err());
}

#[test]
fn malformed_remote_and_non_file_uris_are_rejected() {
    for uri in [
        "not a URI",
        "https://localhost/etc/passwd",
        "file:relative",
        "file://remote.example/etc/passwd",
        "file://localhost.example/etc/passwd",
        "file://user@localhost/etc/passwd",
        "file://user:password@localhost/etc/passwd",
        "file://localhost:123/etc/passwd",
        "file:///etc/%",
        "file:///etc/%0",
        "file:///etc/%GG",
        "file:///etc/%00passwd",
        "file:///etc/passwd?line=%GG",
        "file:///etc/passwd#%00",
        "file:///etc/pa\0sswd",
        "file:///etc/pa\nsswd",
        "file:///etc/passwd ",
        "file:///etc\\passwd",
        " file:///etc/passwd",
    ] {
        assert!(validate(uri, "/").is_err(), "{uri:?}");
    }
}

#[test]
fn missing_directories_special_files_and_invalid_source_cwds_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing");
    let fifo = dir.path().join("fifo");
    let fifo_name = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
    let socket = dir.path().join("socket");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    for path in [
        dir.path(),
        missing.as_path(),
        fifo.as_path(),
        socket.as_path(),
        std::path::Path::new("/dev/null"),
    ] {
        let uri = tauri::Url::from_file_path(path).unwrap().to_string();
        assert!(validate(&uri, "/").is_err(), "{uri}");
    }
    let file = dir.path().join("file");
    fs::write(&file, "file").unwrap();
    let uri = tauri::Url::from_file_path(&file).unwrap().to_string();
    for cwd in [
        missing.to_str().unwrap(),
        file.to_str().unwrap(),
        "relative",
    ] {
        assert!(validate(&uri, cwd).is_err(), "{cwd}");
    }
}

#[test]
fn symlinked_files_keep_the_clicked_path_and_non_utf8_never_becomes_lossy_argv() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("file");
    fs::write(&file, "file").unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    let uri = tauri::Url::from_file_path(&link).unwrap().to_string();
    assert_eq!(
        validate(&uri, "/").unwrap(),
        FileTarget::Neovim(link.to_str().unwrap().to_owned())
    );

    let non_utf8 = dir.path().join(std::ffi::OsString::from_vec(vec![0xff]));
    // Rejection precedes metadata: APFS cannot create this name, and no file is needed.
    let uri = tauri::Url::from_file_path(&non_utf8).unwrap().to_string();
    assert!(validate(&uri, "/").unwrap_err().contains("UTF-8"));
}

#[test]
fn neutral_symlinks_to_launchers_and_nontext_scripts_reveal_the_clicked_path() {
    let dir = tempfile::tempdir().unwrap();
    for extension in [
        "terminal", "fileloc", "desktop", "jar", "py", "pyw", "pyc", "pyo",
    ] {
        let target = dir.path().join(format!("target.{extension}"));
        fs::write(&target, b"\0\xffnot text").unwrap();
        assert_eq!(
            route(&target),
            FileTarget::Reveal(target.to_str().unwrap().to_owned())
        );
        let link = dir.path().join(format!("notes-{extension}"));
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(
            route(&link),
            FileTarget::Reveal(link.to_str().unwrap().to_owned()),
            "{extension}"
        );
    }
}

#[test]
fn resolved_extensions_control_svg_and_executable_document_routes() {
    let dir = tempfile::tempdir().unwrap();
    for (name, target_name, content, text) in [
        ("icon.svg", "run", "#!/bin/sh\nexit 0\n", true),
        ("report.pdf", "payload", "%PDF-1.4\n:; exit 0\n", false),
    ] {
        let target = dir.path().join(target_name);
        fs::write(&target, content).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
        let link = dir.path().join(name);
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let path = link.to_str().unwrap().to_owned();
        assert_eq!(
            route(&link),
            if text {
                FileTarget::Neovim(path)
            } else {
                FileTarget::Reveal(path)
            },
            "{name}"
        );
    }
}

#[test]
fn unreadable_regular_file_is_uncertain_content_not_a_validation_error_or_editor_route() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("private.txt");
    fs::write(&path, "otherwise ordinary text").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&path).is_ok() {
        // Root and other privileged runners may bypass these permissions.
        return;
    }
    assert_eq!(
        route(&path),
        FileTarget::System(path.to_str().unwrap().to_owned())
    );
}

fn stored_zip_fixture() -> Vec<u8> {
    // Stored member a.txt = "hello", its central directory, and the end record.
    [
        b"PK\x03\x04\x14\0\0\0\0\0\0\0\0\0\x86\xa6\x10\x36\x05\0\0\0\x05\0\0\0\x05\0\0\0a.txthello".as_slice(),
        b"PK\x01\x02\x14\0\x14\0\0\0\0\0\0\0\0\0\x86\xa6\x10\x36\x05\0\0\0\x05\0\0\0\x05\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0a.txt".as_slice(),
        b"PK\x05\x06\0\0\0\0\x01\0\x01\0\x33\0\0\0\x28\0\0\0\0\0".as_slice(),
    ]
    .concat()
}

#[test]
fn known_native_and_bytecode_executables_reveal_even_without_executable_mode() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let mut elf = [0; 64];
    elf[..8].copy_from_slice(b"\x7fELF\x02\x01\x01\0");
    let mut dex = [0; 40];
    dex[..8].copy_from_slice(b"dex\n035\0");
    dex[36] = 0x70;
    for (name, content) in [
        ("native.txt", elf.as_slice()),
        (
            "windows.txt",
            b"MZ\x90\0\x03\0\0\0\x04\0\0\0\xff\xff\0\0".as_slice(),
        ),
        ("macos.txt", b"\xcf\xfa\xed\xfe".as_slice()),
        ("java.txt", b"\xca\xfe\xba\xbe\0\0\0\x3d".as_slice()),
        ("module.txt", b"\0asm\x01\0\0\0".as_slice()),
        ("dalvik.txt", dex.as_slice()),
        (
            "shortcut.txt",
            b"\x4c\0\0\0\x01\x14\x02\0\0\0\0\0\xc0\0\0\0\0\0\0\x46".as_slice(),
        ),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, content).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            route(&path),
            FileTarget::Reveal(path.to_str().unwrap().to_owned()),
            "{name}"
        );
    }
}

#[test]
fn launcher_archives_and_associations_reveal_instead_of_running_the_default_handler() {
    let dir = tempfile::tempdir().unwrap();
    let zip = stored_zip_fixture();
    for (name, content) in [
        ("application.jar", zip.as_slice()),
        ("application.JaR", zip.as_slice()),
        (
            "download.JNLP",
            b"<?xml version=\"1.0\"?><jnlp/>".as_slice(),
        ),
        (
            "launcher.DESKTOP",
            b"[Desktop Entry]\nType=Application\nExec=never-run\n".as_slice(),
        ),
        ("image.AppImage", b"\0unknown binary".as_slice()),
        ("shortcut.LnK", b"\x4c\0\0\0\x01\x14\x02\0".as_slice()),
        ("installer.APK", zip.as_slice()),
        ("installer.MSI", b"\0unknown binary".as_slice()),
        (
            "misleading.jar",
            b"Even printable launcher input must not reach java -jar.".as_slice(),
        ),
        (
            "bookmark.URL",
            b"[InternetShortcut]\nURL=untrusted:target\n".as_slice(),
        ),
        (
            "launcher.KDELNK",
            b"[Desktop Entry]\nExec=never-run\n".as_slice(),
        ),
        ("profile.TERMINAL", b"bplist00\0CommandString".as_slice()),
        (
            "location.FILELOC",
            b"<?xml version=\"1.0\"?><plist/>".as_slice(),
        ),
        (
            "location.INETLOC",
            b"<?xml version=\"1.0\"?><plist/>".as_slice(),
        ),
        (
            "location.WEBLOC",
            b"<?xml version=\"1.0\"?><plist/>".as_slice(),
        ),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, content).unwrap();
        assert_eq!(
            route(&path),
            FileTarget::Reveal(path.to_str().unwrap().to_owned()),
            "{name}"
        );
    }
    // A sampled manifest entry also protects a ZIP/JAR with a misleading filename.
    let path = dir.path().join("disguised.txt");
    fs::write(
        &path,
        [
            b"PK\x03\x04\x14\0\0\0\0\0\0\0\0\0\x6e\xc4\x27\x82\x17\0\0\0\x17\0\0\0\x14\0\0\0META-INF/MANIFEST.MFManifest-Version: 1.0\n\n".as_slice(),
            b"PK\x01\x02\x14\0\x14\0\0\0\0\0\0\0\0\0\x6e\xc4\x27\x82\x17\0\0\0\x17\0\0\0\x14\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0META-INF/MANIFEST.MF".as_slice(),
            b"PK\x05\x06\0\0\0\0\x01\0\x01\0\x42\0\0\0\x49\0\0\0\0\0".as_slice(),
        ]
        .concat(),
    )
    .unwrap();
    assert_eq!(
        route(&path),
        FileTarget::Reveal(path.to_str().unwrap().to_owned())
    );
}

#[test]
fn executable_mode_reveals_nontext_and_unreadable_data_but_keeps_text_scripts_in_neovim() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    for (name, content, mode, text) in [
        ("binary", b"\0\xffunknown".as_slice(), 0o700, false),
        ("unreadable", b"\0\xffunknown".as_slice(), 0o100, false),
        (
            "script",
            b"#!/bin/sh\nprintf 'not executed\\n'\n".as_slice(),
            0o700,
            true,
        ),
        (
            "windows-script.cmd",
            b"@echo off\r\necho not executed\r\n".as_slice(),
            0o700,
            true,
        ),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, content).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        let path_string = path.to_str().unwrap().to_owned();
        assert_eq!(
            route(&path),
            if text {
                FileTarget::Neovim(path_string)
            } else {
                FileTarget::Reveal(path_string)
            },
            "{name}"
        );
    }
}

#[test]
fn executable_documents_with_names_open_but_extensionless_or_unknown_payloads_reveal() {
    let dir = tempfile::tempdir().unwrap();
    for (name, content, system) in [
        ("report.pdf", b"%PDF-1.4\nprintable PDF".as_slice(), true),
        ("photo.png", b"\x89PNG\r\n\x1a\n".as_slice(), true),
        ("archive.zip", stored_zip_fixture().as_slice(), true),
        ("document", b"%PDF-1.4\nprintable PDF".as_slice(), false),
        ("report.", b"%PDF-1.4\n:; exit 0\n".as_slice(), false),
        ("unknown", b"\0\xffELF-less data".as_slice(), false),
        ("unknown.bin", b"\0\xffELF-less data".as_slice(), false),
        ("payload.pyc", b"%PDF-1.4\nprintable PDF".as_slice(), false),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, content).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let path_string = path.to_str().unwrap().to_owned();
        assert_eq!(
            route(&path),
            if system {
                FileTarget::System(path_string)
            } else {
                FileTarget::Reveal(path_string)
            },
            "{name}"
        );
    }
}

#[test]
fn readable_shebang_scripts_are_text_but_binary_interpreter_payloads_reveal() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    for (name, content, text) in [
        (
            "readable.txt",
            b"#!/bin/sh\nprintf 'text\\n'\n".as_slice(),
            true,
        ),
        ("nul.txt", b"#!/bin/sh\n\0binary".as_slice(), false),
        (
            "encoding.txt",
            b"#!/usr/bin/env python3\nprint('\xff')\n".as_slice(),
            false,
        ),
        ("control.txt", b"#! /bin/sh\n\x01binary".as_slice(), false),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, content).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let path_string = path.to_str().unwrap().to_owned();
        assert_eq!(
            route(&path),
            if text {
                FileTarget::Neovim(path_string)
            } else {
                FileTarget::Reveal(path_string)
            },
            "{name}"
        );
    }
}

#[test]
fn legacy_script_suffixes_reveal_only_nontext_or_uncertain_payloads() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    for extension in ["BAT", "cMd", "COMMAND", "tool"] {
        let path = dir.path().join(format!("script.{extension}"));
        for (content, text) in [
            (b"echo readable script\r\n".as_slice(), true),
            (b"echo legacy caf\xe9\r\n".as_slice(), false),
            (b"\0binary script payload".as_slice(), false),
        ] {
            fs::write(&path, content).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            let path_string = path.to_str().unwrap().to_owned();
            assert_eq!(
                route(&path),
                if text {
                    FileTarget::Neovim(path_string)
                } else {
                    FileTarget::Reveal(path_string)
                },
                "{extension}"
            );
        }
        // Always non-text, so the same expectation holds for privileged runners
        // that bypass permissions and ordinary runners taking the unreadable path.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(
            route(&path),
            FileTarget::Reveal(path.to_str().unwrap().to_owned())
        );
    }
}

#[test]
fn msi_root_clsid_distinguishes_installers_from_ordinary_compound_documents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("compound.txt");
    let msi_clsid = b"\x84\x10\x0c\0\0\0\0\0\xc0\0\0\0\0\0\0\x46";
    let word_clsid = b"\x06\x09\x02\0\0\0\0\0\xc0\0\0\0\0\0\0\x46";
    for (offset, clsid, installer) in [
        (592, &[0; 16], false),
        (592, word_clsid, false),
        (592, msi_clsid, true),
        (4096, msi_clsid, true),
        (PREFIX_LEN - 16, msi_clsid, true),
        (64, msi_clsid, false),
    ] {
        let mut content = [0; PREFIX_LEN];
        content[..8].copy_from_slice(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1");
        content[offset..offset + clsid.len()].copy_from_slice(clsid);
        fs::write(&path, content).unwrap();
        let path_string = path.to_str().unwrap().to_owned();
        assert_eq!(
            route(&path),
            if installer {
                FileTarget::Reveal(path_string)
            } else {
                FileTarget::System(path_string)
            },
            "root marker at {offset}, installer={installer}"
        );
    }
}
