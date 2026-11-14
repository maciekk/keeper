use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

fn run(cwd: &Path, args: &[&str], code: i32) -> String {
    let o = Command::new(env!("CARGO_BIN_EXE_keeper"))
        .current_dir(cwd)
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
    assert_eq!(
        o.status.code(),
        Some(code),
        "{args:?} from {}: {text}",
        cwd.display()
    );
    text
}
fn setup() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    fs::create_dir_all(t.path().join("2026/trip")).unwrap();
    fs::write(t.path().join("old"), b"old").unwrap();
    fs::write(t.path().join("2026/edited"), b"before").unwrap();
    fs::write(t.path().join("2026/deleted"), b"deleted").unwrap();
    fs::write(t.path().join("2026/trip/photo"), b"photo").unwrap();
    run(t.path(), &["init"], 0);
    t
}

#[test]
fn selectors_are_cwd_relative_and_bare_add_never_widens_to_parent() {
    let t = setup();
    let r = t.path();
    let sub = r.join("2026");
    fs::write(r.join("outside"), b"not selected").unwrap();
    fs::write(sub.join("trip/new"), b"new photo").unwrap();
    fs::write(sub.join("another"), b"another").unwrap();
    run(&sub, &["add", "trip", "--yes"], 0);
    let status = run(r, &["status"], 1);
    assert!(!status.contains("ADDED\t2026/trip/new"));
    assert!(status.contains("ADDED\t2026/another") && status.contains("ADDED\toutside"));
    run(&sub, &["add", "--yes"], 0);
    let status = run(r, &["status"], 1);
    assert!(!status.contains("ADDED\t2026/another") && status.contains("ADDED\toutside"));
    assert!(!sub.join(".keeper").exists());
}

#[test]
fn bare_rm_and_snap_only_authorize_current_subtree() {
    let t = setup();
    let r = t.path();
    let sub = r.join("2026");
    fs::remove_file(r.join("old")).unwrap();
    fs::remove_file(sub.join("deleted")).unwrap();
    run(&sub, &["rm", "--yes"], 0);
    let status = run(r, &["status"], 1);
    assert!(status.contains("MISSING\told"));
    assert!(!status.contains("MISSING\t2026/deleted"));
    fs::write(sub.join("edited"), b"after").unwrap();
    fs::write(sub.join("new"), b"new").unwrap();
    fs::write(r.join("outside"), b"outside").unwrap();
    run(&sub, &["snap", "-m", "Subtree session", "--yes"], 0);
    let status = run(r, &["status"], 1);
    assert!(status.contains("MISSING\told") && status.contains("ADDED\toutside"));
    assert!(!status.contains("CHANGED\t2026/edited") && !status.contains("ADDED\t2026/new"));
    assert!(run(&sub, &["log"], 0).contains("Subtree session"));
}

#[test]
fn update_move_and_parent_relative_selectors_use_current_directory() {
    let t = setup();
    let r = t.path();
    let sub = r.join("2026");
    fs::write(sub.join("edited"), b"after").unwrap();
    run(&sub, &["update", "edited", "--yes"], 0);
    fs::rename(sub.join("edited"), sub.join("renamed")).unwrap();
    run(&sub, &["mv", "edited", "renamed", "--yes"], 0);
    fs::write(sub.join("new"), b"new").unwrap();
    run(&sub.join("trip"), &["add", "../new", "--yes"], 0);
    run(r, &["status"], 0);
    run(&sub, &["add", "../../outside", "--yes"], 2);
}

#[test]
fn explicit_root_overrides_cwd_and_selects_the_whole_requested_dataset() {
    let t = setup();
    let r = t.path();
    let sub = r.join("2026");
    fs::write(sub.join("new"), b"new").unwrap();
    fs::write(r.join("outside"), b"outside").unwrap();
    let root = r.to_str().unwrap();
    run(
        &sub.join("trip"),
        &["--root", root, "add", "2026/new", "--yes"],
        0,
    );
    assert!(run(r, &["status"], 1).contains("ADDED\toutside"));
    run(&sub, &["--root", root, "add", "--yes"], 0);
    run(r, &["status"], 0);
}

#[test]
fn nested_datasets_choose_nearest_and_corrupt_markers_never_fall_back() {
    let t = setup();
    let r = t.path();
    let nested = r.join("2026");
    run(&nested, &["init"], 0); // init remains explicit/current-dir, never discovers
    fs::write(nested.join("new"), b"new").unwrap();
    run(&nested.join("trip"), &["add", "../new", "--yes"], 0);
    assert!(!r.join(".keeper/intent").exists());
    let outer = fs::read(r.join(".keeper/catalog/MANIFEST")).unwrap();
    fs::write(nested.join(".keeper/intent/CURRENT"), b"corrupt").unwrap();
    run(&nested.join("trip"), &["add", "--yes"], 2);
    run(&nested.join("trip"), &["log"], 2);
    assert_eq!(fs::read(r.join(".keeper/catalog/MANIFEST")).unwrap(), outer);
}

#[test]
fn non_directory_symlink_and_incomplete_nearest_markers_are_boundaries() {
    use std::os::unix::fs::symlink;
    for kind in ["file", "symlink", "empty"] {
        let t = setup();
        let r = t.path();
        let sub = r.join("2026");
        let marker = sub.join(".keeper");
        match kind {
            "file" => fs::write(&marker, b"not metadata").unwrap(),
            "symlink" => symlink(r.join(".keeper"), &marker).unwrap(),
            "empty" => fs::create_dir(&marker).unwrap(),
            _ => unreachable!(),
        }
        run(&sub.join("trip"), &["add", "--dry-run"], 2);
        assert!(!r.join(".keeper/intent").exists());
    }
}

#[test]
fn markers_remain_baseline_only_and_discovery_preview_writes_nothing() {
    let t = setup();
    let r = t.path();
    let sub = r.join("2026");
    fs::remove_file(r.join(".keeper/LOCK")).unwrap();
    fs::write(sub.join("new"), b"new").unwrap();
    run(&sub, &["add", "--dry-run"], 0);
    assert!(!r.join(".keeper/LOCK").exists());
    assert!(!sub.join(".keeper").exists());
    run(&sub, &["snap", "--mark", "-m", "Before work", "--yes"], 0);
    assert!(run(r, &["status"], 1).contains("ADDED\t2026/new"));
    assert!(run(&sub, &["log"], 0).contains("Before work"));
}

#[test]
fn unreadable_nearest_metadata_blocks_fallback_and_byte_paths_are_preserved() {
    use std::os::unix::{ffi::OsStringExt, fs::PermissionsExt};
    let t = setup();
    let r = t.path();
    let sub = r.join("2026");
    if unsafe { libc::geteuid() } != 0 {
        fs::create_dir(sub.join(".keeper")).unwrap();
        fs::set_permissions(sub.join(".keeper"), fs::Permissions::from_mode(0o0)).unwrap();
        run(&sub.join("trip"), &["add", "--dry-run"], 2);
        fs::set_permissions(sub.join(".keeper"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_dir(sub.join(".keeper")).unwrap();
        assert!(!r.join(".keeper/intent").exists());
    }
    let name = std::ffi::OsString::from_vec(b"photo-\xff\n".to_vec());
    fs::write(sub.join(&name), b"photo").unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_keeper"))
        .current_dir(&sub)
        .args(["--progress", "never", "add", "--yes", "--"])
        .arg(&name)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    run(r, &["status"], 0);
}

#[test]
fn discovery_does_not_follow_selector_symlinks_or_escape_to_outer_dataset() {
    use std::os::unix::fs::symlink;
    let t = setup();
    let r = t.path();
    let sub = r.join("2026");
    symlink(r, sub.join("link")).unwrap();
    run(&sub, &["add", "link/../old", "--yes"], 2);
    run(&sub, &["add", "link/old", "--yes"], 2);
    run(&sub, &["add", ".keeper/../old", "--yes"], 2);
    let plain = tempfile::tempdir().unwrap();
    assert!(run(plain.path(), &["add", "--dry-run"], 2).contains("no enclosing Keeper dataset"));
}
