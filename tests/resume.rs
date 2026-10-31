use assert_cmd::cargo::cargo_bin_cmd;
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn run(root: &Path, args: &[&str], code: i32) -> String {
    let result = cargo_bin_cmd!("keeper")
        .args(args)
        .arg(root)
        .assert()
        .code(code)
        .get_output()
        .clone();
    String::from_utf8(result.stdout).unwrap()
}

fn baseline(root: &Path) -> Vec<Vec<u8>> {
    ["MANIFEST", "paths.tsv", "objects.tsv"]
        .iter()
        .map(|name| fs::read(root.join(".keeper/catalog").join(name)).unwrap())
        .collect()
}

// A tiny first file followed by a huge sparse file makes interruption independent
// of disk speed. Wait for the durable first record, then signal during hashing.
fn interrupt(root: &Path, operation: &str, signal: i32) -> Vec<u8> {
    fs::write(root.join("a"), b"completed first").unwrap();
    fs::File::create(root.join("z-large"))
        .unwrap()
        .set_len(2 * 1024 * 1024 * 1024)
        .unwrap();
    let mut child = Command::new(assert_cmd::cargo::cargo_bin!("keeper"))
        .args([operation, "--resume"])
        .arg(root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let work = root.join(format!(".keeper/{operation}.work"));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if fs::read(&work).is_ok_and(|bytes| bytes.iter().filter(|&&b| b == b'\n').count() >= 2) {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("timed out waiting for checkpoint");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // SAFETY: signal a child process owned by this test.
    assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
    let status = child.wait().unwrap();
    if signal != libc::SIGKILL {
        assert_eq!(status.code(), Some(2));
    }
    fs::read(work).unwrap()
}

#[test]
fn clean_cancellation_and_process_death_resume_init_without_replacing_baseline() {
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGKILL] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let bytes = interrupt(root, "init", signal);
        assert!(!root.join(".keeper/catalog").exists());
        assert!(bytes.windows(2).any(|s| s == b"a\t"));
        // Incomplete last append is disposable, not grounds to trust partial data.
        fs::OpenOptions::new()
            .append(true)
            .open(root.join(".keeper/init.work"))
            .unwrap()
            .write_all(b"partial\xff")
            .unwrap();
        fs::remove_file(root.join("z-large")).unwrap();
        let report = run(root, &["init", "--resume"], 0);
        assert!(report.contains("1 file(s) reused"));
        assert!(report.contains("NOT a fresh full scrub"));
        assert!(!root.join(".keeper/init.work").exists());
        run(root, &["catalog", "check"], 0);
        run(root, &["status"], 0);
        let original = baseline(root);
        run(root, &["init", "--resume"], 2);
        assert_eq!(original, baseline(root));
    }
}

#[test]
fn resumed_status_keeps_baseline_and_plain_status_still_rereads() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("a"), b"completed first").unwrap();
    run(root, &["init"], 0);
    let original = baseline(root);
    interrupt(root, "status", libc::SIGTERM);
    fs::remove_file(root.join("z-large")).unwrap();
    let report = run(root, &["status", "--resume"], 0);
    assert!(report.contains("1 file(s) reused"));
    assert!(report.contains("Checkpointed session:"));
    assert!(!report.contains("Full-byte check:"));
    assert!(!root.join(".keeper/status.work").exists());
    assert_eq!(original, baseline(root));
    assert!(run(root, &["status"], 0).contains("Full-byte check:"));
    fs::write(root.join("a"), b"changed").unwrap();
    assert!(run(root, &["status", "--resume"], 1).contains("CHANGED\ta"));
    assert_eq!(original, baseline(root));
}

#[test]
fn rejects_wrong_root_corruption_unknown_version_and_symlink_work_files() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let bytes = interrupt(root, "init", libc::SIGTERM);
    fs::remove_file(root.join("z-large")).unwrap();
    let other = tempfile::tempdir().unwrap();
    fs::create_dir(other.path().join(".keeper")).unwrap();
    fs::write(other.path().join(".keeper/init.work"), &bytes).unwrap();
    run(other.path(), &["init", "--resume"], 2);
    assert!(!other.path().join(".keeper/catalog").exists());
    let work = root.join(".keeper/init.work");
    let mut corrupt = bytes.clone();
    corrupt[0] ^= 1;
    fs::write(&work, corrupt).unwrap();
    run(root, &["init", "--resume"], 2);
    let mut corrupt_record = bytes.clone();
    let record_start = corrupt_record.iter().position(|&b| b == b'\n').unwrap() + 1;
    corrupt_record[record_start] ^= 1;
    fs::write(&work, corrupt_record).unwrap();
    run(root, &["init", "--resume"], 2);
    // A valid checksum with an unknown version is also refused.
    let text = String::from_utf8(bytes).unwrap();
    let (header, records) = text.split_once('\n').unwrap();
    let (payload, _) = header.rsplit_once('\t').unwrap();
    let payload = payload.replacen("checkpoint\t1", "checkpoint\t99", 1);
    fs::write(
        &work,
        format!(
            "{payload}\t{}\n{records}",
            blake3::hash(payload.as_bytes()).to_hex()
        ),
    )
    .unwrap();
    run(root, &["init", "--resume"], 2);
    fs::remove_file(&work).unwrap();
    let target = other.path().join("target");
    fs::write(&target, b"untouched").unwrap();
    std::os::unix::fs::symlink(&target, &work).unwrap();
    run(root, &["init", "--resume"], 2);
    assert_eq!(fs::read(target).unwrap(), b"untouched");
    assert!(!root.join(".keeper/catalog").exists());
}

#[test]
fn checkpoint_writers_are_excluded_but_plain_status_remains_read_only() {
    use fs2::FileExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    run(root, &["init"], 0);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join(".keeper/LOCK"))
        .unwrap();
    lock.try_lock_exclusive().unwrap();
    run(root, &["status", "--resume"], 2);
    run(root, &["status"], 0);
    assert!(!root.join(".keeper/status.work").exists());
}

#[test]
fn changed_baseline_rejects_old_verification_session() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    run(root, &["init"], 0);
    interrupt(root, "status", libc::SIGTERM);
    fs::remove_file(root.join("z-large")).unwrap();
    // Simulate an externally supplied *valid* different baseline, not damage.
    let other = tempfile::tempdir().unwrap();
    fs::write(other.path().join("different"), b"bytes").unwrap();
    run(other.path(), &["init"], 0);
    for name in ["MANIFEST", "paths.tsv", "objects.tsv"] {
        fs::copy(
            other.path().join(".keeper/catalog").join(name),
            root.join(".keeper/catalog").join(name),
        )
        .unwrap();
    }
    let expected = baseline(root);
    run(root, &["status", "--resume"], 2);
    assert_eq!(expected, baseline(root));
}
