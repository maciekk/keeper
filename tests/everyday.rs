use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}
fn output(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_keeper"));
    command.args(["--progress", "never"]);
    if args.first() == Some(&"compare") || args.starts_with(&["catalog", "export"]) {
        command.current_dir(root);
    } else {
        command.arg("--root").arg(root);
    }
    command.args(args).stdin(Stdio::null()).output().unwrap()
}
fn run(root: &Path, args: &[&str], code: i32) -> String {
    let o = output(root, args);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(o.status.code(), Some(code), "{args:?}: {text}");
    text
}
fn setup() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join("old"), b"original").unwrap();
    run(t.path(), &["init"], 0);
    t
}
fn active(root: &Path) -> Vec<u8> {
    fs::read(root.join(".keeper/intent/CURRENT")).unwrap()
}

#[test]
fn bulk_add_is_one_command_and_never_accepts_existing_edits_or_deletions() {
    let t = setup();
    let r = t.path();
    let original = fs::read(r.join(".keeper/catalog/MANIFEST")).unwrap();
    fs::write(r.join("old"), b"unexpected corruption").unwrap();
    fs::create_dir(r.join("shoot")).unwrap();
    for n in 0..100 {
        fs::write(r.join(format!("shoot/{n}.jpg")), b"photo").unwrap();
    }
    let result = run(r, &["add", "--yes"], 0);
    assert!(result.contains("100 added, 0 updated, 0 removed"));
    assert_eq!(
        fs::read(r.join(".keeper/legacy-catalog/MANIFEST")).unwrap(),
        original
    );
    assert!(run(r, &["status"], 1).contains("CHANGED\told"));
    let before = active(r);
    run(r, &["add", "old", "--yes"], 2);
    run(r, &["add", "shoot", "--yes"], 0); // no-op
    assert_eq!(active(r), before);
}

#[test]
fn deletion_selection_is_prefix_bounded_and_never_deletes_live_files() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    for dir in ["trip", "trip-old"] {
        fs::create_dir(r.join(dir)).unwrap();
        fs::write(r.join(dir).join("photo"), b"photo").unwrap();
    }
    run(r, &["init"], 0);
    fs::remove_dir_all(r.join("trip")).unwrap();
    fs::remove_dir_all(r.join("trip-old")).unwrap();
    run(r, &["rm", "trip", "--yes"], 0);
    let status = run(r, &["status"], 1);
    assert!(status.contains("MISSING\ttrip-old/photo"));
    assert!(!status.contains("MISSING\ttrip/photo"));
    fs::create_dir(r.join("trip-old")).unwrap();
    fs::write(r.join("trip-old/photo"), b"photo").unwrap();
    let before = active(r);
    run(r, &["rm", "trip-old/photo", "--yes"], 2);
    assert!(r.join("trip-old/photo").exists());
    assert_eq!(active(r), before);
    fs::remove_file(r.join("trip-old/photo")).unwrap();
    run(r, &["rm", "--yes"], 0);
    run(r, &["status"], 0);
}

#[test]
fn update_only_named_files_and_rename_requires_unchanged_contents() {
    let t = setup();
    let r = t.path();
    fs::write(r.join("old"), b"edited").unwrap();
    run(r, &["update"], 2);
    run(r, &["update", ".", "--yes"], 2);
    fs::write(r.join("new"), b"new").unwrap();
    run(r, &["update", "new", "--yes"], 2);
    run(r, &["update", "old", "old", "--yes"], 0);
    fs::rename(r.join("old"), r.join("renamed")).unwrap();
    fs::write(r.join("renamed"), b"changed during rename").unwrap();
    let before = active(r);
    run(r, &["mv", "old", "renamed", "--yes"], 2);
    assert_eq!(active(r), before);
    fs::write(r.join("renamed"), b"edited").unwrap();
    let result = run(r, &["mv", "old", "renamed", "--yes"], 0);
    assert!(result.contains("REMOVE\told") && result.contains("ADD\trenamed"));
    assert!(!r.join("old").exists());
    assert_eq!(fs::read(r.join("renamed")).unwrap(), b"edited");
    run(r, &["add", "--yes"], 0);
    run(r, &["status"], 0);
}

#[test]
fn preview_noop_and_noninteractive_rejection_do_not_migrate_or_create_lock() {
    let t = setup();
    let r = t.path();
    fs::remove_file(r.join(".keeper/LOCK")).unwrap();
    run(r, &["add", "--yes"], 0);
    assert!(!r.join(".keeper/LOCK").exists());
    fs::write(r.join("new"), b"new").unwrap();
    run(r, &["add"], 2);
    let result = run(r, &["add", "--dry-run", "--long"], 0);
    assert!(result.contains("ADD\tnew") && result.contains("new: b3:"));
    run(r, &["snap", "-m", "preview", "--dry-run"], 0);
    assert!(!r.join(".keeper/intent").exists());
    assert!(!r.join(".keeper/LOCK").exists());
    assert!(r.join(".keeper/catalog").exists());
}

#[test]
fn snapshots_accept_mixed_changes_and_markers_preserve_current_expectations() {
    let t = setup();
    let r = t.path();
    fs::write(r.join("new"), b"new").unwrap();
    let marker = run(r, &["snap", "--mark", "-m", "Before editing", "--yes"], 0);
    assert!(marker.contains("live files and changes were NOT checked"));
    assert!(run(r, &["status"], 1).contains("ADDED\tnew"));
    let before = fs::read(r.join(".keeper/intent/revisions/2/paths.tsv")).unwrap();
    fs::write(r.join("old"), b"edited").unwrap();
    run(r, &["snap", "-m", "Session finished", "--yes"], 0);
    run(r, &["status"], 0);
    assert_eq!(
        fs::read(r.join(".keeper/intent/revisions/2/paths.tsv")).unwrap(),
        before
    );
    let prior = active(r);
    run(r, &["snap", "-m", "Another stopping point", "--yes"], 0);
    assert_ne!(prior, active(r));
    let log = run(r, &["log"], 0);
    assert!(
        log.contains("Before editing")
            && log.contains("Session finished")
            && log.contains("Another stopping point")
    );
    fs::remove_file(r.join("old")).unwrap();
    fs::write(r.join("new"), b"another edit").unwrap();
    fs::write(r.join("third"), b"third").unwrap();
    let result = run(r, &["snap", "-m", "Mixed", "--yes"], 0);
    assert!(result.contains("1 added, 1 updated, 1 removed"));
    run(r, &["status"], 0);
}

#[test]
fn snapshot_scope_message_encoding_export_and_schema_compatibility() {
    let t = setup();
    let r = t.path();
    let exports = tempfile::tempdir().unwrap();
    fs::write(r.join("a"), b"a").unwrap();
    fs::write(r.join("b"), b"b").unwrap();
    run(
        r,
        &["snap", "a", "-m", "Line one\nLine two\t\\", "--yes"],
        0,
    );
    let status = run(r, &["status"], 1);
    assert!(status.contains("ADDED\tb"));
    assert!(!status.contains("ADDED\ta"));
    let log = run(r, &["log"], 0);
    assert!(log.contains("Line one\\nLine two\\t\\\\"));
    let a = exports.path().join("a");
    let b = exports.path().join("b");
    run(r, &["catalog", "export", ".", p(&a)], 0);
    run(r, &["catalog", "export", ".", p(&b)], 0);
    run(r, &["compare", "--catalogs", p(&a), p(&b)], 0);
    run(r, &["snap", "--mark", "old", "-m", "bad", "--yes"], 2);
    run(r, &["snap", "-m", "  ", "--yes"], 2);
    // Older intent manifests are still readable as ancestors of named snapshots.
    assert!(
        fs::read_to_string(r.join(".keeper/intent/revisions/1/MANIFEST"))
            .unwrap()
            .starts_with("keeper-intent-version\t1\n")
    );
    assert!(
        fs::read_to_string(r.join(".keeper/intent/revisions/2/MANIFEST"))
            .unwrap()
            .starts_with("keeper-intent-version\t2\n")
    );
}

#[test]
fn selectors_overlap_and_scope_are_safe_and_explicit_roots_do_not_search_upward() {
    use std::os::unix::fs::symlink;
    let t = setup();
    let r = t.path();
    let outside = tempfile::tempdir().unwrap();
    fs::create_dir(r.join("album")).unwrap();
    fs::write(r.join("album/photo"), b"photo").unwrap();
    run(r, &["add", "./album/", "album/photo", "--yes"], 0);
    fs::write(r.join("-odd"), b"odd").unwrap();
    run(r, &["add", "--yes", "--", "-odd"], 0);
    run(r, &["add", "../outside", "--yes"], 2);
    run(r, &["add", "unknown", "--yes"], 2);
    run(r, &["rm", "unknown", "--yes"], 2);
    symlink(outside.path(), r.join("link")).unwrap();
    fs::write(outside.path().join("private"), b"private").unwrap();
    run(r, &["add", "link/private", "--yes"], 2);
    run(&r.join("album"), &["add", "--yes"], 2);
    fs::write(r.join(".keeperignore"), b"album/\n").unwrap();
    run(r, &["add", "--yes"], 2);
}

#[test]
fn inherited_unsupported_files_do_not_get_read_by_add_or_mark() {
    use std::os::unix::fs::symlink;
    let t = setup();
    let r = t.path();
    fs::remove_file(r.join("old")).unwrap();
    symlink("absent", r.join("old")).unwrap();
    fs::write(r.join("new"), b"new").unwrap();
    run(r, &["add", "--yes"], 0);
    run(
        r,
        &["snap", "--mark", "-m", "Expectations only", "--yes"],
        0,
    );
    run(r, &["snap", "-m", "Must not accept symlink", "--yes"], 2);
    run(r, &["rm", "old", "--yes"], 2);
}

// A PTY for stdin lets the real CLI exercise interactive confirmation while
// stdout is a polled file. No production-only fault injection is needed.
struct Prompt {
    child: std::process::Child,
    input: fs::File,
    log: std::path::PathBuf,
    _temp: tempfile::TempDir,
}
impl Prompt {
    fn start(root: &Path, args: &[&str]) -> Self {
        use std::os::fd::FromRawFd;
        let (mut master, mut slave) = (-1, -1);
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let input = unsafe { fs::File::from_raw_fd(master) };
        let slave = unsafe { fs::File::from_raw_fd(slave) };
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("output");
        let child = Command::new(env!("CARGO_BIN_EXE_keeper"))
            .args(["--progress", "never", "--root"])
            .arg(root)
            .args(args)
            .stdin(Stdio::from(slave))
            .stdout(Stdio::from(fs::File::create(&log).unwrap()))
            .stderr(Stdio::from(
                fs::OpenOptions::new().append(true).open(&log).unwrap(),
            ))
            .spawn()
            .unwrap();
        let mut prompt = Self {
            child,
            input,
            log,
            _temp: temp,
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if fs::read_to_string(&prompt.log).unwrap().contains("[y/N]") {
                break;
            }
            assert!(
                prompt.child.try_wait().unwrap().is_none(),
                "{}",
                fs::read_to_string(&prompt.log).unwrap()
            );
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for prompt"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        prompt
    }
    fn answer(&mut self, answer: &[u8], code: i32) -> String {
        self.input.write_all(answer).unwrap();
        assert_eq!(
            self.child.wait().unwrap().code(),
            Some(code),
            "{}",
            fs::read_to_string(&self.log).unwrap()
        );
        fs::read_to_string(&self.log).unwrap()
    }
}
impl Drop for Prompt {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn interactive_decline_and_prompt_time_byte_changes_do_not_migrate() {
    let t = setup();
    let r = t.path();
    fs::remove_file(r.join(".keeper/LOCK")).unwrap();
    fs::write(r.join("new"), b"reviewed").unwrap();
    let mut prompt = Prompt::start(r, &["add"]);
    prompt.answer(b"n\n", 0);
    assert!(!r.join(".keeper/LOCK").exists());
    assert!(!r.join(".keeper/intent").exists());
    let mut prompt = Prompt::start(r, &["add"]);
    fs::write(r.join("new"), b"unapproved").unwrap();
    prompt.answer(b"y\n", 2);
    assert!(!r.join(".keeper/intent").exists());
}

#[test]
fn interactive_batch_does_not_expand_and_changed_parent_rejects_the_whole_batch() {
    let t = setup();
    let r = t.path();
    fs::write(r.join("a"), b"a").unwrap();
    let mut prompt = Prompt::start(r, &["add"]);
    fs::write(r.join("b"), b"b").unwrap();
    prompt.answer(b"yes\n", 0);
    let status = run(r, &["status"], 1);
    assert!(status.contains("ADDED\tb") && !status.contains("ADDED\ta"));
    let mut prompt = Prompt::start(r, &["add"]);
    run(r, &["snap", "--mark", "-m", "New parent", "--yes"], 0);
    let before = active(r);
    prompt.answer(b"y\n", 2);
    assert_eq!(before, active(r));
}

#[test]
fn prompt_cancellation_and_removal_reappearance_leave_expectations_unchanged() {
    let t = setup();
    let r = t.path();
    fs::write(r.join("new"), b"new").unwrap();
    let mut prompt = Prompt::start(r, &["add"]);
    assert_eq!(
        unsafe { libc::kill(prompt.child.id() as i32, libc::SIGTERM) },
        0
    );
    assert_eq!(prompt.child.wait().unwrap().code(), Some(2));
    assert!(!r.join(".keeper/intent").exists());
    fs::remove_file(r.join("old")).unwrap();
    let mut prompt = Prompt::start(r, &["rm"]);
    fs::write(r.join("old"), b"reappeared").unwrap();
    prompt.answer(b"y\n", 2);
    assert!(!r.join(".keeper/intent").exists());
}

#[test]
fn corrupt_named_messages_scopes_and_unreadable_absences_fail_closed() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    fs::write(r.join(".keeperignore"), b"excluded/\n").unwrap();
    fs::create_dir(r.join("private")).unwrap();
    fs::write(r.join("private/photo"), b"photo").unwrap();
    run(r, &["init"], 0);
    fs::create_dir(r.join("excluded")).unwrap();
    fs::write(r.join("excluded/photo"), b"ignored").unwrap();
    run(r, &["add", "excluded", "--yes"], 2);
    run(r, &["add", "--yes"], 0);
    assert!(!r.join(".keeper/intent").exists());
    if unsafe { libc::geteuid() } != 0 {
        fs::remove_file(r.join("private/photo")).unwrap();
        fs::set_permissions(r.join("private"), fs::Permissions::from_mode(0o0)).unwrap();
        run(r, &["rm", "--yes"], 2);
        fs::set_permissions(r.join("private"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(r.join("private/photo"), b"photo").unwrap();
    }
    run(r, &["snap", "--mark", "-m", "Original message", "--yes"], 0);
    let manifest = r.join(".keeper/intent/revisions/2/MANIFEST");
    let text = fs::read_to_string(&manifest)
        .unwrap()
        .replace("Original message", "Corrupted message");
    fs::write(manifest, text).unwrap();
    run(r, &["catalog", "check"], 2);
    run(r, &["log"], 2);
    run(r, &["add", "--yes"], 2); // never fall back to legacy metadata
}

#[test]
fn root_is_a_dataset_selector_not_a_working_directory_change() {
    let caller = tempfile::tempdir().unwrap();
    let dataset = caller.path().join("data");
    fs::create_dir(&dataset).unwrap();
    fs::write(dataset.join("old"), b"original").unwrap();
    let invoke = |args: &[&str], code: i32| {
        let o = Command::new(env!("CARGO_BIN_EXE_keeper"))
            .current_dir(caller.path())
            .args(["--progress", "never"])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        assert_eq!(o.status.code(), Some(code), "{args:?}: {text}");
        text
    };
    invoke(&["--root", "data", "init"], 0);
    fs::write(dataset.join("new"), b"dataset file").unwrap();
    fs::write(caller.path().join("new"), b"unrelated shell file").unwrap();
    invoke(&["--root", "data", "add", "new", "--yes"], 0);
    invoke(&["status", "--root", "data"], 0);
    assert_eq!(
        fs::read(caller.path().join("new")).unwrap(),
        b"unrelated shell file"
    );
    assert!(!caller.path().join(".keeper").exists());
    invoke(&["--root", "data", "status", "."], 2);
    invoke(&["--root", "data", "catalog", "check", "data"], 2);
    invoke(&["--root", "data", "compare", "data", "other"], 2);
    invoke(
        &["--root", "data", "catalog", "export", "data", "exported"],
        2,
    );
    assert!(!caller.path().join("exported").exists());
    // Legacy commands keep shell-relative inputs/outputs, not dataset-relative ones.
    invoke(&["catalog", "export", "data", "exported"], 0);
    assert!(caller.path().join("exported/MANIFEST").is_file());
    assert!(!dataset.join("exported").exists());
    invoke(&["-C", "data", "status"], 2);
}

#[test]
fn global_root_option_and_nested_help_are_consistent() {
    let t = setup();
    let r = t.path();
    run(r, &["catalog", "check"], 0);
    run(r, &["help", "snap"], 0);
    for cmd in ["add", "rm", "update", "mv", "snap", "log"] {
        run(r, &["help", cmd], 0);
    }
    let o = Command::new(env!("CARGO_BIN_EXE_keeper"))
        .current_dir(r.parent().unwrap())
        .args(["status", "--root", p(r)])
        .output()
        .unwrap();
    assert!(o.status.success());
}
