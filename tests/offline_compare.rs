use assert_cmd::cargo::cargo_bin_cmd;
use std::{fs, path::Path};

fn run(args: &[&str], paths: &[&Path], code: i32) -> String {
    let output = cargo_bin_cmd!("keeper")
        .args(args)
        .args(paths)
        .assert()
        .code(code)
        .get_output()
        .stdout
        .clone();
    String::from_utf8(output).unwrap()
}

#[test]
fn exported_catalogs_work_after_original_drives_are_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let left = tmp.path().join("ssd");
    let right = tmp.path().join("DATA2");
    let a = tmp.path().join("ssd-catalog");
    let b = tmp.path().join("DATA2-catalog");
    for root in [&left, &right] {
        fs::create_dir(root).unwrap();
        fs::write(root.join("file"), b"same bytes").unwrap();
        fs::write(root.join(".keeperignore"), b"ignored/\n").unwrap();
        run(&["init"], &[root], 0);
    }
    run(&["catalog", "export"], &[&left, &a], 0);
    run(&["catalog", "export"], &[&right, &b], 0);
    let before = fs::read(a.join("MANIFEST")).unwrap();
    run(&["catalog", "export"], &[&left, &a], 2);
    assert_eq!(fs::read(a.join("MANIFEST")).unwrap(), before);
    fs::remove_dir_all(&left).unwrap();
    fs::remove_dir_all(&right).unwrap();
    run(&["compare", "--catalogs"], &[&a, &b], 0);
    fs::write(b.join("paths.tsv"), b"corrupted").unwrap();
    run(&["compare", "--catalogs"], &[&a, &b], 2);
}

#[test]
fn offline_differences_and_incompatible_modes() {
    let tmp = tempfile::tempdir().unwrap();
    let mut catalogs = Vec::new();
    for (name, bytes) in [("left", "aaa"), ("right", "bbb")] {
        let root = tmp.path().join(name);
        fs::create_dir(&root).unwrap();
        fs::write(root.join("file"), bytes).unwrap();
        run(&["init"], &[&root], 0);
        catalogs.push(root.join(".keeper/catalog"));
    }
    let paths = [catalogs[0].as_path(), catalogs[1].as_path()];
    run(&["compare", "--catalogs"], &paths, 1);
    run(&["compare", "--catalogs", "--live"], &paths, 2);
    run(&["compare", "--catalogs", "--metadata-only"], &paths, 2);
}
