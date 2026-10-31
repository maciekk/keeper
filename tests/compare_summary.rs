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
        assert_row(&brief, "Directory Left-only Right-only Different Junk");
        assert_row(&brief, "photos 2 1 1 0");
        assert_row(&brief, "photos/nested 1 0 0 0");
        assert_row(&brief, ". 2 0 0 1");
        assert!(!brief.contains("OBSERVED_DIRECTORY"));
        let assessment = brief.find("Comparison:").unwrap();
        let inventory = brief.find("\nINVENTORY\n").unwrap();
        let directories = brief.find("\nDIRECTORY DIFFERENCES\n").unwrap();
        let notes = brief.find("\nNOTES\n").unwrap();
        assert!(assessment < inventory && inventory < directories && directories < notes);
        let inventory_text = &brief[inventory..directories];
        assert!(inventory_text.contains("Root (L / R)"));
        assert!(inventory_text.contains(left.path().to_str().unwrap()));
        assert!(inventory_text.contains(right.path().to_str().unwrap()));
        assert!(!inventory_text.contains("Side"));
        let rows: Vec<_> = inventory_text
            .lines()
            .filter(|line| line.trim_start().starts_with('/'))
            .collect();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].starts_with(left.path().to_str().unwrap()));
        assert!(rows[1].starts_with(right.path().to_str().unwrap()));
        assert!(brief.find("File counts by direct parent").unwrap() > notes);
        assert!(!brief.contains("FILE DIFFERENCES\n"));
        assert!(!brief.contains("changed.jpg"));
        assert!(!brief.contains("deep.jpg"));
        assert!(!brief.contains("root-file"));
        assert!(brief.contains("5 only left, 1 only right, 1 different"));
        assert!(brief.contains("Use --long"));
        let long = run(left.path(), right.path(), mode, true, 1);
        assert_row(&long, "photos 2 1 1 0");
        assert!(long.contains("ONLY_LEFT\tphotos/left-one.jpg"));
        assert!(long.contains("ONLY_RIGHT\tphotos/right-one.jpg"));
        assert!(long.contains("DIFFERENT\tphotos/changed.jpg\t"));
        assert!(long.contains("5 only left, 1 only right, 1 different"));
        assert!(!long.contains("Individual file differences omitted"));
        let directories = long.find("\nDIRECTORY DIFFERENCES\n").unwrap();
        let files = long.find("\nFILE DIFFERENCES\n").unwrap();
        let notes = long.find("\nNOTES\n").unwrap();
        assert!(directories < files && files < notes);
        assert!(long.find("ONLY_LEFT\t").unwrap() > files);
    }
}

#[test]
fn inventory_preserves_and_escapes_supplied_root_arguments() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("left\nroot")).unwrap();
    fs::create_dir(temp.path().join("right")).unwrap();
    let output = cargo_bin_cmd!("keeper")
        .current_dir(temp.path())
        .args(["compare", "--metadata-only", "./left\nroot", "./right"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report = String::from_utf8(output).unwrap();
    assert!(report.contains("./left\\nroot"));
    assert!(report.contains("./right"));
    assert!(!report.contains("left\nroot"));
    assert!(!report.contains(temp.path().to_str().unwrap()));
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
    assert!(colored.contains(&format!("\x1b[0;34m{}", left.path().display())));
    assert!(colored.contains("\x1b[0;34m."));
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
    let notes = plain.find("\nNOTES\n").unwrap();
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
    assert_row(&brief, ".Trash 1 0 0 1");
    assert!(brief.contains("Delta (R - L): -1 files, -3 B."));
    assert!(brief.contains("Observed:"));
    assert_eq!(brief.matches("INCOMPLETE").count(), 1);
    assert!(!brief.contains("Likely junk: all"));
    assert!(brief.starts_with("\nINCOMPLETE"));
    let notes = brief.find("\nNOTES / ISSUES\n").unwrap();
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
    assert_row(&brief, "odd\\nname 1 0 0 0");
    assert!(!brief.contains("odd\nname"));
    assert!(!brief.contains("unchanged"));
    fs::remove_dir_all(left.path().join("odd\nname")).unwrap();
    let equal = run(left.path(), right.path(), &["--metadata-only"], false, 0);
    assert!(!equal.contains("Left-only"));
    assert!(!equal.contains("Individual file differences omitted"));
}
