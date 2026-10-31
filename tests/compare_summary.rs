use assert_cmd::cargo::cargo_bin_cmd;
use std::{fs, path::Path};

fn run(left: &Path, right: &Path, mode: &[&str], long: bool, code: i32) -> String {
    let mut command = cargo_bin_cmd!("keeper");
    command.arg("compare").args(mode).arg(left).arg(right);
    if long {
        command.arg("--long");
    }
    String::from_utf8(command.assert().code(code).get_output().stdout.clone()).unwrap()
}

fn assert_row(report: &str, expected: &str) {
    let expected: Vec<_> = expected.split_whitespace().collect();
    assert!(
        report
            .lines()
            .any(|line| line.split_whitespace().collect::<Vec<_>>() == expected),
        "missing row {expected:?} in:\n{report}"
    );
}

#[test]
fn all_modes_summarize_immediate_directories_and_long_adds_files() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    for root in [left.path(), right.path()] {
        fs::create_dir_all(root.join("photos/nested")).unwrap();
        fs::write(
            root.join("photos/changed.jpg"),
            if root == left.path() {
                b"a".as_slice()
            } else {
                b"bb"
            },
        )
        .unwrap();
    }
    fs::write(left.path().join("photos/left-one.jpg"), b"a").unwrap();
    fs::write(left.path().join("photos/left-two.jpg"), b"a").unwrap();
    fs::write(right.path().join("photos/right-one.jpg"), b"a").unwrap();
    fs::write(left.path().join("photos/nested/deep.jpg"), b"a").unwrap();
    fs::write(left.path().join("root-file"), b"a").unwrap();
    fs::write(left.path().join(".DS_Store"), b"a").unwrap();
    for root in [left.path(), right.path()] {
        cargo_bin_cmd!("keeper")
            .arg("init")
            .arg(root)
            .assert()
            .success();
    }
    for mode in [&["--metadata-only"][..], &["--live"][..], &[][..]] {
        let brief = run(left.path(), right.path(), mode, false, 1);
        assert_row(&brief, "Left-only Right-only Different Junk Directory");
        assert_row(&brief, "2 1 1 0 photos");
        assert_row(&brief, "1 0 0 0 photos/nested");
        assert_row(&brief, "2 0 0 1 .");
        assert!(!brief.contains("DIRECTORY"));
        let assessment = brief.find("Comparison:").unwrap();
        let inventory = brief.find("\nInventory\n").unwrap();
        let directories = brief.find("\nDirectory differences\n").unwrap();
        let notes = brief.find("\nNotes\n").unwrap();
        assert!(assessment < inventory && inventory < directories && directories < notes);
        assert!(brief.find("File counts by direct parent").unwrap() > notes);
        assert!(!brief.contains("File differences\n"));
        assert!(!brief.contains("changed.jpg"));
        assert!(!brief.contains("deep.jpg"));
        assert!(!brief.contains("root-file"));
        assert!(brief.contains("5 only left, 1 only right, 1 different"));
        assert!(brief.contains("Use --long"));
        let long = run(left.path(), right.path(), mode, true, 1);
        assert_row(&long, "2 1 1 0 photos");
        assert!(long.contains("ONLY_LEFT\tphotos/left-one.jpg"));
        assert!(long.contains("ONLY_RIGHT\tphotos/right-one.jpg"));
        assert!(long.contains("DIFFERENT\tphotos/changed.jpg\t"));
        assert!(long.contains("5 only left, 1 only right, 1 different"));
        assert!(!long.contains("Individual file differences omitted"));
        let directories = long.find("\nDirectory differences\n").unwrap();
        let files = long.find("\nFile differences\n").unwrap();
        let notes = long.find("\nNotes\n").unwrap();
        assert!(directories < files && files < notes);
        assert!(long.find("ONLY_LEFT\t").unwrap() > files);
    }
}

#[test]
fn explanatory_notes_are_dim_and_plain_reports_have_no_escapes() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    fs::write(left.path().join(".DS_Store"), b"a").unwrap();
    let output = cargo_bin_cmd!("keeper")
        .args([
            "compare",
            "--metadata-only",
            "--color",
            "always",
            "--progress",
            "never",
        ])
        .arg(left.path())
        .arg(right.path())
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let colored = String::from_utf8(output).unwrap();
    assert!(colored.contains("\x1b[2m- Paths + sizes only"));
    assert!(colored.contains("\x1b[2m- File counts by direct parent"));
    assert!(colored.contains("\x1b[2m- Junk ="));
    let plain = run(
        left.path(),
        right.path(),
        &["--metadata-only", "--color", "never"],
        false,
        1,
    );
    assert!(!plain.contains('\x1b'));
    assert!(plain.lines().count() <= 20, "{plain}");
    let notes = plain.find("\nNotes\n").unwrap();
    assert!(plain.find("Paths + sizes only").unwrap() > notes);
    assert!(plain.find("Junk =").unwrap() > notes);
    assert!(
        plain[notes..]
            .lines()
            .skip(2)
            .all(|line| line.starts_with("- "))
    );
}

#[test]
fn incomplete_summary_retains_warnings_without_listing_files() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("missing", left.path().join("link")).unwrap();
    fs::create_dir(left.path().join(".Trash")).unwrap();
    fs::write(left.path().join(".Trash/photo.jpg"), b"abc").unwrap();
    let brief = run(left.path(), right.path(), &["--metadata-only"], false, 2);
    assert!(brief.contains("Left ISSUE\tlink\t"));
    assert_row(&brief, "1 0 0 1 .Trash");
    assert!(brief.contains("Delta (R - L): -1 files, -3 B."));
    assert!(brief.contains("Observed:"));
    assert_eq!(brief.matches("INCOMPLETE").count(), 1);
    assert!(!brief.contains("Likely junk: all"));
    assert!(brief.starts_with("INCOMPLETE"));
    let notes = brief.find("\nNotes / issues\n").unwrap();
    assert!(brief.find("Left ISSUE\t").unwrap() > notes);
    assert!(brief.find("Observed files only").unwrap() > notes);
    assert!(brief.find("Paths + sizes only").unwrap() > notes);
    assert!(
        brief[notes..]
            .lines()
            .skip(2)
            .all(|line| line.starts_with("- "))
    );
    assert!(!brief.contains("photo.jpg"));
    assert!(!brief.contains("All reported differences are limited"));
}

#[test]
fn summary_escapes_directory_names_and_omits_unchanged_directories() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    fs::create_dir(left.path().join("odd\nname")).unwrap();
    fs::write(left.path().join("odd\nname/file"), b"a").unwrap();
    for root in [left.path(), right.path()] {
        fs::create_dir(root.join("unchanged")).unwrap();
        fs::write(root.join("unchanged/a"), b"a").unwrap();
    }
    let brief = run(left.path(), right.path(), &["--metadata-only"], false, 1);
    assert_row(&brief, "1 0 0 0 odd\\nname");
    assert!(!brief.contains("odd\nname"));
    assert!(!brief.contains("unchanged"));
    fs::remove_dir_all(left.path().join("odd\nname")).unwrap();
    let equal = run(left.path(), right.path(), &["--metadata-only"], false, 0);
    assert!(!equal.contains("Left-only"));
    assert!(!equal.contains("Individual file differences omitted"));
}
