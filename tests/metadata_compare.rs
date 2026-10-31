use assert_cmd::cargo::cargo_bin_cmd;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
};

fn compare(left: &Path, right: &Path, code: i32) -> String {
    compare_output(left, right, code, true)
}

fn compare_output(left: &Path, right: &Path, code: i32, long: bool) -> String {
    let mut command = cargo_bin_cmd!("keeper");
    command.args(["compare", "--metadata-only", "--progress", "always"]);
    if long {
        command.arg("--long");
    }
    let output = command
        .arg(left)
        .arg(right)
        .assert()
        .code(code)
        .get_output()
        .clone();
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Hashing"));
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn compares_paths_and_sizes_without_catalogs_or_content_equality_claims() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    fs::write(left.path().join("same-size"), b"aaa").unwrap();
    fs::write(right.path().join("same-size"), b"bbb").unwrap();
    fs::write(left.path().join("size-change"), b"a").unwrap();
    fs::write(right.path().join("size-change"), b"bb").unwrap();
    fs::write(left.path().join("left-only"), b"left").unwrap();
    fs::write(right.path().join("right-only"), b"right").unwrap();
    // An invalid catalog is ignored, just as in live mode.
    fs::create_dir(left.path().join(".keeper")).unwrap();
    fs::write(left.path().join(".keeper/catalog"), b"not a catalog").unwrap();
    let report = compare(left.path(), right.path(), 1);
    assert!(report.contains("Paths + sizes only; contents unchecked."));
    assert!(report.contains("ONLY_LEFT\tleft-only"));
    assert!(report.contains("ONLY_RIGHT\tright-only"));
    assert!(report.contains("DIFFERENT\tsize-change\tleft_size=1 B\tright_size=2 B"));
    assert!(report.contains("1 matching, 1 only left, 1 only right, 1 different"));
    assert!(report.contains("Delta (R - L): +0 files, +2 B."));
    assert!(!report.contains("All reported differences are limited"));
    assert!(!right.path().join(".keeper").exists());
    assert_eq!(
        fs::read(left.path().join(".keeper/catalog")).unwrap(),
        b"not a catalog"
    );
    assert_eq!(fs::read(right.path().join("same-size")).unwrap(), b"bbb");
}

#[test]
fn classifies_but_never_hides_housekeeping_differences() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    fs::create_dir_all(left.path().join(".Trash-1000/files")).unwrap();
    fs::write(left.path().join(".Trash-1000/files/valuable-photo"), b"a").unwrap();
    fs::create_dir_all(right.path().join("System Volume Information")).unwrap();
    fs::write(right.path().join("System Volume Information/index"), b"bb").unwrap();
    let report = compare(left.path(), right.path(), 1);
    assert!(
        report.contains("ONLY_LEFT\t.Trash-1000/files/valuable-photo\tcategory=OS_HOUSEKEEPING")
    );
    assert!(
        report.contains("ONLY_RIGHT\tSystem Volume Information/index\tcategory=OS_HOUSEKEEPING")
    );
    assert!(report.contains("Likely junk: all 2 differing paths."));
    assert!(report.contains("trash may hold user data"));
    fs::write(left.path().join("user-data"), b"x").unwrap();
    let report = compare(left.path(), right.path(), 1);
    assert!(report.contains("Likely junk: 2 paths; other: 1."));
    assert!(!report.contains("All reported differences are limited"));
    // Annotation also applies to recorded and live content comparisons.
    for root in [left.path(), right.path()] {
        cargo_bin_cmd!("keeper")
            .arg("init")
            .arg(root)
            .assert()
            .success();
    }
    for mode in [vec!["compare"], vec!["compare", "--live"]] {
        let output = cargo_bin_cmd!("keeper")
            .args(mode)
            .arg("--long")
            .arg(left.path())
            .arg(right.path())
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();
        assert!(
            String::from_utf8(output)
                .unwrap()
                .contains("category=OS_HOUSEKEEPING")
        );
    }
}

#[test]
fn equal_size_is_only_a_metadata_match_and_empty_directories_are_ignored() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    fs::write(left.path().join("a"), b"a").unwrap();
    fs::write(right.path().join("a"), b"b").unwrap();
    fs::create_dir(left.path().join("empty")).unwrap();
    let report = compare(left.path(), right.path(), 0);
    assert!(report.contains("1 matching"));
    assert!(report.contains("contents unchecked"));
}

#[test]
fn unreadable_directories_report_observed_deltas_without_claiming_absence() {
    // Root can bypass mode bits, so this permission regression requires an ordinary user.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let blocked = left.path().join("lost+found");
    fs::create_dir(&blocked).unwrap();
    fs::write(blocked.join("hidden"), b"abc").unwrap();
    fs::create_dir(right.path().join("lost+found")).unwrap();
    fs::write(right.path().join("lost+found/hidden"), b"abc").unwrap();
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
    let report = compare(left.path(), right.path(), 2);
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(report.contains("Left ISSUE\tlost+found\tPermission denied"));
    assert!(report.contains("Delta (R - L): +1 files, +3 B."));
    assert!(report.contains("OBSERVED_ONLY_RIGHT\tlost+found/hidden"));
    assert!(report.contains("unreadable paths may change totals and apparent absences"));
}

#[test]
fn rejects_unsupported_objects_overlaps_and_conflicting_modes() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    symlink("missing", left.path().join("link")).unwrap();
    symlink("missing", right.path().join("right-link")).unwrap();
    fs::create_dir(left.path().join(".Trash")).unwrap();
    fs::write(left.path().join(".Trash/file"), b"abc").unwrap();
    let report = compare(left.path(), right.path(), 2);
    assert!(!report.contains("Comparison:"));
    assert!(report.contains("Left ISSUE\tlink\t"));
    assert!(report.contains("Right ISSUE\tright-link\t"));
    assert!(report.contains("Delta (R - L): -1 files, -3 B."));
    assert!(report.contains("OBSERVED_ONLY_LEFT\t.Trash/file"));
    assert!(report.contains("Observed:"));
    assert_eq!(report.matches("INCOMPLETE").count(), 1);
    assert!(report.contains("Observed files only"));
    assert!(!report.contains("Likely junk: all"));
    assert!(!report.contains("All reported differences are limited"));
    compare(right.path(), right.path(), 2);
    fs::create_dir(right.path().join("nested")).unwrap();
    compare(right.path(), &right.path().join("nested"), 2);
    cargo_bin_cmd!("keeper")
        .args(["compare", "--live", "--metadata-only"])
        .arg(left.path())
        .arg(right.path())
        .assert()
        .code(2);
}
