//! Exercise real interrupted sessions without reading a large file.
use assert_cmd::cargo::cargo_bin_cmd;
use std::{
    fs,
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};

fn interrupt_init(root: &Path) -> Vec<u8> {
    // The first record is published before init preflight. A FIFO would fail
    // preflight, so instead use many paths and stop once the durable header exists.
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin!("keeper"))
        .args(["init", "--resume"])
        .arg(root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let work = root.join(".keeper/init.work");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if work.exists() {
            break;
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "init ended before checkpoint was observed"
        );
        assert!(Instant::now() < deadline, "checkpoint timeout");
        std::thread::sleep(Duration::from_millis(1));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(!root.join(".keeper/catalog").exists());
    fs::read(work).unwrap()
}

#[test]
fn adding_or_changing_policy_invalidates_an_interrupted_session() {
    for initial in [None, Some(b"# original\n".as_slice())] {
        let temp = tempfile::tempdir().unwrap();
        if let Some(source) = initial {
            fs::write(temp.path().join(".keeperignore"), source).unwrap();
        }
        // Sparse contents make it impossible to finish before we terminate the session.
        fs::File::create(temp.path().join("large"))
            .unwrap()
            .set_len(2 * 1024 * 1024 * 1024)
            .unwrap();
        let before = interrupt_init(temp.path());
        fs::write(temp.path().join(".keeperignore"), b"/large\n").unwrap();
        let result = cargo_bin_cmd!("keeper")
            .args(["init", "--resume"])
            .arg(temp.path())
            .assert()
            .code(2)
            .get_output()
            .clone();
        let error = String::from_utf8(result.stderr).unwrap();
        assert!(
            error.contains("scope policy or baseline mismatch"),
            "{error}"
        );
        assert_eq!(
            fs::read(temp.path().join(".keeper/init.work")).unwrap(),
            before
        );
        assert!(!temp.path().join(".keeper/catalog").exists());
        // Restoring the exact policy allows the same session to be reopened.
        if let Some(source) = initial {
            fs::write(temp.path().join(".keeperignore"), source).unwrap();
        } else {
            fs::remove_file(temp.path().join(".keeperignore")).unwrap();
        }
        // Do not reread 2 GiB in a unit test: remove the data, then finish the session.
        fs::remove_file(temp.path().join("large")).unwrap();
        cargo_bin_cmd!("keeper")
            .args(["init", "--resume"])
            .arg(temp.path())
            .assert()
            .success();
    }
}
