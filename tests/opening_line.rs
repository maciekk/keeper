use assert_cmd::cargo::cargo_bin_cmd;
use std::{
    fs,
    io::Read,
    os::fd::FromRawFd,
    path::Path,
    process::{Command, Stdio},
};

fn assert_one_opening_line(bytes: &[u8]) {
    assert!(bytes.starts_with(b"\n"), "{bytes:?}");
    assert!(!bytes.starts_with(b"\n\n"), "{bytes:?}");
}

#[test]
fn every_command_help_version_and_errors_have_one_opening_line() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let left_str = left.path().to_str().unwrap();
    let right_str = right.path().to_str().unwrap();
    for root in [left.path(), right.path()] {
        let output = cargo_bin_cmd!("keeper")
            .arg("init")
            .arg(root)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_one_opening_line(&output);
    }
    for args in [
        vec!["--help"],
        vec!["help"],
        vec!["help", "compare"],
        vec!["help", "catalog", "check"],
        vec!["--version"],
        vec!["preflight", left_str],
        vec!["status", left_str],
        vec!["status", "--resume", left_str],
        vec!["catalog", "check", left_str],
        vec!["compare", left_str, right_str],
        vec!["compare", "--metadata-only", left_str, right_str],
        vec!["compare", "--live", left_str, right_str],
    ] {
        let output = cargo_bin_cmd!("keeper")
            .args(&args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_one_opening_line(&output);
    }
    let fresh = tempfile::tempdir().unwrap();
    let output = cargo_bin_cmd!("keeper")
        .args(["init", "--resume"])
        .arg(fresh.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_one_opening_line(&output);
    for args in [
        vec!["not-a-command"],
        vec!["compare"],
        vec!["status", "/does/not/exist/keeper-test"],
    ] {
        let output = cargo_bin_cmd!("keeper")
            .args(args)
            .assert()
            .code(2)
            .get_output()
            .clone();
        assert_one_opening_line(&output.stdout);
        assert!(!output.stderr.is_empty());
    }
}

fn terminal_output(left: &Path, right: &Path, progress: &str) -> Vec<u8> {
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: valid fd output pointers; null optional termios/winsize use defaults.
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
    // SAFETY: openpty returned distinct owned file descriptors.
    let mut master = unsafe { fs::File::from_raw_fd(master) };
    let slave = unsafe { fs::File::from_raw_fd(slave) };
    let mut child = Command::new(assert_cmd::cargo::cargo_bin!("keeper"))
        .args([
            "compare",
            "--metadata-only",
            "--progress",
            progress,
            "--color",
            "never",
        ])
        .arg(left)
        .arg(right)
        .stdin(Stdio::null())
        .stdout(slave.try_clone().unwrap())
        .stderr(slave)
        .spawn()
        .unwrap();
    let mut output = Vec::new();
    // Linux PTYs end with EIO once all slave descriptors close.
    if let Err(error) = master.read_to_end(&mut output) {
        assert_eq!(error.raw_os_error(), Some(libc::EIO));
    }
    assert!(child.wait().unwrap().success());
    output
}

#[test]
fn terminal_progress_and_reports_do_not_duplicate_the_opening_separator() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    for progress in ["always", "never"] {
        let output = terminal_output(left.path(), right.path(), progress);
        assert!(output.starts_with(b"\r\n"), "{output:?}");
        assert!(!output.starts_with(b"\r\n\r\n"), "{output:?}");
        if progress == "never" {
            assert!(output.starts_with(b"\r\nComparison:"));
        }
    }
}
