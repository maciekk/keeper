use std::{
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn command(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_keeper"))
        .args(["--progress", "never"])
        .args(args)
        .output()
        .unwrap()
}
fn run(args: &[&str], code: i32) -> String {
    let o = command(args);
    assert_eq!(
        o.status.code(),
        Some(code),
        "{args:?}\n{}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}
fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}
fn setup() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join("old"), b"original").unwrap();
    run(&["init", p(t.path())], 0);
    run(&["catalog", "migrate", p(t.path())], 0);
    t
}
fn plan(root: &Path, destination: &Path, options: &[&str]) {
    let mut args = vec!["accept", "plan", p(root), p(destination)];
    args.extend_from_slice(options);
    run(&args, 0);
}
fn current(root: &Path) -> Vec<u8> {
    fs::read(root.join(".keeper/intent/CURRENT")).unwrap()
}
fn revision(root: &Path, n: u64) -> PathBuf {
    root.join(format!(".keeper/intent/revisions/{n}"))
}

#[test]
fn migration_is_lossless_metadata_only_and_preserves_scoped_and_legacy_formats() {
    for scoped in [false, true] {
        let t = tempfile::tempdir().unwrap();
        if scoped {
            fs::write(
                t.path().join(".keeperignore"),
                b"# exact scope\nexcluded/\n",
            )
            .unwrap();
        }
        fs::write(t.path().join("old"), b"before").unwrap();
        run(&["init", p(t.path())], 0);
        let before: Vec<_> = ["MANIFEST", "paths.tsv", "objects.tsv"]
            .map(|n| fs::read(t.path().join(".keeper/catalog").join(n)).unwrap())
            .into();
        // Migration must not require even the existence of inherited bytes.
        fs::remove_file(t.path().join("old")).unwrap();
        run(&["catalog", "migrate", p(t.path())], 0);
        assert!(!t.path().join(".keeper/catalog").exists());
        for (i, name) in ["MANIFEST", "paths.tsv", "objects.tsv"].iter().enumerate() {
            assert_eq!(
                fs::read(t.path().join(".keeper/legacy-catalog").join(name)).unwrap(),
                before[i]
            );
            if *name != "MANIFEST" {
                assert_eq!(
                    fs::read(revision(t.path(), 1).join(name)).unwrap(),
                    before[i]
                );
            }
        }
        if scoped {
            assert_eq!(
                fs::read(revision(t.path(), 1).join("scope.ignore")).unwrap(),
                b"# exact scope\nexcluded/\n"
            );
        }
        run(&["catalog", "check", p(t.path())], 0);
        assert!(run(&["status", p(t.path())], 1).contains("MISSING\told"));
        assert!(run(&["init", p(t.path())], 0).contains("Baseline left unchanged"));
        run(&["catalog", "migrate", p(t.path())], 2);
    }
}

#[test]
fn accepting_additions_defers_changed_and_missing_old_files_without_reading_them() {
    let t = setup();
    let root = t.path();
    let reports = tempfile::tempdir().unwrap();
    let saved = reports.path().join("plan");
    fs::write(root.join("old"), b"unexpected change").unwrap();
    fs::write(root.join("new"), b"approved").unwrap();
    fs::write(root.join("deferred"), b"not selected").unwrap();
    let r1 = fs::read(revision(root, 1).join("MANIFEST")).unwrap();
    plan(root, &saved, &["--add", "new"]);
    let output = run(&["accept", "show", p(&saved)], 0);
    assert!(output.contains("ADD\tnew") && output.contains("new: b3:"));
    // Replacing an inherited file with an unsupported object must not affect acceptance.
    fs::remove_file(root.join("old")).unwrap();
    symlink("missing-target", root.join("old")).unwrap();
    run(&["accept", "apply", p(root), p(&saved)], 0);
    assert_eq!(fs::read(revision(root, 1).join("MANIFEST")).unwrap(), r1);
    fs::remove_file(root.join("old")).unwrap();
    let status = run(&["status", p(root)], 1);
    assert!(status.contains("MISSING\told") && status.contains("ADDED\tdeferred"));
    assert!(!status.contains("ADDED\tnew"));
    fs::write(root.join("old"), b"unexpected change").unwrap();
    assert!(run(&["status", p(root)], 1).contains("CHANGED\told"));
    run(&["accept", "apply", p(root), p(&saved)], 2); // exact-parent, no repeat/rebase
}

#[test]
fn replace_remove_and_explicit_rename_preserve_history() {
    let t = setup();
    let r = t.path();
    let plans = tempfile::tempdir().unwrap();
    fs::write(r.join("old"), b"replacement").unwrap();
    let replace = plans.path().join("replace");
    plan(r, &replace, &["--replace", "old"]);
    run(&["accept", "apply", p(r), p(&replace)], 0);
    run(&["status", p(r)], 0);
    fs::rename(r.join("old"), r.join("renamed")).unwrap();
    let rename = plans.path().join("rename");
    plan(r, &rename, &["--remove", "old", "--add", "renamed"]);
    run(&["accept", "apply", p(r), p(&rename)], 0);
    run(&["status", p(r)], 0);
    for n in 1..=3 {
        assert!(revision(r, n).is_dir());
    }
    let text = fs::read_to_string(revision(r, 3).join("MANIFEST")).unwrap();
    assert!(text.contains("parent\t2\n"));
    // Historical metadata is checksum-bound by its child, not merely canonical.
    let original = revision(r, 1).join("MANIFEST");
    let old = fs::read_to_string(&original).unwrap();
    let mutated = old
        .lines()
        .map(|line| {
            if line.starts_with("recorded-unix-ns\t") {
                "recorded-unix-ns\t1"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(original, mutated).unwrap();
    run(&["catalog", "check", p(r)], 2);
}

#[test]
fn stale_bytes_absences_roots_and_plans_fail_all_or_nothing() {
    let t = setup();
    let r = t.path();
    let plans = tempfile::tempdir().unwrap();
    fs::write(r.join("a"), b"aaa").unwrap();
    fs::write(r.join("b"), b"bbb").unwrap();
    let saved = plans.path().join("plan");
    plan(r, &saved, &["--add", "a", "--add", "b"]);
    let before = current(r);
    fs::write(r.join("b"), b"ccc").unwrap(); // same size, different bytes
    run(&["accept", "apply", p(r), p(&saved)], 2);
    assert_eq!(before, current(r));
    let other = setup();
    run(&["accept", "apply", p(other.path()), p(&saved)], 2);
    assert_eq!(before, current(r));
    let mut text = fs::read(&saved).unwrap();
    text[0] = b'X';
    fs::write(&saved, text).unwrap();
    run(&["accept", "apply", p(r), p(&saved)], 2);
    fs::remove_file(r.join("old")).unwrap();
    let removal = plans.path().join("remove");
    plan(r, &removal, &["--remove", "old"]);
    symlink("missing", r.join("old")).unwrap(); // dangling link is not absence
    run(&["accept", "apply", p(r), p(&removal)], 2);
    assert_eq!(before, current(r));
}

#[test]
fn no_implicit_changes_scope_bypasses_unsafe_paths_or_symlinks() {
    let t = setup();
    let r = t.path();
    let plans = tempfile::tempdir().unwrap();
    fs::write(r.join("new"), b"new").unwrap();
    symlink(plans.path(), r.join("link")).unwrap();
    fs::write(plans.path().join("outside"), b"private").unwrap();
    for options in [
        vec![],
        vec!["--add", "old"],
        vec!["--replace", "new"],
        vec!["--remove", "old"],
        vec!["--add", "new", "--add", "new"],
        vec!["--add", "../outside"],
        vec!["--add", "link/outside"],
        vec!["--add", ".keeper/LOCK"],
        vec!["--add", ".keeperignore"],
    ] {
        let dest = plans.path().join("rejected");
        let mut args = vec!["accept", "plan", p(r), p(&dest)];
        args.extend(options);
        run(&args, 2);
        assert!(!dest.exists());
    }
    let saved = plans.path().join("saved");
    plan(r, &saved, &["--add", "new"]);
    fs::write(r.join(".keeperignore"), b"new\n").unwrap();
    run(&["accept", "apply", p(r), p(&saved)], 2);
    assert!(!revision(r, 2).exists());
}

#[test]
fn file_ancestor_conflicts_and_removals_of_existing_directories_fail() {
    let t = setup();
    let r = t.path();
    let plans = tempfile::tempdir().unwrap();
    fs::remove_file(r.join("old")).unwrap();
    fs::create_dir(r.join("old")).unwrap();
    fs::write(r.join("old/child"), b"child").unwrap();
    let saved = plans.path().join("plan");
    run(
        &["accept", "plan", p(r), p(&saved), "--add", "old/child"],
        2,
    );
    // A removal authorizes absence of the path, not replacing it with a directory.
    run(
        &[
            "accept",
            "plan",
            p(r),
            p(&saved),
            "--remove",
            "old",
            "--add",
            "old/child",
        ],
        2,
    );
}

#[test]
fn current_is_authority_orphans_are_not_recovered_implicitly_and_exports_are_offline() {
    let t = setup();
    let r = t.path();
    let exports = tempfile::tempdir().unwrap();
    fs::create_dir(revision(r, 99)).unwrap(); // must not become authoritative
    run(&["catalog", "check", p(r)], 0);
    fs::write(r.join("new"), b"new").unwrap();
    let saved = exports.path().join("plan");
    plan(r, &saved, &["--add", "new"]);
    run(&["accept", "apply", p(r), p(&saved)], 0);
    assert!(revision(r, 100).is_dir());
    let a = exports.path().join("a");
    let b = exports.path().join("b");
    run(&["catalog", "export", p(r), p(&a)], 0);
    run(&["catalog", "export", p(r), p(&b)], 0);
    run(&["catalog", "export", p(r), p(&a)], 2);
    drop(t);
    run(&["compare", "--catalogs", p(&a), p(&b)], 0);
    assert!(
        fs::read_to_string(a.join("MANIFEST"))
            .unwrap()
            .contains("revision\t100\n")
    );
}

#[test]
fn corrupt_or_missing_references_never_fall_back_to_legacy_and_unsafe_members_fail() {
    for damage in [
        "missing",
        "corrupt",
        "symlink-current",
        "symlink-revision",
        "symlink-table",
        "bad-parent",
    ] {
        let t = setup();
        let r = t.path();
        let reference = r.join(".keeper/intent/CURRENT");
        match damage {
            "missing" => fs::remove_file(&reference).unwrap(),
            "corrupt" => fs::write(&reference, b"keeper-current-1\t99\tb3:bad\n").unwrap(),
            "symlink-current" => {
                fs::rename(&reference, r.join("saved")).unwrap();
                symlink(r.join("saved"), &reference).unwrap();
            }
            "symlink-revision" => {
                fs::rename(revision(r, 1), r.join("saved")).unwrap();
                symlink(r.join("saved"), revision(r, 1)).unwrap();
            }
            "symlink-table" => {
                let paths = revision(r, 1).join("paths.tsv");
                fs::rename(&paths, r.join("saved")).unwrap();
                symlink(r.join("saved"), paths).unwrap();
            }
            "bad-parent" => {
                let manifest = revision(r, 1).join("MANIFEST");
                let text = fs::read_to_string(&manifest)
                    .unwrap()
                    .replace("parent\t0", "parent\t1");
                fs::write(manifest, text).unwrap();
            }
            _ => unreachable!(),
        }
        run(&["catalog", "check", p(r)], 2);
        run(&["init", p(r)], 2);
        assert!(!r.join(".keeper/catalog").exists());
    }
}

#[test]
fn scoped_acceptance_preserves_exact_policy_and_byte_escaped_paths() {
    use std::os::unix::ffi::OsStringExt;
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    let plans = tempfile::tempdir().unwrap();
    fs::write(r.join(".keeperignore"), b"# preserved exactly\nexcluded/\n").unwrap();
    run(&["init", p(r)], 0);
    run(&["catalog", "migrate", p(r)], 0);
    let name = std::ffi::OsString::from_vec(b"odd\\\t\n\xff".to_vec());
    fs::write(r.join(&name), b"new").unwrap();
    let saved = plans.path().join("plan");
    let o = Command::new(env!("CARGO_BIN_EXE_keeper"))
        .args(["--progress", "never", "accept", "plan"])
        .arg(r)
        .arg(&saved)
        .arg("--add")
        .arg(&name)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let show = run(&["accept", "show", p(&saved)], 0);
    assert!(show.contains("\\xff"));
    run(&["accept", "apply", p(r), p(&saved)], 0);
    assert_eq!(
        fs::read(revision(r, 2).join("scope.ignore")).unwrap(),
        b"# preserved exactly\nexcluded/\n"
    );
    run(&["status", p(r)], 0);
    fs::create_dir(r.join("excluded")).unwrap();
    fs::write(r.join("excluded/file"), b"outside scope").unwrap();
    run(
        &[
            "accept",
            "plan",
            p(r),
            p(&plans.path().join("excluded-plan")),
            "--add",
            "excluded/file",
        ],
        2,
    );
}

#[test]
fn writers_are_excluded_and_permission_errors_are_not_removals() {
    use fs2::FileExt;
    use std::os::unix::fs::PermissionsExt;
    let t = setup();
    let r = t.path();
    let plans = tempfile::tempdir().unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(r.join(".keeper/LOCK"))
        .unwrap();
    lock.try_lock_exclusive().unwrap();
    fs::write(r.join("new"), b"new").unwrap();
    let saved = plans.path().join("plan");
    run(&["accept", "plan", p(r), p(&saved), "--add", "new"], 2);
    drop(lock);
    plan(r, &saved, &["--add", "new"]);
    let lock = fs::File::open(r.join(".keeper/LOCK")).unwrap();
    lock.try_lock_exclusive().unwrap();
    run(&["accept", "apply", p(r), p(&saved)], 2);
    drop(lock);
    if unsafe { libc::geteuid() } != 0 {
        fs::create_dir(r.join("private")).unwrap();
        fs::set_permissions(r.join("private"), fs::Permissions::from_mode(0o0)).unwrap();
        run(
            &[
                "accept",
                "plan",
                p(r),
                p(&plans.path().join("denied")),
                "--add",
                "private/file",
            ],
            2,
        );
        fs::set_permissions(r.join("private"), fs::Permissions::from_mode(0o700)).unwrap();
    }
    assert!(!revision(r, 2).exists());
}

#[test]
fn cancelling_selected_reads_does_not_publish_a_plan_or_intent() {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    let t = setup();
    let r = t.path();
    let plans = tempfile::tempdir().unwrap();
    fs::File::create(r.join("large"))
        .unwrap()
        .set_len(2 * 1024 * 1024 * 1024)
        .unwrap();
    let saved = plans.path().join("cancelled");
    let before = current(r);
    let log = plans.path().join("progress");
    let mut child = Command::new(env!("CARGO_BIN_EXE_keeper"))
        .args(["--progress", "always", "accept", "plan"])
        .arg(r)
        .arg(&saved)
        .args(["--add", "large"])
        .stdout(Stdio::null())
        .stderr(Stdio::from(fs::File::create(&log).unwrap()))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if fs::read_to_string(&log)
            .unwrap()
            .contains("Review selected files: started")
        {
            break;
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "child exited before selected reads: {}",
            fs::read_to_string(&log).unwrap()
        );
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("no selected reads observed");
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    assert_eq!(child.wait_with_output().unwrap().status.code(), Some(2));
    assert!(!saved.exists());
    assert_eq!(current(r), before);
}

#[test]
fn acceptance_help_supports_nested_paths() {
    for args in [
        vec!["accept", "--help"],
        vec!["help", "accept", "plan"],
        vec!["help", "accept", "show"],
        vec!["help", "accept", "apply"],
        vec!["help", "catalog", "migrate"],
    ] {
        run(&args, 0);
    }
}
