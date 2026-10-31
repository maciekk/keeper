use assert_cmd::{Command, cargo::cargo_bin_cmd};
use std::{
    ffi::OsString,
    fs,
    os::unix::{ffi::OsStringExt, fs::symlink},
    path::Path,
};

fn keeper(args: &[&str], root: &Path) -> Command {
    let mut command = cargo_bin_cmd!("keeper");
    command.args(args).arg(root);
    command
}

fn output(args: &[&str], root: &Path, code: i32) -> String {
    let result = keeper(args, root).assert().code(code).get_output().clone();
    String::from_utf8(result.stdout).unwrap()
}

fn metadata(root: &Path) -> Vec<Vec<u8>> {
    ["MANIFEST", "objects.tsv", "paths.tsv"]
        .iter()
        .map(|name| fs::read(root.join(".keeper/catalog").join(name)).unwrap())
        .collect()
}

fn rehash_manifest(root: &Path) {
    let catalog = root.join(".keeper/catalog");
    let mut manifest = "keeper-catalog-version\t1\ngeneration\t1\n".to_owned();
    for name in ["objects.tsv", "paths.tsv"] {
        manifest.push_str(&format!(
            "file\t{name}\tb3:{}\n",
            blake3::hash(&fs::read(catalog.join(name)).unwrap()).to_hex()
        ));
    }
    fs::write(catalog.join("MANIFEST"), manifest).unwrap();
}

#[test]
fn init_check_and_status_preserve_baseline() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir(root.join("photos")).unwrap();
    fs::write(root.join("photos/a"), b"known bytes").unwrap();
    fs::write(root.join("deleted"), b"gone").unwrap();
    assert!(output(&["init"], root, 0).contains("2 file(s)"));
    assert!(output(&["catalog", "check"], root, 0).contains("Catalog OK"));
    assert!(output(&["status"], root, 0).contains("0 added, 0 missing, 0 changed"));
    let baseline = metadata(root);
    fs::write(root.join("photos/a"), b"other bytes").unwrap();
    fs::remove_file(root.join("deleted")).unwrap();
    fs::write(root.join("added"), b"new").unwrap();
    let report = output(&["status"], root, 1);
    assert!(report.contains("CHANGED\tphotos/a\texpected=b3:"));
    assert!(report.contains("MISSING\tdeleted"));
    assert!(report.contains("ADDED\tadded"));
    assert!(report.contains("1 added, 1 missing, 1 changed"));
    assert_eq!(metadata(root), baseline);
    output(&["init"], root, 2);
    assert_eq!(metadata(root), baseline);
}

#[test]
fn detects_same_size_change_with_preserved_mtime() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("data");
    fs::write(&path, b"original").unwrap();
    let mtime = fs::metadata(&path).unwrap().modified().unwrap();
    output(&["init"], tmp.path(), 0);
    fs::write(&path, b"bit rot!").unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(mtime))
        .unwrap();
    assert!(output(&["status"], tmp.path(), 1).contains("CHANGED\tdata"));
}

#[test]
fn arbitrary_unix_filenames_are_reversible_and_safe_to_display() {
    let tmp = tempfile::tempdir().unwrap();
    let name = OsString::from_vec(b"space tab\tnewline\nslash\\escape\x1b-\xff".to_vec());
    let path = tmp.path().join(&name);
    fs::write(&path, b"abc").unwrap();
    output(&["init"], tmp.path(), 0);
    output(&["catalog", "check"], tmp.path(), 0);
    output(&["status"], tmp.path(), 0);
    let paths = fs::read_to_string(tmp.path().join(".keeper/catalog/paths.tsv")).unwrap();
    assert!(paths.contains("space tab\\tnewline\\nslash\\\\escape\\x1b-\\xff\tfile\tb3:"));
    assert!(paths.contains(&format!("b3:{}", blake3::hash(b"abc").to_hex())));
    fs::remove_file(path).unwrap();
    let report = output(&["status"], tmp.path(), 1);
    assert!(report.contains("MISSING\tspace tab\\tnewline\\nslash\\\\escape\\x1b-\\xff"));
    assert!(!report.contains('\x1b'));
}

#[test]
fn empty_tree_and_duplicate_content() {
    let tmp = tempfile::tempdir().unwrap();
    output(&["init"], tmp.path(), 0);
    output(&["catalog", "check"], tmp.path(), 0);
    output(&["status"], tmp.path(), 0);

    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("a"), b"duplicate").unwrap();
    fs::write(tmp.path().join("b"), b"duplicate").unwrap();
    fs::hard_link(tmp.path().join("a"), tmp.path().join("c")).unwrap();
    output(&["init"], tmp.path(), 0);
    let objects = fs::read_to_string(tmp.path().join(".keeper/catalog/objects.tsv")).unwrap();
    assert_eq!(objects.lines().count(), 2); // header + one content object
    output(&["status"], tmp.path(), 0);
    fs::write(tmp.path().join("a"), b"different").unwrap();
    assert!(output(&["status"], tmp.path(), 1).contains("2 changed"));
}

#[test]
fn symlinks_fail_without_publishing_and_retry_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("regular"), b"safe").unwrap();
    symlink("regular", tmp.path().join("link")).unwrap();
    output(&["init"], tmp.path(), 2);
    assert!(!tmp.path().join(".keeper/catalog").exists());
    fs::remove_file(tmp.path().join("link")).unwrap();
    output(&["init"], tmp.path(), 0);
    let baseline = metadata(tmp.path());
    symlink("regular", tmp.path().join("link")).unwrap();
    output(&["status"], tmp.path(), 2);
    assert_eq!(metadata(tmp.path()), baseline);
    // Metadata-only check deliberately does not examine the dataset.
    output(&["catalog", "check"], tmp.path(), 0);
}

#[test]
fn special_objects_are_not_opened() {
    let tmp = tempfile::tempdir().unwrap();
    let _socket = std::os::unix::net::UnixListener::bind(tmp.path().join("socket")).unwrap();
    output(&["init"], tmp.path(), 2);
    assert!(!tmp.path().join(".keeper/catalog").exists());
}

#[test]
fn metadata_directory_and_members_cannot_be_symlinks() {
    let tmp = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    symlink(elsewhere.path(), tmp.path().join(".keeper")).unwrap();
    output(&["init"], tmp.path(), 2);
    assert_eq!(fs::read_dir(elsewhere.path()).unwrap().count(), 0);
    fs::remove_file(tmp.path().join(".keeper")).unwrap();
    output(&["init"], tmp.path(), 0);
    let paths = tmp.path().join(".keeper/catalog/paths.tsv");
    fs::rename(&paths, elsewhere.path().join("paths.tsv")).unwrap();
    symlink(elsewhere.path().join("paths.tsv"), &paths).unwrap();
    output(&["catalog", "check"], tmp.path(), 2);
    output(&["status"], tmp.path(), 2);
}

#[test]
fn missing_or_corrupt_catalog_is_an_error_not_a_clean_status() {
    let tmp = tempfile::tempdir().unwrap();
    output(&["status"], tmp.path(), 2);
    assert!(!tmp.path().join(".keeper").exists());
    output(&["init"], tmp.path(), 0);
    fs::write(tmp.path().join(".keeper/catalog/paths.tsv"), b"corrupt").unwrap();
    output(&["catalog", "check"], tmp.path(), 2);
    output(&["status"], tmp.path(), 2);
}

#[test]
fn checks_structure_even_when_manifest_checksums_match() {
    for invalid in [
        "../escape\tfile\tHASH\t3\n",
        "/absolute\tfile\tHASH\t3\n",
        ".keeper/hidden\tfile\tHASH\t3\n",
        "a\tfile\tHASH\t03\n",
        "a\tfile\tHASH\t3\na\tfile\tHASH\t3\n",
        "a\tfile\tHASH\t3\na/b\tfile\tHASH\t3\n",
        "b\tfile\tHASH\t3\na\tfile\tHASH\t3\n",
        "\\x61\tfile\tHASH\t3\n",
        "a\tfile\tHASH\t4\n", // contradicts objects.tsv
        "a\tfile\tb3:123\t3\n",
        "a\tfile\tHASH\t3", // no final newline
    ] {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a"), b"abc").unwrap();
        output(&["init"], tmp.path(), 0);
        let hash = format!("b3:{}", blake3::hash(b"abc").to_hex());
        let table = format!(
            "path\tkind\tobject\tsize\n{}",
            invalid.replace("HASH", &hash)
        );
        fs::write(tmp.path().join(".keeper/catalog/paths.tsv"), table).unwrap();
        rehash_manifest(tmp.path());
        output(&["catalog", "check"], tmp.path(), 2);
    }
}

#[test]
fn rejects_extra_metadata_and_unreferenced_objects() {
    let tmp = tempfile::tempdir().unwrap();
    output(&["init"], tmp.path(), 0);
    fs::write(tmp.path().join(".keeper/catalog/extra"), b"stray").unwrap();
    output(&["catalog", "check"], tmp.path(), 2);
    fs::remove_file(tmp.path().join(".keeper/catalog/extra")).unwrap();
    fs::write(
        tmp.path().join(".keeper/catalog/objects.tsv"),
        format!("object\tsize\nb3:{}\t0\n", blake3::hash(b"").to_hex()),
    )
    .unwrap();
    rehash_manifest(tmp.path());
    output(&["catalog", "check"], tmp.path(), 2);
}

#[test]
fn help_is_discoverable_equivalent_and_supports_nested_commands() {
    let help = |args: &[&str]| {
        cargo_bin_cmd!("keeper")
            .args(args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone()
    };
    assert_eq!(help(&["--help"]), help(&["help"]));
    let top = String::from_utf8(help(&["help"])).unwrap();
    for syntax in [
        "init [ROOT] [--resume]",
        "status [ROOT] [--resume]",
        "preflight [ROOT]",
        "compare LEFT RIGHT [--live]",
        "catalog check [ROOT]",
        "help [COMMAND]...",
    ] {
        assert!(top.contains(syntax), "missing {syntax}");
    }
    for command in ["init", "status", "preflight", "compare", "catalog"] {
        assert_eq!(help(&["help", command]), help(&[command, "--help"]));
    }
    let nested = help(&["help", "catalog", "check"]);
    assert_eq!(nested, help(&["catalog", "check", "--help"]));
    assert!(
        String::from_utf8(nested)
            .unwrap()
            .contains("Dataset file contents are NOT read")
    );
    cargo_bin_cmd!("keeper")
        .args(["help", "unknown"])
        .assert()
        .code(2);
}

#[test]
fn presentation_is_readable_rich_and_plain_when_redirected() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("a"), vec![0; 1234]).unwrap();
    fs::write(tmp.path().join("b"), vec![0; 1234]).unwrap();
    let report = output(&["init"], tmp.path(), 0);
    assert!(report.contains("2.41 KiB.\n  1 unique content object(s)"));
    assert!(report.contains("1 unique content object(s), 1.21 KiB unique content"));
    assert!(!report.contains('\x1b'));
    fs::write(tmp.path().join("a"), b"changed").unwrap();
    let colored = output(&["status", "--color", "always"], tmp.path(), 1);
    assert!(colored.contains("\x1b[1;31mCHANGED\x1b[0m"));
    let plain = output(&["status", "--color", "never"], tmp.path(), 1);
    assert!(!plain.contains('\x1b'));
    assert!(plain.contains("expected_size=1.21 KiB\tobserved_size=7 B"));
    let result = keeper(&["status"], tmp.path())
        .env("NO_COLOR", "1")
        .assert()
        .code(1)
        .get_output()
        .clone();
    assert!(!result.stdout.contains(&0x1b));
}

#[test]
fn preflight_uses_units_and_colors_both_results_and_progress() {
    let tmp = tempfile::tempdir().unwrap();
    fs::File::create(tmp.path().join("large"))
        .unwrap()
        .set_len(1_234_567)
        .unwrap();
    let plain = keeper(
        &["preflight", "--progress", "always", "--color", "never"],
        tmp.path(),
    )
    .assert()
    .success()
    .get_output()
    .clone();
    assert!(String::from_utf8_lossy(&plain.stdout).contains(
        "Preflight: 1 regular file(s), 1.18 MiB, 0 issue(s).\nContents not read; file readability and publication support not checked.\n"
    ));
    assert!(String::from_utf8_lossy(&plain.stderr).contains("inventoried=1.18 MiB"));
    assert!(!plain.stdout.contains(&0x1b));
    assert!(!plain.stderr.contains(&0x1b));
    let colored = keeper(
        &["preflight", "--progress", "always", "--color", "always"],
        tmp.path(),
    )
    .assert()
    .success()
    .get_output()
    .clone();
    assert!(String::from_utf8_lossy(&colored.stdout).contains("\x1b[1;36mPreflight\x1b[0m"));
    assert!(String::from_utf8_lossy(&colored.stdout).contains("\x1b[1m1.18 MiB\x1b[0m"));
    assert!(String::from_utf8_lossy(&colored.stderr).contains("\x1b[1;32mPreflight: completed"));
    let automatic = keeper(&["preflight", "--progress", "always"], tmp.path())
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(!automatic.stdout.contains(&0x1b));
    assert!(!automatic.stderr.contains(&0x1b));
}

#[test]
fn default_root_is_current_directory() {
    let tmp = tempfile::tempdir().unwrap();
    cargo_bin_cmd!("keeper")
        .arg("init")
        .current_dir(tmp.path())
        .assert()
        .success();
    cargo_bin_cmd!("keeper")
        .arg("status")
        .current_dir(tmp.path())
        .assert()
        .success();
}
