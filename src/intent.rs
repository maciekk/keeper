//! Immutable intent revisions. CURRENT, never the largest directory, is authority.
use crate::{
    catalog::{self, Catalog},
    policy::Policy,
};
use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub struct Revision {
    pub number: u64,
    pub parent: u64,
    parent_checksum: String,
    pub checksum: String,
    pub catalog: Catalog,
    // Consumed by the forthcoming named-snapshot CLI.
    #[allow(dead_code)]
    pub message: Option<String>,
    #[allow(dead_code)]
    pub recorded_ns: u128,
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("b3:{}", blake3::hash(bytes).to_hex())
}

pub(crate) fn number(text: &str) -> Result<u64> {
    let n: u64 = text.parse()?;
    ensure!(n.to_string() == text, "noncanonical integer");
    Ok(n)
}

pub(crate) fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

pub fn exists(root: &Path) -> Result<bool> {
    present(&root.join(".keeper/intent"))
}

fn directory(path: &Path) -> Result<()> {
    ensure!(
        fs::symlink_metadata(path)?.is_dir(),
        "{} must be a real directory",
        path.display()
    );
    Ok(())
}

fn boundary() {
    #[cfg(test)]
    {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static STEP: AtomicUsize = AtomicUsize::new(0);
        if let Ok(target) = std::env::var("KEEPER_INTENT_KILL_STEP")
            && STEP.fetch_add(1, Ordering::SeqCst) == target.parse::<usize>().unwrap()
        {
            unsafe {
                libc::raise(libc::SIGKILL);
            }
        }
    }
}

fn sync(path: &Path) -> Result<()> {
    boundary();
    File::open(path)?.sync_all()?;
    boundary();
    Ok(())
}

fn create_dir(path: &Path) -> Result<()> {
    fs::create_dir(path)?;
    boundary();
    sync(path.parent().context("directory parent")?)
}

pub(crate) fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
    boundary();
    f.write_all(bytes)?;
    boundary();
    f.sync_all()?;
    boundary();
    Ok(())
}

fn manifest(
    n: u64,
    parent: u64,
    parent_checksum: &str,
    time: u128,
    objects: &[u8],
    paths: &[u8],
    policy: &Policy,
) -> String {
    let mut text = format!(
        "keeper-intent-version\t1\nrevision\t{n}\nparent\t{parent}\nparent-manifest\t{parent_checksum}\nrecorded-unix-ns\t{time}\nfile\tobjects.tsv\t{}\nfile\tpaths.tsv\t{}\n",
        digest(objects),
        digest(paths)
    );
    if let Some(source) = &policy.source {
        text.push_str(&format!("file\tscope.ignore\t{}\n", digest(source)));
    }
    text
}

fn described_manifest(base: String, message: Option<&str>) -> String {
    match message {
        None => base,
        Some(message) => base
            .replacen(
                "keeper-intent-version\t1\n",
                "keeper-intent-version\t2\n",
                1,
            )
            .replacen(
                "file\tobjects.tsv\t",
                &format!(
                    "message\t{}\nfile\tobjects.tsv\t",
                    crate::encoding::encode(message.as_bytes())
                ),
                1,
            ),
    }
}

pub fn is_revision(dir: &Path) -> Result<bool> {
    Ok(catalog::read_regular(&dir.join("MANIFEST"))?.starts_with(b"keeper-intent-version\t"))
}

pub fn read_revision(dir: &Path) -> Result<Revision> {
    let mut identity = None;
    let catalog = catalog::read_with_manifest(dir, |text, objects, paths, policy| {
        let rows: Vec<_> = text.lines().collect();
        ensure!(
            rows.len() >= 7
                && matches!(
                    rows[0],
                    "keeper-intent-version\t1" | "keeper-intent-version\t2"
                ),
            "invalid intent manifest"
        );
        let n = number(
            rows[1]
                .strip_prefix("revision\t")
                .context("revision field")?,
        )?;
        let parent = number(rows[2].strip_prefix("parent\t").context("parent field")?)?;
        ensure!(
            n > 0 && parent < n && (n == 1) == (parent == 0),
            "invalid revision ancestry"
        );
        let parent_checksum = rows[3]
            .strip_prefix("parent-manifest\t")
            .context("parent manifest field")?;
        ensure!(
            if parent == 0 {
                parent_checksum == "-"
            } else {
                catalog::valid_hash(parent_checksum)
            },
            "invalid parent manifest checksum"
        );
        let time: u128 = rows[4]
            .strip_prefix("recorded-unix-ns\t")
            .context("time field")?
            .parse()?;
        let message = if rows[0] == "keeper-intent-version\t2" {
            Some(String::from_utf8(crate::encoding::decode(
                rows[5].strip_prefix("message\t").context("message field")?,
            )?)?)
        } else {
            None
        };
        ensure!(
            text == described_manifest(
                manifest(n, parent, parent_checksum, time, objects, paths, policy),
                message.as_deref()
            ),
            "intent checksum or canonical format mismatch"
        );
        identity = Some((
            n,
            parent,
            parent_checksum.to_owned(),
            digest(text.as_bytes()),
            message,
            time,
        ));
        Ok(())
    })?;
    let (number, parent, parent_checksum, checksum, message, recorded_ns) =
        identity.context("missing revision identity")?;
    Ok(Revision {
        number,
        parent,
        parent_checksum,
        checksum,
        catalog,
        message,
        recorded_ns,
    })
}

pub fn load(root: &Path) -> Result<Revision> {
    let store = catalog::metadata_dir(root)?.join("intent");
    directory(&store)?;
    directory(&store.join("revisions"))?;
    let reference = String::from_utf8(catalog::read_regular(&store.join("CURRENT"))?)?;
    let fields: Vec<_> = reference.trim_end_matches('\n').split('\t').collect();
    ensure!(
        fields.len() == 3 && fields[0] == "keeper-current-1",
        "invalid CURRENT"
    );
    let n = number(fields[1])?;
    ensure!(
        reference == format!("keeper-current-1\t{n}\t{}\n", fields[2]),
        "noncanonical CURRENT"
    );
    let revision = read_revision(&store.join("revisions").join(n.to_string()))?;
    ensure!(
        revision.number == n && revision.checksum == fields[2],
        "CURRENT revision checksum mismatch"
    );
    // Validate the published ancestry, ignoring orphan revisions.
    let mut parent = revision.parent;
    let mut checksum = revision.parent_checksum.clone();
    while parent != 0 {
        let previous = read_revision(&store.join("revisions").join(parent.to_string()))?;
        ensure!(
            previous.number == parent && previous.checksum == checksum,
            "parent directory identity or manifest checksum mismatch"
        );
        parent = previous.parent;
        checksum = previous.parent_checksum;
    }
    Ok(revision)
}

fn publish_revision(
    store: &Path,
    n: u64,
    parent: u64,
    parent_checksum: &str,
    catalog: &Catalog,
) -> Result<Revision> {
    publish_revision_described(store, n, parent, parent_checksum, catalog, None)
}

fn publish_revision_described(
    store: &Path,
    n: u64,
    parent: u64,
    parent_checksum: &str,
    catalog: &Catalog,
    message: Option<&str>,
) -> Result<Revision> {
    let (paths, objects) = catalog::tables(catalog)?;
    let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let text = described_manifest(
        manifest(
            n,
            parent,
            parent_checksum,
            time,
            objects.as_bytes(),
            paths.as_bytes(),
            &catalog.policy,
        ),
        message,
    );
    let stage = tempfile::Builder::new()
        .prefix(".stage-")
        .tempdir_in(store)?;
    for (name, bytes) in [
        ("MANIFEST", text.as_bytes()),
        ("paths.tsv", paths.as_bytes()),
        ("objects.tsv", objects.as_bytes()),
    ] {
        write_file(&stage.path().join(name), bytes)?;
    }
    if let Some(source) = &catalog.policy.source {
        write_file(&stage.path().join("scope.ignore"), source)?;
    }
    let revision = read_revision(stage.path())?;
    ensure!(revision.catalog == *catalog, "intent round-trip failed");
    sync(stage.path())?;
    crate::cancel::check()?;
    boundary();
    catalog::rename_no_replace(stage.path(), &store.join("revisions").join(n.to_string()))?;
    boundary();
    sync(&store.join("revisions"))?;
    Ok(revision)
}

fn switch(store: &Path, revision: &Revision) -> Result<()> {
    let mut temp = tempfile::Builder::new()
        .prefix(".current-")
        .tempfile_in(store)?;
    writeln!(
        temp,
        "keeper-current-1\t{}\t{}",
        revision.number, revision.checksum
    )?;
    boundary();
    temp.as_file().sync_all()?;
    boundary();
    crate::cancel::check()?;
    fs::rename(temp.path(), store.join("CURRENT"))?;
    boundary();
    sync(store).context("CURRENT switched but directory sync failed: publication uncertain; inspect catalog check before retrying")
}

/// Caller holds LOCK and has freshly revalidated the selected actions.
// Used by the forthcoming selective-acceptance command.
#[allow(dead_code)]
pub fn publish(root: &Path, parent: &Revision, catalog: &Catalog) -> Result<Revision> {
    publish_named(root, parent, catalog, None)
}

pub fn publish_named(
    root: &Path,
    parent: &Revision,
    catalog: &Catalog,
    message: Option<&str>,
) -> Result<Revision> {
    let active = load(root)?;
    ensure!(
        active.number == parent.number && active.checksum == parent.checksum,
        "active parent changed"
    );
    let store = root.join(".keeper/intent");
    let mut highest = parent.number;
    for item in fs::read_dir(store.join("revisions"))? {
        let item = item?;
        let name = item.file_name();
        let n = number(name.to_str().context("invalid revision name")?)?;
        ensure!(
            n > 0 && item.file_type()?.is_dir(),
            "invalid revision directory"
        );
        highest = highest.max(n);
    }
    let n = highest
        .checked_add(1)
        .context("revision numbers exhausted")?;
    let revision =
        publish_revision_described(&store, n, parent.number, &parent.checksum, catalog, message)?;
    switch(&store, &revision)?;
    Ok(revision)
}

/// Explicit, restartable, metadata-only migration. Never consults dataset bytes.
pub fn migrate(root: &Path) -> Result<Revision> {
    let _guard = catalog::InitGuard::lock(root)?;
    migrate_locked(root)
}

/// Caller holds the dataset writer lock.
pub(crate) fn migrate_locked(root: &Path) -> Result<Revision> {
    let metadata = catalog::metadata_dir(root)?;
    let store = metadata.join("intent");
    let original = metadata.join("catalog");
    let retained = metadata.join("legacy-catalog");
    if exists(root)? {
        directory(&store)?;
        ensure!(
            !present(&store.join("CURRENT"))?,
            "intent already active; migration is not required"
        );
    } else {
        ensure!(
            !present(&retained)?,
            "conflicting legacy-catalog; investigate before migration"
        );
        catalog::read_generation(&original)?;
        create_dir(&store)?;
    }
    ensure!(
        present(&original)? != present(&retained)?,
        "migration requires exactly one original catalog location"
    );
    let source = if present(&original)? {
        &original
    } else {
        &retained
    };
    ensure!(
        !is_revision(source)?,
        "migration source must be a legacy catalog"
    );
    let catalog = catalog::read_generation(source)?;
    let revisions = store.join("revisions");
    if !present(&revisions)? {
        create_dir(&revisions)?;
    }
    directory(&revisions)?;
    for child in fs::read_dir(&revisions)? {
        ensure!(
            child?.file_name() == "1",
            "unexpected migration revision; investigate"
        );
    }
    let revision = if present(&revisions.join("1"))? {
        let r = read_revision(&revisions.join("1"))?;
        ensure!(
            r.number == 1 && r.parent == 0 && r.catalog == catalog,
            "migration revision disagrees with retained original"
        );
        r
    } else {
        publish_revision(&store, 1, 0, "-", &catalog)?
    };
    if present(&original)? {
        boundary();
        catalog::rename_no_replace(&original, &retained)?;
        boundary();
        sync(&metadata)?;
    }
    switch(&store, &revision)?;
    Ok(revision)
}

/// Export a self-contained revision, not its history or physical evidence.
pub fn export(root: &Path, destination: &Path) -> Result<Revision> {
    let revision = load(root)?;
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stage = tempfile::Builder::new()
        .prefix(".export-")
        .tempdir_in(parent)?;
    let source = root
        .join(".keeper/intent/revisions")
        .join(revision.number.to_string());
    for name in ["MANIFEST", "paths.tsv", "objects.tsv", "scope.ignore"] {
        if name == "scope.ignore" && revision.catalog.policy.source.is_none() {
            continue;
        }
        write_file(
            &stage.path().join(name),
            &catalog::read_regular(&source.join(name))?,
        )?;
    }
    ensure!(
        read_revision(stage.path())?.checksum == revision.checksum,
        "export revision changed"
    );
    sync(stage.path())?;
    catalog::rename_no_replace(stage.path(), destination)?;
    sync(parent).context("export published but parent sync failed")?;
    Ok(revision)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Entry;
    use std::process::Command;

    fn changed() -> Catalog {
        let mut c = Catalog::new();
        c.insert(
            b"new".to_vec(),
            Entry {
                hash: digest(b"new bytes"),
                size: 9,
            },
        );
        c
    }

    // Fault injection is compiled only into the unit-test executable, never the CLI.
    #[test]
    fn crash_helper() {
        let Some(root) = std::env::var_os("KEEPER_INTENT_TEST_ROOT") else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        if std::env::var("KEEPER_INTENT_TEST_MODE").unwrap() == "migrate" {
            migrate(&root).unwrap();
        } else {
            let _guard = catalog::InitGuard::lock(&root).unwrap();
            let parent = load(&root).unwrap();
            if std::env::var("KEEPER_INTENT_TEST_MODE").unwrap() == "snap" {
                publish_named(
                    &root,
                    &parent,
                    &changed(),
                    Some("Named stopping point\nwith details"),
                )
                .unwrap();
            } else {
                publish(&root, &parent, &changed()).unwrap();
            }
        }
    }

    #[test]
    fn kill_at_each_storage_boundary_preserves_authority_and_migration_recovery() {
        use std::os::unix::process::ExitStatusExt;
        for mode in ["migrate", "publish", "snap"] {
            let mut kills = 0;
            for step in 0..100 {
                let tmp = tempfile::tempdir().unwrap();
                let root = tmp.path();
                let guard = catalog::InitGuard::lock(root).unwrap();
                guard.publish(&Catalog::new()).unwrap();
                drop(guard);
                if mode != "migrate" {
                    migrate(root).unwrap();
                }
                let result = Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "intent::tests::crash_helper", "--nocapture"])
                    .env("KEEPER_INTENT_TEST_ROOT", root)
                    .env("KEEPER_INTENT_TEST_MODE", mode)
                    .env("KEEPER_INTENT_KILL_STEP", step.to_string())
                    .output()
                    .unwrap();
                if result.status.success() {
                    break;
                }
                assert_eq!(
                    result.status.signal(),
                    Some(libc::SIGKILL),
                    "{}",
                    String::from_utf8_lossy(&result.stderr)
                );
                kills += 1;
                if mode == "migrate" {
                    if !root.join(".keeper/intent/CURRENT").exists() {
                        // An incomplete store must fail closed even while catalog/ exists.
                        if exists(root).unwrap() {
                            assert!(catalog::load(root).is_err());
                        }
                        migrate(root).unwrap();
                    }
                    assert_eq!(load(root).unwrap().catalog, Catalog::new());
                    assert_eq!(
                        catalog::read_generation(&root.join(".keeper/legacy-catalog")).unwrap(),
                        Catalog::new()
                    );
                } else {
                    let active = load(root).unwrap();
                    assert!(active.catalog == Catalog::new() || active.catalog == changed());
                    if mode == "snap" && active.number != 1 {
                        assert_eq!(
                            active.message.as_deref(),
                            Some("Named stopping point\nwith details")
                        );
                    }
                    assert_eq!(
                        read_revision(&root.join(".keeper/intent/revisions/1"))
                            .unwrap()
                            .catalog,
                        Catalog::new()
                    );
                    let _guard = catalog::InitGuard::lock(root).unwrap();
                    let next = publish(root, &active, &changed()).unwrap();
                    assert!(next.number > active.number);
                    assert_eq!(load(root).unwrap().catalog, changed());
                }
            }
            assert!(
                kills > 15 && kills < 100,
                "did not exercise every {mode} boundary: {kills}"
            );
        }
    }

    #[test]
    fn migration_refuses_disagreeing_originals_and_conflicting_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let guard = catalog::InitGuard::lock(root).unwrap();
        guard.publish(&Catalog::new()).unwrap();
        drop(guard);
        let store = root.join(".keeper/intent");
        create_dir(&store).unwrap();
        create_dir(&store.join("revisions")).unwrap();
        publish_revision(&store, 1, 0, "-", &changed()).unwrap();
        assert!(migrate(root).is_err());
        assert!(!store.join("CURRENT").exists());
        assert!(root.join(".keeper/catalog").exists());
    }
}
