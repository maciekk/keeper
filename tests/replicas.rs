use assert_cmd::cargo::cargo_bin_cmd;
use std::{
    fs,
    os::unix::{ffi::OsStringExt, fs::symlink},
    path::Path,
    process::Output,
};

fn run(args: &[&str], roots: &[&Path], code: i32) -> Output {
    // These detailed-report tests exercise individual file records.
    let mut command = cargo_bin_cmd!("keeper");
    command.args(args);
    if args.first() == Some(&"compare") {
        command.arg("--long");
    }
    command
        .args(roots)
        .timeout(std::time::Duration::from_secs(10))
        .assert()
        .code(code)
        .get_output()
        .clone()
}

fn stdout(output: &Output) -> &str {
    std::str::from_utf8(&output.stdout).unwrap()
}
fn stderr(output: &Output) -> &str {
    std::str::from_utf8(&output.stderr).unwrap()
}

fn baseline(root: &Path) -> Vec<Vec<u8>> {
    ["MANIFEST", "objects.tsv", "paths.tsv"]
        .iter()
        .map(|name| fs::read(root.join(".keeper/catalog").join(name)).unwrap())
        .collect()
}

#[test]
fn preflight_collects_all_issues_without_creating_metadata_or_reading_contents() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("regular"), b"abc").unwrap();
    symlink("regular", root.join("link-one")).unwrap();
    symlink("missing", root.join("link-two")).unwrap();
    // A FIFO would hang if preflight tried to read/open it synchronously.
    let path = std::ffi::CString::new(root.join("fifo").as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: valid NUL-terminated path; mkfifo does not retain the pointer.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let result = run(&["preflight"], &[root], 2);
    let report = stdout(&result);
    assert!(report.contains("ISSUE\tlink-one\t"));
    assert!(report.contains("ISSUE\tlink-two\t"));
    assert!(report.contains("ISSUE\tfifo\t"));
    assert!(report.contains("1 regular file(s), 3 B, 3 issue(s)"));
    assert!(report.contains("Contents not read"));
    assert!(!root.join(".keeper").exists());
    // Init performs the same preflight before any hashing or publication.
    let result = run(&["init", "--progress", "always"], &[root], 2);
    assert!(!stderr(&result).contains("Hashing:"));
    assert!(!root.join(".keeper/catalog").exists());
}

#[test]
fn preflight_counts_hidden_and_nested_keeper_files_but_excludes_root_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("nested/.keeper")).unwrap();
    fs::write(tmp.path().join("nested/.keeper/file"), b"nested").unwrap();
    fs::write(tmp.path().join(".hidden"), b"hidden").unwrap();
    run(&["init"], &[tmp.path()], 0);
    let before = baseline(tmp.path());
    let result = run(&["preflight"], &[tmp.path()], 0);
    assert!(stdout(&result).contains("2 regular file(s), 12 B, 0 issue(s)"));
    assert_eq!(baseline(tmp.path()), before);
}

#[test]
fn catalog_comparison_does_not_claim_to_check_current_bytes() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    for root in [left.path(), right.path()] {
        fs::write(root.join("a"), b"original").unwrap();
        run(&["init"], &[root], 0);
    }
    let left_before = baseline(left.path());
    let right_before = baseline(right.path());
    fs::write(right.path().join("a"), b"bit rot!").unwrap();
    let result = run(&["compare"], &[left.path(), right.path()], 0);
    assert!(stdout(&result).contains("recorded catalogs only"));
    assert!(stdout(&result).contains("bytes were NOT checked"));
    assert!(stdout(&result).contains("1 matching"));
    let result = run(&["compare", "--live"], &[left.path(), right.path()], 1);
    assert!(stdout(&result).contains("freshly read file bytes"));
    assert!(stdout(&result).contains("DIFFERENT\ta\tleft=b3:"));
    assert_eq!(baseline(left.path()), left_before);
    assert_eq!(baseline(right.path()), right_before);
}

#[test]
fn live_comparison_needs_no_catalog_and_is_path_sensitive() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    for root in [left.path(), right.path()] {
        fs::write(root.join("same"), b"identical").unwrap();
    }
    run(&["compare", "--live"], &[left.path(), right.path()], 0);
    fs::write(left.path().join("old-name"), b"same content").unwrap();
    fs::write(right.path().join("new-name"), b"same content").unwrap();
    let result = run(&["compare", "--live"], &[left.path(), right.path()], 1);
    assert!(stdout(&result).contains("ONLY_LEFT\told-name"));
    assert!(stdout(&result).contains("ONLY_RIGHT\tnew-name"));
    assert!(stdout(&result).contains("1 matching, 1 only left, 1 only right, 0 different"));
    assert!(!left.path().join(".keeper").exists());
    assert!(!right.path().join(".keeper").exists());
    run(&["compare"], &[left.path(), right.path()], 2);
}

#[test]
fn catalog_comparison_reports_every_difference_and_rejects_corruption() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    fs::write(left.path().join("left-only"), b"left").unwrap();
    fs::write(right.path().join("right-only"), b"right").unwrap();
    fs::write(left.path().join("different"), b"a").unwrap();
    fs::write(right.path().join("different"), b"bb").unwrap();
    for root in [left.path(), right.path()] {
        run(&["init"], &[root], 0);
    }
    let result = run(&["compare"], &[left.path(), right.path()], 1);
    assert!(stdout(&result).contains("ONLY_LEFT\tleft-only"));
    assert!(stdout(&result).contains("ONLY_RIGHT\tright-only"));
    assert!(stdout(&result).contains("left_size=1"));
    assert!(stdout(&result).contains("right_size=2"));
    fs::write(right.path().join(".keeper/catalog/MANIFEST"), b"bad").unwrap();
    let result = run(&["compare"], &[left.path(), right.path()], 2);
    assert_eq!(result.stdout, b"\n"); // Opening separator, no partial report.
    assert!(stderr(&result).contains("right catalog"));
    // Live mode explicitly ignores catalogs, including corrupt ones.
    run(&["compare", "--live"], &[left.path(), right.path()], 1);
}

#[test]
fn live_checks_both_preflights_before_hashing_and_never_reports_partial_agreement() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    fs::write(left.path().join("a"), b"readable").unwrap();
    symlink("missing", right.path().join("bad")).unwrap();
    let result = run(
        &["compare", "--live", "--progress", "always"],
        &[left.path(), right.path()],
        2,
    );
    assert_eq!(result.stdout, b"\n"); // Opening separator, no partial report.
    assert!(stderr(&result).contains("Left Preflight"));
    assert!(stderr(&result).contains("Right Preflight"));
    assert!(!stderr(&result).contains("Hashing:"));
    symlink("missing", left.path().join("left-bad")).unwrap();
    let both = run(&["compare", "--live"], &[left.path(), right.path()], 2);
    assert_eq!(both.stdout, b"\n"); // Opening separator, no partial report.
    assert!(stderr(&both).contains("left dataset:"));
    assert!(stderr(&both).contains("ISSUE\tleft-bad\t"));
    assert!(stderr(&both).contains("right dataset:"));
    assert!(stderr(&both).contains("ISSUE\tbad\t"));
    assert!(!left.path().join(".keeper").exists());
    assert!(!right.path().join(".keeper").exists());
}

#[test]
fn progress_is_on_stderr_and_can_be_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("a"), vec![0; 300_001]).unwrap();
    let result = run(&["init", "--progress", "always"], &[tmp.path()], 0);
    assert!(stderr(&result).contains("Preflight: completed"));
    assert!(
        stderr(&result)
            .contains("Hashing: completed; 100.0%; files=1/1; read=292.97 KiB/292.97 KiB")
    );
    assert!(!stdout(&result).contains("Hashing:"));
    let result = run(&["status", "--progress", "never"], &[tmp.path()], 0);
    assert!(result.stderr.is_empty());
    // Pipes disable progress in auto mode.
    let result = run(&["status"], &[tmp.path()], 0);
    assert!(result.stderr.is_empty());
}

#[test]
fn comparisons_refuse_identical_or_overlapping_roots_including_aliases() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("child")).unwrap();
    symlink(tmp.path(), tmp.path().join("alias")).unwrap();
    for other in [
        tmp.path().to_path_buf(),
        tmp.path().join("child"),
        tmp.path().join("alias"),
    ] {
        run(&["compare", "--live"], &[tmp.path(), &other], 2);
        run(&["compare"], &[tmp.path(), &other], 2);
    }
}

#[test]
fn comparison_escapes_names_and_checks_every_duplicate_copy() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let name = std::ffi::OsString::from_vec(b"newline\ncontrol\x1b-\xff".to_vec());
    for root in [left.path(), right.path()] {
        fs::write(root.join(&name), b"one").unwrap();
        fs::write(root.join("duplicate"), b"one").unwrap();
    }
    fs::write(right.path().join(&name), b"two").unwrap();
    let result = run(&["compare", "--live"], &[left.path(), right.path()], 1);
    assert!(stdout(&result).contains("DIFFERENT\tnewline\\ncontrol\\x1b-\\xff\t"));
    assert!(stdout(&result).contains("1 matching, 0 only left, 0 only right, 1 different"));
    assert!(!stdout(&result).contains('\x1b'));
}

#[test]
fn first_milestone_catalog_remains_readable_without_reindexing() {
    // Captured from the release binary for foundation commit 85debcc, not
    // regenerated by the implementation under test.
    let tmp = tempfile::tempdir().unwrap();
    let catalog = tmp.path().join(".keeper/catalog");
    fs::create_dir_all(&catalog).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/genesis-v1");
    for name in ["MANIFEST", "objects.tsv", "paths.tsv"] {
        fs::copy(fixture.join(name), catalog.join(name)).unwrap();
    }
    fs::write(tmp.path().join("a"), b"abc").unwrap();
    let original = baseline(tmp.path());
    run(&["catalog", "check"], &[tmp.path()], 0);
    run(&["status"], &[tmp.path()], 0);
    let other = tempfile::tempdir().unwrap();
    fs::write(other.path().join("a"), b"abc").unwrap();
    run(&["init"], &[other.path()], 0);
    run(&["compare"], &[tmp.path(), other.path()], 0);
    assert_eq!(baseline(tmp.path()), original);
    assert_eq!(baseline(other.path()), original);
}
