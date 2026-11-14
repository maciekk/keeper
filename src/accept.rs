//! Exact-parent review plans. Selected reads only; no implicit authorization.
use crate::{
    catalog::{self, Catalog, Entry},
    encoding,
    intent::{self, Revision},
    progress::Progress,
    scan,
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::BTreeMap,
    ffi::{CString, OsString},
    fs::{self, File, Metadata},
    io::Write,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStringExt, fs::MetadataExt},
    },
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub(crate) struct Action {
    pub old: Option<Entry>,
    pub new: Option<Entry>,
}
struct Plan {
    root: Vec<u8>,
    dev: u64,
    ino: u64,
    parent: u64,
    checksum: String,
    actions: BTreeMap<Vec<u8>, Action>,
}

fn entry_fields(entry: &Option<Entry>) -> String {
    match entry {
        Some(e) => format!("{}\t{}", e.hash, e.size),
        None => "-\t-".into(),
    }
}

impl Plan {
    fn encode(&self) -> String {
        let mut text = format!(
            "keeper-accept-plan\t1\nroot\t{}\t{}\t{}\nparent\t{}\t{}\n",
            encoding::encode(&self.root),
            self.dev,
            self.ino,
            self.parent,
            self.checksum
        );
        for (path, action) in &self.actions {
            let kind = match (&action.old, &action.new) {
                (None, Some(_)) => "add",
                (Some(_), Some(_)) => "replace",
                (Some(_), None) => "remove",
                _ => unreachable!(),
            };
            text.push_str(&format!(
                "{kind}\t{}\t{}\t{}\n",
                encoding::encode(path),
                entry_fields(&action.old),
                entry_fields(&action.new)
            ));
        }
        text.push_str(&format!("checksum\t{}\n", intent::digest(text.as_bytes())));
        text
    }

    fn read(path: &Path) -> Result<Self> {
        let text = String::from_utf8(catalog::read_regular(path)?)?;
        let lines: Vec<_> = text.lines().collect();
        ensure!(
            lines.len() >= 5 && lines[0] == "keeper-accept-plan\t1",
            "invalid acceptance plan"
        );
        let root: Vec<_> = lines[1].split('\t').collect();
        let parent: Vec<_> = lines[2].split('\t').collect();
        ensure!(
            root.len() == 4 && root[0] == "root" && parent.len() == 3 && parent[0] == "parent",
            "invalid plan identity"
        );
        ensure!(catalog::valid_hash(parent[2]), "invalid parent checksum");
        let mut plan = Self {
            root: encoding::decode(root[1])?,
            dev: intent::number(root[2])?,
            ino: intent::number(root[3])?,
            parent: intent::number(parent[1])?,
            checksum: parent[2].into(),
            actions: BTreeMap::new(),
        };
        for line in &lines[3..lines.len() - 1] {
            let f: Vec<_> = line.split('\t').collect();
            ensure!(f.len() == 6, "invalid plan action");
            let parse = |hash: &str, size: &str| -> Result<Option<Entry>> {
                if hash == "-" && size == "-" {
                    return Ok(None);
                }
                ensure!(catalog::valid_hash(hash), "invalid action hash");
                Ok(Some(Entry {
                    hash: hash.into(),
                    size: intent::number(size)?,
                }))
            };
            let action = Action {
                old: parse(f[2], f[3])?,
                new: parse(f[4], f[5])?,
            };
            ensure!(
                matches!(
                    (f[0], &action.old, &action.new),
                    ("add", None, Some(_))
                        | ("replace", Some(_), Some(_))
                        | ("remove", Some(_), None)
                ),
                "invalid action kind"
            );
            let path = encoding::decode(f[1])?;
            encoding::validate_path(&path)?;
            ensure!(
                plan.actions.insert(path, action).is_none(),
                "duplicate plan action"
            );
        }
        ensure!(
            plan.encode() == text,
            "plan checksum or canonical format mismatch"
        );
        Ok(plan)
    }

    fn validate(&self, root: &Path, revision: &Revision) -> Result<Catalog> {
        let meta = fs::metadata(root)?;
        ensure!(
            self.root == root.as_os_str().as_encoded_bytes()
                && self.dev == meta.dev()
                && self.ino == meta.ino(),
            "plan belongs to a different physical root"
        );
        ensure!(
            self.parent == revision.number && self.checksum == revision.checksum,
            "stale plan: active parent differs; review a new plan (no automatic rebase)"
        );
        revision.catalog.policy.ensure_current(root)?;
        ensure!(!self.actions.is_empty(), "empty acceptance plan");
        updated(&revision.catalog, &self.actions)
    }
}

pub(crate) fn updated(catalog: &Catalog, actions: &BTreeMap<Vec<u8>, Action>) -> Result<Catalog> {
    let mut next = catalog.clone();
    for (path, action) in actions {
        encoding::validate_path(path)?;
        ensure!(
            path != b".keeperignore" && !next.policy.excludes(path, false),
            "selected path is outside recorded scope"
        );
        ensure!(
            catalog.get(path) == action.old.as_ref(),
            "action old expectation differs from parent"
        );
        ensure!(action.old != action.new, "action does not change intent");
        match &action.new {
            Some(e) => {
                next.insert(path.clone(), e.clone());
            }
            None => {
                next.remove(path);
            }
        }
    }
    catalog::tables(&next)?; // includes file/ancestor conflicts, scope and object invariants
    Ok(next)
}

// Walk every ancestor with openat(O_NOFOLLOW), so a selected path cannot read
// outside the root via a symlink. Missing ancestors imply absence; unreadable or
// non-directory ancestors are errors, never evidence for removal.
fn parent(root: &Path, path: &[u8]) -> Result<Option<(File, OsString)>> {
    encoding::validate_path(path)?;
    let mut dir = File::open(root)?;
    let parts: Vec<_> = path.split(|&b| b == b'/').collect();
    for part in &parts[..parts.len() - 1] {
        crate::cancel::check()?;
        let name = CString::new(*part)?;
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(error).context("selected ancestor must be a readable real directory");
        }
        dir = unsafe { File::from_raw_fd(fd) };
    }
    Ok(Some((
        dir,
        OsString::from_vec(parts.last().unwrap().to_vec()),
    )))
}

pub(crate) fn inspect(root: &Path, relative: &[u8]) -> Result<Option<(File, PathBuf, Metadata)>> {
    let Some((dir, name)) = parent(root, relative)? else {
        return Ok(None);
    };
    let path = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd())).join(name);
    match fs::symlink_metadata(&path) {
        Ok(meta) => Ok(Some((dir, path, meta))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub(crate) fn observe(
    root: &Path,
    relative: &[u8],
    progress: &mut Progress<'_>,
) -> Result<(Entry, Metadata)> {
    let (_dir, path, meta) = inspect(root, relative)?.context("selected file is absent")?;
    ensure!(
        meta.is_file(),
        "selected addition/replacement must be a regular file"
    );
    progress.begin_file(meta.len());
    let entry = scan::hash_file(&path, &meta, |n| {
        progress.advance(0, n)?;
        Ok(())
    })?;
    progress.end_file();
    progress.advance(1, 0)?;
    Ok((entry, meta))
}

pub(crate) fn absent(root: &Path, path: &[u8]) -> Result<()> {
    ensure!(
        inspect(root, path)?.is_none(),
        "selected removal still exists (including symlinks or special files)"
    );
    Ok(())
}

/// Create a checksummed, human-reviewable plan; does not change intent.
pub fn plan(
    root: &Path,
    destination: &Path,
    add: Vec<PathBuf>,
    replace: Vec<PathBuf>,
    remove: Vec<PathBuf>,
    progress: &mut Progress<'_>,
) -> Result<()> {
    let _guard = catalog::InitGuard::lock(root)?;
    let revision =
        intent::load(root).context("run keeper catalog migrate before selective acceptance")?;
    revision.catalog.policy.ensure_current(root)?;
    let meta = fs::metadata(root)?;
    let mut plan = Plan {
        root: root.as_os_str().as_encoded_bytes().to_vec(),
        dev: meta.dev(),
        ino: meta.ino(),
        parent: revision.number,
        checksum: revision.checksum.clone(),
        actions: BTreeMap::new(),
    };
    progress.begin("Review selected files", None)?;
    for (kind, paths) in [("add", add), ("replace", replace), ("remove", remove)] {
        for path in paths {
            crate::cancel::check()?;
            let path = path.as_os_str().as_encoded_bytes().to_vec();
            encoding::validate_path(&path)?;
            ensure!(
                path != b".keeperignore" && !revision.catalog.policy.excludes(&path, false),
                "selected path outside recorded scope"
            );
            let old = revision.catalog.get(&path).cloned();
            ensure!(
                (kind == "add") == old.is_none(),
                "{kind} action disagrees with parent at {}",
                encoding::encode(&path)
            );
            ensure!(!plan.actions.contains_key(&path), "duplicate selected path");
            let new = if kind == "remove" {
                absent(root, &path)?;
                None
            } else {
                Some(observe(root, &path, progress)?.0)
            };
            plan.actions.insert(path, Action { old, new });
        }
    }
    progress.finish()?;
    plan.validate(root, &revision)?;
    crate::cancel::check()?;
    // Publish without replacement only after a fully validated plan is durable.
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::Builder::new()
        .prefix(".plan-")
        .tempfile_in(parent)?;
    temp.write_all(plan.encode().as_bytes())?;
    temp.as_file().sync_all()?;
    catalog::rename_no_replace(temp.path(), destination)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

/// All-or-nothing application. Unselected files are not opened or traversed.
pub fn apply(root: &Path, source: &Path, progress: &mut Progress<'_>) -> Result<Revision> {
    let _guard = catalog::InitGuard::lock(root)?;
    let plan = Plan::read(source)?;
    let revision = intent::load(root)?;
    let next = plan.validate(root, &revision)?;
    revalidate(root, &plan.actions, progress)?;
    revision.catalog.policy.ensure_current(root)?;
    crate::cancel::check()?;
    intent::publish(root, &revision, &next)
}

pub(crate) fn revalidate(
    root: &Path,
    actions: &BTreeMap<Vec<u8>, Action>,
    progress: &mut Progress<'_>,
) -> Result<()> {
    let mut reads = BTreeMap::new();
    progress.begin("Revalidate approved files", None)?;
    for (path, action) in actions {
        crate::cancel::check()?;
        match &action.new {
            Some(expected) => {
                let (observed, meta) = observe(root, path, progress)
                    .with_context(|| format!("revalidate {}", encoding::encode(path)))?;
                ensure!(
                    &observed == expected,
                    "approved bytes changed at {}; create and review a new plan",
                    encoding::encode(path)
                );
                reads.insert(path, meta);
            }
            None => absent(root, path)?,
        }
    }
    progress.finish()?;
    for (path, action) in actions {
        if action.new.is_none() {
            absent(root, path)?;
        } else {
            let (_, _, meta) = inspect(root, path)?.context("selected path disappeared")?;
            ensure!(
                scan::same_file(&reads[path], &meta),
                "selected physical identity changed before publication"
            );
        }
    }
    crate::cancel::check()?;
    Ok(())
}

pub fn show(source: &Path, out: &mut impl Write) -> Result<()> {
    let plan = Plan::read(source)?;
    writeln!(
        out,
        "Parent R{} ({})\nRoot: {}",
        plan.parent,
        plan.checksum,
        encoding::encode(&plan.root)
    )?;
    for (path, action) in &plan.actions {
        let kind = match (&action.old, &action.new) {
            (None, Some(_)) => "ADD",
            (Some(_), Some(_)) => "REPLACE",
            (Some(_), None) => "REMOVE",
            _ => bail!("invalid action"),
        };
        writeln!(out, "{kind}\t{}", encoding::encode(path))?;
        if let Some(e) = &action.old {
            writeln!(out, "  old: {} ({})", e.hash, crate::display::bytes(e.size))?;
        }
        if let Some(e) = &action.new {
            writeln!(out, "  new: {} ({})", e.hash, crate::display::bytes(e.size))?;
        }
    }
    writeln!(
        out,
        "{} action(s). Unselected expectations remain unchanged.\nApplying rereads selected new bytes. Stop writers; this is not a snapshot.",
        crate::display::number(plan.actions.len())
    )?;
    Ok(())
}
