use super::*;
use std::{
    cell::Cell,
    io::{BufRead, BufReader, Read},
    os::unix::{
        ffi::OsStrExt,
        fs::{symlink, PermissionsExt},
        net::UnixListener,
    },
    process::{Child, Command, Stdio},
};

#[test]
fn launcher_keeps_valid_activation_boundaries_and_drops_invalid_values_without_failing() {
    for (value, accepted) in [
        ("".to_owned(), true),
        ("a".repeat(4096), true),
        ("é".repeat(2048), true),
        ("a".repeat(4097), false),
        ("é".repeat(2049), false),
        ("a\0b".to_owned(), false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("runtime");
        let expected = accepted.then_some(value.clone());
        let owner = control::Server::start(
            &runtime,
            move |command, responder| {
                assert_eq!(
                    command,
                    control::Command::SetWindowVisible {
                        visible: control::Visibility::Set(true),
                        activation_token: expected.clone(),
                        desktop_startup_id: expected.clone(),
                    }
                );
                responder.send(Ok(control::Done::new()));
            },
            || {},
        )
        .unwrap();
        let stream =
            UnixStream::connect(runtime.join(format!("control-{}.sock", std::process::id())))
                .unwrap();
        assert_eq!(
            handover_with_activation(stream, STARTUP_TIMEOUT, Some(&value), Some(&value)),
            Ok(()),
            "invalid activation values must not prevent handover"
        );
        owner.close();
    }
}

#[test]
fn build_profiles_have_separate_default_keys_and_only_release_scans_legacy() {
    let release = Key::select(false, None).unwrap();
    assert_eq!(release.filename, "instance.lock");
    assert!(release.legacy_fallback);
    let debug = Key::select(true, None).unwrap();
    assert_eq!(debug.filename, "instance-debug.lock");
    assert!(!debug.legacy_fallback);
    for debug in [false, true] {
        let isolated = Key::select(debug, Some(OsStr::new("nested-1_test"))).unwrap();
        assert_eq!(isolated.filename, "instance-nested-1_test.lock");
        assert!(!isolated.legacy_fallback);
    }
}

#[test]
fn override_accepts_exact_boundaries_and_rejects_invalid_names() {
    for valid in ["a", "0", "_", "-", "abcdefghijklmnopqrstuvwxyz012345"] {
        assert!(
            Key::select(false, Some(OsStr::new(valid))).is_ok(),
            "{valid}"
        );
    }
    for invalid in [
        "",
        "A",
        "a.b",
        "a/b",
        "a b",
        "a\nb",
        "é",
        "abcdefghijklmnopqrstuvwxyz0123456",
    ] {
        assert!(Key::select(false, Some(OsStr::new(invalid)))
            .err()
            .unwrap()
            .contains("ULTRA9000_INSTANCE"));
    }
    assert!(Key::select(false, Some(OsStr::from_bytes(b"\xff"))).is_err());
}

fn contended() -> (tempfile::TempDir, File, File, Key) {
    let dir = tempfile::tempdir().unwrap();
    let key = Key::select(true, None).unwrap();
    let path = dir.path().join(&key.filename);
    let mut owner = open_lock(&path).unwrap();
    assert!(acquire(&mut owner, &key.filename, 123).unwrap());
    let contender = open_lock(&path).unwrap();
    (dir, owner, contender, key)
}

#[test]
fn two_open_fds_contend_and_exit_releases_without_removing_the_lock_file() {
    let (dir, owner, mut loser, key) = contended();
    assert!(!acquire(&mut loser, &key.filename, 456).unwrap());
    assert_eq!(
        owner_pid(&loser, &key.filename),
        Some(123),
        "a loser never overwrites the pid"
    );
    drop(owner);
    assert!(
        dir.path().join(&key.filename).exists(),
        "never unlink the shared inode"
    );
    assert!(acquire(&mut loser, &key.filename, 456).unwrap());
    assert_eq!(owner_pid(&loser, &key.filename), Some(456));
}

#[test]
fn owner_record_requires_the_matching_key_a_complete_line_and_positive_pid() {
    let file = tempfile::tempfile().unwrap();
    for (text, expected) in [
        ("123 instance.lock\n", Some(123)),
        ("2147483647 instance.lock\n", Some(libc::pid_t::MAX)),
        ("123\n", None),
        ("123 instance-debug.lock\n", None),
        ("123 instance.lock", None),
        ("0 instance.lock\n", None),
        ("-1 instance.lock\n", None),
        ("2147483648 instance.lock\n", None),
        ("42 instance.lock\ntrash", None),
    ] {
        file.set_len(0).unwrap();
        file.write_all_at(text.as_bytes(), 0).unwrap();
        assert_eq!(owner_pid(&file, "instance.lock"), expected, "{text:?}");
        assert_eq!(
            owner_pid(&file, "instance.lock"),
            expected,
            "reading does not advance the fd offset"
        );
    }
}

#[test]
fn lock_open_rejects_symlinks_without_touching_the_target() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    fs::write(&target, "untouched").unwrap();
    let path = dir.path().join("instance.lock");
    symlink(&target, &path).unwrap();
    assert!(open_lock(&path).is_err());
    assert_eq!(fs::read_to_string(target).unwrap(), "untouched");
}

#[test]
fn starting_owner_can_publish_its_pid_and_socket_during_the_retry_window() {
    let (_dir, owner, mut file, key) = contended();
    owner.set_len(0).unwrap();
    let elapsed = Cell::new(Duration::ZERO);
    let attempts = Cell::new(0);
    let result = wait_for_owner(
        &mut file,
        &key,
        456,
        &mut |pid, _, timeout| {
            attempts.set(attempts.get() + 1);
            assert_eq!(pid, 123);
            assert_eq!(timeout, STARTUP_TIMEOUT - elapsed.get());
            if elapsed.get() >= Duration::from_millis(300) {
                Ok(())
            } else {
                Err("starting".into())
            }
        },
        &mut || None::<(libc::pid_t, ())>,
        &mut || elapsed.get(),
        &mut |delay| {
            elapsed.set(elapsed.get() + delay);
            if elapsed.get() == Duration::from_millis(100) {
                owner
                    .write_all_at(format!("123 {}\n", key.filename).as_bytes(), 0)
                    .unwrap();
            }
        },
    );
    assert_eq!(result, Ok(Outcome::HandedOver(123)));
    assert_eq!(elapsed.get(), Duration::from_millis(300));
    assert_eq!(attempts.get(), 3);
}

#[test]
fn held_lock_released_mid_wait_becomes_owned_and_publishes_the_new_pid() {
    let (_dir, owner, mut file, key) = contended();
    let mut owner = Some(owner);
    let elapsed = Cell::new(Duration::ZERO);
    let result = wait_for_owner(
        &mut file,
        &key,
        456,
        &mut |_, _, _| Err("quitting".into()),
        &mut || None::<(libc::pid_t, ())>,
        &mut || elapsed.get(),
        &mut |delay| {
            elapsed.set(elapsed.get() + delay);
            drop(owner.take());
        },
    );
    assert_eq!(result, Ok(Outcome::Owner));
    assert_eq!(elapsed.get(), RETRY_INTERVAL);
    assert_eq!(owner_pid(&file, &key.filename), Some(456));
}

#[test]
fn disconnected_handover_retries_until_the_same_owner_accepts() {
    let (_dir, _owner, mut file, key) = contended();
    let elapsed = Cell::new(Duration::ZERO);
    let mut attempts = 0;
    let result = wait_for_owner(
        &mut file,
        &key,
        456,
        &mut |pid, _, _| {
            assert_eq!(pid, 123);
            attempts += 1;
            if attempts == 3 {
                Ok(())
            } else {
                Err("disconnected".into())
            }
        },
        &mut || None::<(libc::pid_t, ())>,
        &mut || elapsed.get(),
        &mut |delay| elapsed.set(elapsed.get() + delay),
    );
    assert_eq!(result, Ok(Outcome::HandedOver(123)));
    assert_eq!(attempts, 3);
}

#[test]
fn stale_plain_pid_and_mismatched_key_are_not_used_for_handover() {
    for record in [
        "123\n",
        "123 instance-other.lock\n",
        "123 instance-debug.lock",
    ] {
        let (_dir, owner, mut file, key) = contended();
        owner.set_len(0).unwrap();
        owner.write_all_at(record.as_bytes(), 0).unwrap();
        let elapsed = Cell::new(Duration::ZERO);
        assert!(wait_for_owner(
            &mut file,
            &key,
            456,
            &mut |_, _, _| panic!("unpublished/wrong-key owner must not receive handover"),
            &mut || None::<(libc::pid_t, ())>,
            &mut || elapsed.get(),
            &mut |delay| elapsed.set(elapsed.get() + delay),
        )
        .is_err());
    }
}

#[test]
fn held_lock_and_failed_handover_time_out_instead_of_starting_another_app() {
    let (_dir, _owner, mut file, key) = contended();
    let elapsed = Cell::new(Duration::ZERO);
    let error = wait_for_owner(
        &mut file,
        &key,
        456,
        &mut |_, _, _| Err("pid 123: quitting".into()),
        &mut || None::<(libc::pid_t, ())>,
        &mut || elapsed.get(),
        &mut |delay| elapsed.set(elapsed.get() + delay),
    )
    .unwrap_err();
    assert_eq!(elapsed.get(), STARTUP_TIMEOUT);
    assert!(error.contains("within 5 s"));
    assert!(error.contains("pid 123: quitting"));
    assert!(error.contains("not starting another instance"));
}

#[test]
fn acquisition_after_wait_obeys_the_same_legacy_policy() {
    for (debug, override_name, scan_expected) in [
        (false, None, true),
        (true, None, false),
        (false, Some(OsStr::new("test")), false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let key = Key::select(debug, override_name).unwrap();
        let mut owner = open_lock(&dir.path().join(&key.filename)).unwrap();
        assert!(acquire(&mut owner, &key.filename, 123).unwrap());
        let mut owner = Some(owner);
        let elapsed = Cell::new(Duration::ZERO);
        let mut file = open_lock(&dir.path().join(&key.filename)).unwrap();
        let mut scans = 0;
        let outcome = wait_for_owner(
            &mut file,
            &key,
            456,
            &mut |pid, stream, _| {
                assert_eq!(pid, 123);
                if stream == Some(()) {
                    Ok(())
                } else {
                    Err("quitting".into())
                }
            },
            &mut || {
                scans += 1;
                Some((123, ()))
            },
            &mut || elapsed.get(),
            &mut |delay| {
                elapsed.set(elapsed.get() + delay);
                drop(owner.take());
            },
        )
        .unwrap();
        assert_eq!(scans != 0, scan_expected);
        assert_eq!(
            outcome,
            if scan_expected {
                Outcome::HandedOver(123)
            } else {
                Outcome::Owner
            }
        );
    }
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn legacy_discovery_requires_another_live_process_and_a_listening_socket() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut child = OwnedChild(Command::new("sleep").arg("60").spawn().unwrap());
    let pid = child.0.id();
    let path = dir.path().join(format!("control-{pid}.sock"));
    fs::write(&path, "not a socket").unwrap();
    assert!(legacy_owner(dir.path()).is_none());
    fs::remove_file(&path).unwrap();
    let listener = UnixListener::bind(&path).unwrap();
    let (owner, connection) = legacy_owner(dir.path()).unwrap();
    assert_eq!(u32::try_from(owner).unwrap(), pid);
    drop(connection);
    for key in ["instance-debug.lock", "instance-isolated.lock"] {
        let mut lock = open_lock(&dir.path().join(key)).unwrap();
        assert!(acquire(&mut lock, key, pid).unwrap());
        assert!(
            legacy_owner(dir.path()).is_none(),
            "a locked {key} owner is not legacy"
        );
        drop(lock);
        assert!(
            legacy_owner(dir.path()).is_some(),
            "an unlocked stale lock is ignored"
        );
    }
    drop(listener);
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    assert!(legacy_owner(dir.path()).is_none(), "dead owner ignored");
    let _self_socket = UnixListener::bind(
        dir.path()
            .join(format!("control-{}.sock", std::process::id())),
    )
    .unwrap();
    assert!(
        legacy_owner(dir.path()).is_none(),
        "never hand over to our own pid"
    );
}

#[test]
fn insecure_runtime_directory_reports_ownership_mode_and_recovery() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let error = prepare_runtime_dir(dir.path()).unwrap_err();
    for expected in [
        dir.path().to_str().unwrap(),
        "owner uid",
        "mode 0755",
        "XDG_RUNTIME_DIR",
        "TMPDIR",
        "remove it only after",
        "do not delete a live lock",
    ] {
        assert!(error.contains(expected), "missing {expected} from {error}");
    }
}

#[test]
fn crashed_process_releases_its_flock_for_the_next_launcher() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("instance.lock");
    let mut child = OwnedChild(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "instance::tests::child_holds_lock_until_exit",
                "--exact",
                "--nocapture",
            ])
            .env("U9K_TEST_INSTANCE_LOCK", &path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut reader = BufReader::new(child.0.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        assert_ne!(
            reader.read_line(&mut line).unwrap(),
            0,
            "child must acquire lock"
        );
        if line.contains("INSTANCE_LOCK_READY") {
            break;
        }
        line.clear();
    }
    let mut file = open_lock(&path).unwrap();
    assert!(!acquire(&mut file, "instance.lock", std::process::id()).unwrap());
    assert_eq!(
        owner_pid(&file, "instance.lock"),
        libc::pid_t::try_from(child.0.id()).ok()
    );
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    assert!(acquire(&mut file, "instance.lock", std::process::id()).unwrap());
}

#[test]
fn child_holds_lock_until_exit() {
    let Some(path) = std::env::var_os("U9K_TEST_INSTANCE_LOCK") else {
        return;
    };
    let mut file = open_lock(Path::new(&path)).unwrap();
    assert!(acquire(&mut file, "instance.lock", std::process::id()).unwrap());
    println!("INSTANCE_LOCK_READY");
    io::stdout().flush().unwrap();
    let _ = io::stdin().read(&mut [0]);
}
