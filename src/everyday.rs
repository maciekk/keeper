//! Intent-oriented acceptance without a user-managed staging area.
use crate::{
    accept::{self, Action},
    catalog::{self, Catalog},
    display, encoding, intent,
    progress::Progress,
    scan,
};
use anyhow::{Context, Result, ensure};
use clap::Args;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, IsTerminal, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Component,
    path::{Path, PathBuf},
};

#[derive(Args, Default)]
pub struct Options {
    /// Preview; no migration, publication, or prompt. May read selected file bytes.
    #[arg(long, conflicts_with = "yes")]
    pub dry_run: bool,
    /// Authorize without prompting; never bypass validation.
    #[arg(short = 'y', long)]
    pub yes: bool,
    /// List selected paths and exact old/new hashes and sizes.
    #[arg(long)]
    pub long: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Add,
    Remove,
    Update,
    Move,
    Snap,
    Mark,
}

fn selected(path: &[u8], prefix: &[u8]) -> bool {
    prefix.is_empty()
        || path == prefix
        || (path.starts_with(prefix) && path.get(prefix.len()) == Some(&b'/'))
}

fn selector(path: &Path) -> Result<Vec<u8>> {
    ensure!(!path.as_os_str().is_empty(), "empty selector");
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => normalized.push(name),
            _ => anyhow::bail!("selectors must stay within the dataset root"),
        }
    }
    let bytes = normalized.as_os_str().as_encoded_bytes().to_vec();
    if bytes.is_empty() {
        return Ok(bytes);
    }
    encoding::validate_path(&bytes)?;
    ensure!(
        bytes != b".keeperignore",
        ".keeperignore is configuration, not dataset content"
    );
    Ok(bytes)
}

fn directory(root: &Path, relative: &[u8]) -> Result<File> {
    if relative.is_empty() {
        return Ok(File::open(root)?);
    }
    let (_parent, path, before) =
        accept::inspect(root, relative)?.context("directory disappeared")?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    ensure!(
        scan::same_file(&before, &file.metadata()?),
        "directory changed while opening"
    );
    Ok(file)
}

fn walk(
    root: &Path,
    relative: &[u8],
    baseline: &Catalog,
    kind: Kind,
    files: &mut BTreeSet<Vec<u8>>,
) -> Result<()> {
    crate::cancel::check()?;
    let dir = directory(root, relative)?;
    let before = dir.metadata()?;
    let mut names = BTreeSet::new();
    for child in fs::read_dir(format!("/proc/self/fd/{}", dir.as_raw_fd()))? {
        crate::cancel::check()?;
        let name = child?.file_name();
        names.insert(name.clone());
        let mut path = relative.to_vec();
        if !path.is_empty() {
            path.push(b'/');
        }
        path.extend_from_slice(name.as_encoded_bytes());
        if path == b".keeper" || path == b".keeperignore" {
            continue;
        }
        let (_, _, meta) =
            accept::inspect(root, &path)?.context("path disappeared during discovery")?;
        if baseline.policy.excludes(&path, meta.is_dir()) {
            continue;
        }
        if meta.is_dir() {
            walk(root, &path, baseline, kind, files)?;
        } else if kind == Kind::Add && baseline.contains_key(&path) {
            // No content read or health claim for inherited physical files.
        } else {
            ensure!(
                meta.is_file(),
                "unsupported selected object at {}",
                encoding::encode(&path)
            );
            files.insert(path);
        }
    }
    let mut after_names = BTreeSet::new();
    for child in fs::read_dir(format!("/proc/self/fd/{}", dir.as_raw_fd()))? {
        crate::cancel::check()?;
        after_names.insert(child?.file_name());
    }
    // Coarse directory timestamps (notably ntfs3) cannot prove membership stability.
    ensure!(
        names == after_names,
        "directory membership changed during discovery"
    );
    ensure!(
        scan::same_file(&before, &dir.metadata()?),
        "directory changed during discovery"
    );
    if !relative.is_empty() {
        let (_, _, after) = accept::inspect(root, relative)?.context("directory disappeared")?;
        ensure!(
            scan::same_file(&before, &after),
            "directory replaced during discovery"
        );
    }
    Ok(())
}

fn discover(
    root: &Path,
    baseline: &Catalog,
    kind: Kind,
    paths: &[PathBuf],
    progress: &mut Progress<'_>,
) -> Result<BTreeMap<Vec<u8>, Action>> {
    let mut actions = BTreeMap::new();
    if kind == Kind::Mark {
        return Ok(actions);
    }
    let prefixes = if paths.is_empty() {
        vec![Vec::new()]
    } else {
        paths
            .iter()
            .map(|p| selector(p))
            .collect::<Result<Vec<_>>>()?
    };
    progress.begin("Discover selected changes", None)?;
    if kind == Kind::Move {
        let old = &prefixes[0];
        let new = &prefixes[1];
        let expected = baseline.get(old).context("OLD must name a tracked file")?;
        ensure!(!baseline.contains_key(new), "NEW must be untracked");
        ensure!(
            !baseline.policy.excludes(new, false),
            "NEW is outside recorded scope"
        );
        accept::absent(root, old)?;
        let observed = accept::observe(root, new, progress)?.0;
        ensure!(
            &observed == expected,
            "rename contents differ from OLD; mv cannot authorize an edit"
        );
        actions.insert(
            old.clone(),
            Action {
                old: Some(expected.clone()),
                new: None,
            },
        );
        actions.insert(
            new.clone(),
            Action {
                old: None,
                new: Some(observed),
            },
        );
    } else if kind == Kind::Update {
        for path in &prefixes {
            let old = baseline.get(path).context(
                "update requires explicitly named tracked files, not directories or additions",
            )?;
            let new = accept::observe(root, path, progress)?.0;
            if old != &new {
                actions.insert(
                    path.clone(),
                    Action {
                        old: Some(old.clone()),
                        new: Some(new),
                    },
                );
            }
        }
    } else {
        let mut files = BTreeSet::new();
        for prefix in &prefixes {
            let meta = if prefix.is_empty() {
                None
            } else {
                accept::inspect(root, prefix)?
            };
            let descendants = baseline.keys().any(|p| p != prefix && selected(p, prefix));
            let is_dir = prefix.is_empty()
                || meta.as_ref().is_some_and(|(_, _, m)| m.is_dir())
                || descendants;
            ensure!(
                prefix.is_empty() || !baseline.policy.excludes(prefix, is_dir),
                "selector is outside recorded scope: {}",
                encoding::encode(prefix)
            );
            if kind == Kind::Remove {
                ensure!(
                    prefix.is_empty()
                        || baseline.keys().any(|p| selected(p, prefix))
                        || meta.as_ref().is_some_and(|(_, _, m)| m.is_dir()),
                    "unknown deletion selector: {}",
                    encoding::encode(prefix)
                );
                if baseline.contains_key(prefix) {
                    accept::absent(root, prefix)?;
                }
            } else if prefix.is_empty() || meta.as_ref().is_some_and(|(_, _, m)| m.is_dir()) {
                walk(root, prefix, baseline, kind, &mut files)?;
            } else if let Some((_, _, meta)) = meta {
                ensure!(
                    meta.is_file(),
                    "selector must be a regular file or real directory"
                );
                ensure!(
                    kind != Kind::Add || !baseline.contains_key(prefix),
                    "file is already tracked; use keeper update for deliberate edits"
                );
                files.insert(prefix.clone());
            } else {
                ensure!(
                    kind == Kind::Snap && (baseline.contains_key(prefix) || descendants),
                    "unknown addition selector: {}",
                    encoding::encode(prefix)
                );
            }
        }
        for path in files {
            if kind == Kind::Add && baseline.contains_key(&path) {
                continue;
            }
            let old = baseline.get(&path).cloned();
            let new = accept::observe(root, &path, progress)?.0;
            if old.as_ref() != Some(&new) {
                actions.insert(
                    path,
                    Action {
                        old,
                        new: Some(new),
                    },
                );
            }
        }
        if matches!(kind, Kind::Remove | Kind::Snap) {
            for (path, old) in baseline {
                crate::cancel::check()?;
                if !prefixes.iter().any(|p| selected(path, p)) {
                    continue;
                }
                if accept::inspect(root, path)?.is_none() {
                    actions.insert(
                        path.clone(),
                        Action {
                            old: Some(old.clone()),
                            new: None,
                        },
                    );
                }
            }
        }
    }
    progress.finish()?;
    Ok(actions)
}

fn report(
    root: &Path,
    actions: &BTreeMap<Vec<u8>, Action>,
    kind: Kind,
    message: Option<&str>,
    options: &Options,
    out: &mut impl Write,
) -> Result<()> {
    writeln!(
        out,
        "Dataset: {}",
        encoding::encode(root.as_os_str().as_encoded_bytes())
    )?;
    if let Some(message) = message {
        writeln!(out, "Snapshot: {}", encoding::encode(message.as_bytes()))?;
    }
    let (mut add, mut change, mut remove, mut bytes) = (0u64, 0u64, 0u64, 0u64);
    let mut directories: BTreeMap<Vec<u8>, [u64; 3]> = BTreeMap::new();
    for (path, action) in actions {
        let (label, index) = match (&action.old, &action.new) {
            (None, Some(_)) => {
                add += 1;
                ("ADD", 0)
            }
            (Some(_), Some(_)) => {
                change += 1;
                ("UPDATE", 1)
            }
            (Some(_), None) => {
                remove += 1;
                ("REMOVE", 2)
            }
            _ => unreachable!(),
        };
        if let Some(new) = &action.new {
            bytes = bytes.saturating_add(new.size);
        }
        let parent = path
            .iter()
            .rposition(|&b| b == b'/')
            .map_or(b".".as_slice(), |i| &path[..i]);
        directories.entry(parent.to_vec()).or_default()[index] += 1;
        if options.long || kind == Kind::Move {
            writeln!(out, "{label}\t{}", encoding::encode(path))?;
            for (label, entry) in [("old", &action.old), ("new", &action.new)] {
                if let Some(e) = entry {
                    writeln!(out, "  {label}: {} ({})", e.hash, display::bytes(e.size))?;
                }
            }
        }
    }
    if !options.long && kind != Kind::Move {
        for (dir, counts) in directories {
            writeln!(
                out,
                "  {}: {} added, {} updated, {} removed",
                encoding::encode(&dir),
                display::number(counts[0]),
                display::number(counts[1]),
                display::number(counts[2])
            )?;
        }
    }
    writeln!(
        out,
        "{} added, {} updated, {} removed; {} of selected new bytes.",
        display::number(add),
        display::number(change),
        display::number(remove),
        display::bytes(bytes)
    )?;
    writeln!(
        out,
        "Other expectations remain unchanged. Dataset files are never moved, deleted or copied."
    )?;
    if matches!(kind, Kind::Snap | Kind::Mark) {
        writeln!(
            out,
            "Snapshots preserve paths and checksums, NOT file contents or recoverable copies."
        )?;
    }
    if kind == Kind::Mark {
        writeln!(out, "Marker only: live files and changes were NOT checked.")?;
    }
    if remove > 0 {
        writeln!(
            out,
            "Confirm these deletions are intentional, not a missing mount or disconnected subtree."
        )?;
    }
    Ok(())
}

fn confirmation() -> Result<bool> {
    let mut answer = Vec::new();
    loop {
        crate::cancel::check()?;
        let mut fd = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        // Poll rather than blocking in read_line: signals must cancel a prompt too.
        let ready = unsafe { libc::poll(&mut fd, 1, 100) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        if ready == 0 {
            continue;
        }
        let mut byte = 0u8;
        // SAFETY: byte is writable, and stdin is the terminal checked by run().
        let n = unsafe { libc::read(libc::STDIN_FILENO, (&mut byte as *mut u8).cast(), 1) };
        if n < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        if n == 0 {
            return Ok(false);
        }
        if byte == b'\n' {
            break;
        }
        answer.push(byte);
        ensure!(answer.len() <= 4096, "confirmation input too long");
    }
    crate::cancel::check()?;
    Ok(matches!(
        std::str::from_utf8(&answer)?.trim(),
        "y" | "Y" | "yes" | "YES"
    ))
}

pub fn run(
    root: &Path,
    kind: Kind,
    paths: Vec<PathBuf>,
    message: Option<&str>,
    options: &Options,
    out: &mut impl Write,
    progress: &mut Progress<'_>,
) -> Result<()> {
    ensure!(
        options.yes || options.dry_run || io::stdin().is_terminal(),
        "noninteractive acceptance requires --yes or --dry-run"
    );
    if let Some(message) = message {
        ensure!(
            !message.trim().is_empty(),
            "snapshot message must not be empty"
        );
    }
    // Read-only preview/decline must not create LOCK or other metadata. Discovery
    // pins expectations; acquire writer exclusion only after authorization, then
    // reject a changed parent and freshly revalidate the exact approved batch.
    let binding = fs::metadata(root)?;
    let migrated = intent::exists(root)?;
    let (initial, identity) = if migrated {
        let revision = intent::load(root)?;
        (revision.catalog, revision.checksum)
    } else {
        (
            catalog::load(root)
                .context("read baseline (use init first, or --root to select the dataset root)")?,
            intent::digest(&catalog::read_regular(
                &root.join(".keeper/catalog/MANIFEST"),
            )?),
        )
    };
    initial.policy.ensure_current(root)?;
    let actions = discover(root, &initial, kind, &paths, progress)?;
    let next = accept::updated(&initial, &actions)?;
    initial.policy.ensure_current(root)?;
    report(root, &actions, kind, message, options, out)?;
    if kind != Kind::Mark {
        if let [path] = paths.as_slice() {
            writeln!(
                out,
                "Selection (dataset-relative): {}",
                encoding::encode(path.as_os_str().as_encoded_bytes())
            )?;
        } else {
            writeln!(
                out,
                "Selection: {} explicit path(s); unselected expectations are inherited.",
                display::number(paths.len())
            )?;
        }
    }
    let snapshot = matches!(kind, Kind::Snap | Kind::Mark);
    if actions.is_empty() && !snapshot {
        writeln!(out, "Nothing to accept. Baseline unchanged.")?;
        return Ok(());
    }
    if !migrated {
        writeln!(
            out,
            "Will preserve the original catalog and enable history automatically. Do not use older Keeper builds on this dataset afterward."
        )?;
    }
    if options.dry_run {
        writeln!(out, "Dry run: no metadata changed.")?;
        return Ok(());
    }
    if !options.yes {
        write!(
            out,
            "{} [y/N] ",
            if snapshot {
                "Record this snapshot?"
            } else {
                "Accept these changes?"
            }
        )?;
        out.flush()?;
        if !confirmation()? {
            writeln!(out, "Nothing accepted. Baseline unchanged.")?;
            return Ok(());
        }
    }
    out.flush()?;
    let _guard = catalog::InitGuard::lock(root)?;
    let after = fs::metadata(root)?;
    ensure!(
        binding.dev() == after.dev() && binding.ino() == after.ino(),
        "physical dataset root changed during review"
    );
    ensure!(
        intent::exists(root)? == migrated,
        "baseline changed during review; retry and review again"
    );
    let (current, checksum) = if migrated {
        let r = intent::load(root)?;
        (r.catalog, r.checksum)
    } else {
        (
            catalog::load(root)?,
            intent::digest(&catalog::read_regular(
                &root.join(".keeper/catalog/MANIFEST"),
            )?),
        )
    };
    ensure!(
        current == initial && checksum == identity,
        "baseline changed during review; retry and review again"
    );
    current.policy.ensure_current(root)?;
    accept::revalidate(root, &actions, progress)?;
    current.policy.ensure_current(root)?;
    crate::cancel::check()?;
    let parent = if migrated {
        intent::load(root)?
    } else {
        intent::migrate_locked(root).context("automatic migration failed; original expectations are preserved; inspect catalog check or catalog migrate for recovery")?
    };
    ensure!(parent.catalog == initial, "migration changed expectations");
    ensure!(
        !migrated || parent.checksum == identity,
        "active parent changed before publication"
    );
    let revision = intent::publish_named(root, &parent, &next, message)
        .context("could not finish publication; inspect catalog check before retrying (automatic migration may already have completed)")?;
    writeln!(
        out,
        "{} History retained (R{}).",
        if snapshot {
            "Snapshot recorded."
        } else {
            "Changes accepted."
        },
        revision.number
    )?;
    Ok(())
}

fn date(nanoseconds: u128) -> Result<String> {
    let seconds: libc::time_t = (nanoseconds / 1_000_000_000)
        .try_into()
        .context("snapshot time out of range")?;
    let mut time = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: valid pointers to the input time and writable tm storage.
    ensure!(
        !unsafe { libc::gmtime_r(&seconds, time.as_mut_ptr()) }.is_null(),
        "snapshot time cannot be displayed"
    );
    let mut text = [0u8; 64];
    // SAFETY: gmtime_r succeeded; buffer has the supplied capacity.
    let len = unsafe {
        libc::strftime(
            text.as_mut_ptr().cast(),
            text.len(),
            c"%Y-%m-%dT%H:%M:%SZ".as_ptr(),
            time.as_ptr(),
        )
    };
    ensure!(len > 0, "snapshot date formatting failed");
    Ok(std::str::from_utf8(&text[..len])?.to_owned())
}

pub fn log(root: &Path, out: &mut impl Write) -> Result<()> {
    if !intent::exists(root)? {
        catalog::load(root)?;
        writeln!(out, "Legacy baseline; no named snapshots yet.")?;
        return Ok(());
    }
    let mut revision = intent::load(root)?;
    loop {
        writeln!(
            out,
            "R{}\t{}\t{}",
            revision.number,
            date(revision.recorded_ns)?,
            revision
                .message
                .as_ref()
                .map(|m| encoding::encode(m.as_bytes()))
                .unwrap_or_else(|| "(accepted expectations)".into())
        )?;
        if revision.parent == 0 {
            break;
        }
        revision = intent::read_revision(
            &root
                .join(".keeper/intent/revisions")
                .join(revision.parent.to_string()),
        )?;
    }
    Ok(())
}
