use assert_cmd::cargo::cargo_bin_cmd;
use std::{
    fs,
    os::unix::{
        ffi::OsStringExt,
        fs::{PermissionsExt, symlink},
    },
    path::Path,
    process::Output,
};

fn run(args: &[&str], roots: &[&Path], code: i32) -> Output {
    cargo_bin_cmd!("keeper")
        .args(args)
        .args(roots)
        .assert()
        .code(code)
        .get_output()
        .clone()
}
fn text(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}
fn err(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}
fn policy(root: &Path, contents: &[u8]) {
    fs::write(root.join(".keeperignore"), contents).unwrap();
}
fn baseline(root: &Path) -> Vec<Vec<u8>> {
    ["MANIFEST", "paths.tsv", "objects.tsv", "scope.ignore"]
        .map(|name| fs::read(root.join(".keeper/catalog").join(name)).unwrap())
        .to_vec()
}

#[test]
fn root_policy_prunes_unreadable_subtree_and_does_not_catalog_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir(root.join("lost+found")).unwrap();
    fs::write(root.join("lost+found/recovered"), b"valuable").unwrap();
    symlink("missing", root.join("lost+found/unsupported-link")).unwrap();
    fs::create_dir_all(root.join("nested/lost+found")).unwrap();
    fs::write(root.join("nested/lost+found/keep"), b"keep").unwrap();
    fs::write(root.join("data"), b"data").unwrap();
    policy(root, b"# deliberate scope\n/lost+found/\n");
    fs::set_permissions(root.join("lost+found"), fs::Permissions::from_mode(0o000)).unwrap();
    let report = text(&run(&["preflight"], &[root], 0));
    assert!(report.contains("2 regular file(s), 8 B, 0 issue(s)"));
    assert!(report.contains("1 excluded entry/subtree root(s)"));
    assert!(!report.contains("ISSUE\t"));
    assert!(!root.join(".keeper").exists());
    run(&["init", "--resume"], &[root], 0);
    let before = baseline(root);
    let manifest = fs::read_to_string(root.join(".keeper/catalog/MANIFEST")).unwrap();
    assert!(manifest.starts_with("keeper-catalog-version\t2\n"));
    assert_eq!(
        fs::read(root.join(".keeper/catalog/scope.ignore")).unwrap(),
        b"# deliberate scope\n/lost+found/\n"
    );
    let paths = fs::read_to_string(root.join(".keeper/catalog/paths.tsv")).unwrap();
    assert!(!paths.contains(".keeperignore"));
    assert!(!paths.contains("\nlost+found/"));
    assert!(paths.contains("nested/lost+found/keep"));
    run(&["catalog", "check"], &[root], 0);
    run(&["status"], &[root], 0);
    run(&["status", "--resume"], &[root], 0);
    assert_eq!(baseline(root), before);
    fs::set_permissions(root.join("lost+found"), fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        fs::read(root.join("lost+found/recovered")).unwrap(),
        b"valuable"
    );
}

#[test]
fn patterns_cover_globs_arbitrary_names_and_skip_special_objects_explicitly() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("photos/a/b")).unwrap();
    for file in [
        "photos/scratch1.jpg",
        "photos/a/b/scratch2.jpg",
        "photos/a/b/keep.jpg",
    ] {
        fs::write(root.join(file), b"a").unwrap();
    }
    fs::write(
        root.join(std::ffi::OsString::from_vec(b"bad\xff.tmp".to_vec())),
        b"a",
    )
    .unwrap();
    symlink("missing", root.join("link")).unwrap();
    policy(root, b"photos/**/scratch?.jpg\n*.tmp\n/link\n");
    let report = text(&run(&["preflight"], &[root], 0));
    assert!(report.contains("1 regular file(s), 1 B, 0 issue(s)"));
    assert!(report.contains("4 excluded entry/subtree root(s)"));
    run(&["init"], &[root], 0);
    run(&["status"], &[root], 0);
}

#[test]
fn changed_or_removed_policy_cannot_hide_baselined_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("keep"), b"data").unwrap();
    policy(root, b"# original\n");
    run(&["init"], &[root], 0);
    let before = baseline(root);
    for source in [b"/keep\n".as_slice(), b"# comment changed\n", b""] {
        policy(root, source);
        for args in [
            vec!["status", "--progress", "always"],
            vec!["status", "--resume", "--progress", "always"],
        ] {
            let result = run(&args, &[root], 2);
            assert!(err(&result).contains("differs from recorded catalog scope"));
            assert!(!err(&result).contains("Hashing"));
        }
        assert_eq!(baseline(root), before);
    }
    fs::remove_file(root.join(".keeperignore")).unwrap();
    run(&["status"], &[root], 2);
    // Catalog checks only the recorded policy, not today's configuration.
    run(&["catalog", "check"], &[root], 0);
    policy(root, b"# original\n");
    run(&["status"], &[root], 0);
}

#[test]
fn legacy_catalogs_cannot_be_silently_restricted_by_a_new_policy() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("keep"), b"a").unwrap();
    run(&["init"], &[temp.path()], 0);
    assert!(
        fs::read_to_string(temp.path().join(".keeper/catalog/MANIFEST"))
            .unwrap()
            .starts_with("keeper-catalog-version\t1\n")
    );
    policy(temp.path(), b"/keep\n");
    run(&["status"], &[temp.path()], 2);
    run(&["catalog", "check"], &[temp.path()], 0);
}

#[test]
fn comparison_modes_report_scope_and_refuse_equal_claims_across_policies() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    for root in [left.path(), right.path()] {
        fs::write(root.join("data"), b"same").unwrap();
        fs::create_dir(root.join("lost+found")).unwrap();
        fs::write(
            root.join("lost+found/different"),
            root.as_os_str().as_encoded_bytes(),
        )
        .unwrap();
        policy(root, b"/lost+found/\n");
        run(&["init"], &[root], 0);
    }
    for mode in [
        vec!["compare"],
        vec!["compare", "--live"],
        vec!["compare", "--metadata-only"],
    ] {
        let report = text(&run(&mode, &[left.path(), right.path()], 0));
        assert!(report.contains("Excluded paths are outside this check"));
        assert!(report.contains("patterns: /lost+found/"));
    }
    policy(
        right.path(),
        b"# different policy, same effective rules\n/lost+found/\n",
    );
    for mode in [
        vec!["compare", "--live"],
        vec!["compare", "--metadata-only"],
    ] {
        let report = text(&run(&mode, &[left.path(), right.path()], 2));
        assert!(report.contains("SCOPE MISMATCH"));
        assert!(report.contains("policies differ"));
    }
    // Catalog comparison still uses the saved, equal policies, not current files.
    run(&["compare"], &[left.path(), right.path()], 0);
    let third = tempfile::tempdir().unwrap();
    fs::write(third.path().join("data"), b"same").unwrap();
    run(&["init"], &[third.path()], 0);
    assert!(text(&run(&["compare"], &[left.path(), third.path()], 2)).contains("SCOPE MISMATCH"));
}

#[test]
fn invalid_policy_and_nonregular_configuration_fail_before_hashing() {
    let temp = tempfile::tempdir().unwrap();
    for source in [b"!keep".as_slice(), b"[abc]", b"a\\b", b"\xff", b"a**b"] {
        policy(temp.path(), source);
        let result = run(&["init", "--progress", "always"], &[temp.path()], 2);
        assert!(!err(&result).contains("Hashing"));
        assert!(!temp.path().join(".keeper/catalog").exists());
    }
    fs::remove_file(temp.path().join(".keeperignore")).unwrap();
    symlink("missing", temp.path().join(".keeperignore")).unwrap();
    run(&["preflight"], &[temp.path()], 2);
    fs::remove_file(temp.path().join(".keeperignore")).unwrap();
    fs::create_dir(temp.path().join(".keeperignore")).unwrap();
    run(&["preflight"], &[temp.path()], 2);
}

#[test]
fn saved_policy_cannot_contradict_its_path_table_even_with_updated_checksums() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("data"), b"a").unwrap();
    policy(temp.path(), b"# original\n");
    run(&["init"], &[temp.path()], 0);
    let dir = temp.path().join(".keeper/catalog");
    let manifest = fs::read_to_string(dir.join("MANIFEST")).unwrap();
    fs::write(dir.join("scope.ignore"), b"/data\n").unwrap();
    fs::write(
        dir.join("MANIFEST"),
        manifest.replace(
            blake3::hash(b"# original\n").to_hex().as_str(),
            blake3::hash(b"/data\n").to_hex().as_str(),
        ),
    )
    .unwrap();
    let result = run(&["catalog", "check"], &[temp.path()], 2);
    assert!(err(&result).contains("path excluded by its recorded scope policy"));
}

#[test]
fn nested_ignore_is_data_and_root_metadata_cannot_be_hidden() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("nested")).unwrap();
    fs::write(
        temp.path().join("nested/.keeperignore"),
        b"!unsupported but ordinary data",
    )
    .unwrap();
    policy(temp.path(), b"/.keeper/\n");
    run(&["init"], &[temp.path()], 0);
    let paths = fs::read_to_string(temp.path().join(".keeper/catalog/paths.tsv")).unwrap();
    assert!(paths.contains("nested/.keeperignore"));
    let other = tempfile::tempdir().unwrap();
    policy(other.path(), b"/.keeper/\n");
    symlink("missing", other.path().join(".keeper")).unwrap();
    let result = run(&["preflight"], &[other.path()], 2);
    assert!(text(&result).contains("reserved .keeper must be a real directory"));
}

#[test]
fn saved_scope_is_checksummed_and_required() {
    let temp = tempfile::tempdir().unwrap();
    policy(temp.path(), b"/lost+found/\n");
    run(&["init"], &[temp.path()], 0);
    fs::write(
        temp.path().join(".keeper/catalog/scope.ignore"),
        b"# changed\n/lost+found/\n",
    )
    .unwrap();
    run(&["catalog", "check"], &[temp.path()], 2);
    fs::remove_file(temp.path().join(".keeper/catalog/scope.ignore")).unwrap();
    run(&["catalog", "check"], &[temp.path()], 2);
}
