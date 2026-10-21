use super::*;
use std::fs;
use std::os::unix::ffi::OsStringExt;

#[test]
fn every_extension_and_literal_filename_character_is_preserved_as_argv() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    for name in [
        "image.png",
        "archive.tar.gz",
        "executable",
        "unknown.extension",
        "+command --flag;$(touch injected)'\"%#?&=+{cwd}\\雪\n\r\t",
        "literal%2Fand%00.txt",
        "trailing space ",
    ] {
        let path = dir.path().join(name);
        fs::write(&path, [0, 255]).unwrap();
        let uri = tauri::Url::from_file_path(&path).unwrap().to_string();
        assert_eq!(
            validate(&uri, cwd).unwrap(),
            vec!["--".to_owned(), path.to_str().unwrap().to_owned()],
            "{uri}"
        );
    }
}

#[test]
fn localhost_and_encoded_separators_decode_once_without_losing_reserved_characters() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("nested");
    fs::create_dir(&nested).unwrap();
    let path = nested.join("file #?%2F.txt");
    fs::write(&path, "file").unwrap();
    let uri = tauri::Url::from_file_path(&path).unwrap().to_string();
    let expected = vec!["--".to_owned(), path.to_str().unwrap().to_owned()];
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
            vec!["--".to_owned(), path.to_str().unwrap().to_owned()]
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
    for path in [
        dir.path(),
        missing.as_path(),
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
    assert_eq!(validate(&uri, "/").unwrap()[1], link.to_str().unwrap());

    let non_utf8 = dir.path().join(std::ffi::OsString::from_vec(vec![0xff]));
    // Rejection precedes metadata: APFS cannot create this name, and no file is needed.
    let uri = tauri::Url::from_file_path(&non_utf8).unwrap().to_string();
    assert!(validate(&uri, "/").unwrap_err().contains("UTF-8"));
}
