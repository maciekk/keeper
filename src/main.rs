#[cfg(not(target_os = "linux"))]
compile_error!("Keeper currently supports Linux only.");

mod cancel;
mod catalog;
mod checkpoint;
mod compare;
mod encoding;
mod progress;
mod scan;

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use progress::Progress;
use std::{
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

#[derive(Parser)]
#[command(
    version,
    about = "An inspectable integrity catalog. Never silently accepts changes.",
    after_help = "Command syntax:\n  init [ROOT] [--resume]       Record a baseline of current bytes\n  status [ROOT] [--resume]     Compare file bytes against the baseline\n  preflight [ROOT]            Inspect metadata without reading contents\n  compare LEFT RIGHT [--live] Compare catalogs, or freshly read both trees\n  catalog check [ROOT]        Validate catalog metadata only\n  help [COMMAND]...           Show detailed help (e.g. help catalog check)\n\nROOT defaults to the current directory. Use 'keeper help <command>' for details."
)]
struct Cli {
    /// Progress on stderr: auto enables it only on a terminal.
    #[arg(long, value_enum, default_value = "auto", global = true)]
    progress: ProgressMode,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum ProgressMode {
    Auto,
    Always,
    Never,
}

#[derive(Subcommand)]
enum Command {
    /// Record the current bytes as a baseline; does NOT certify that they are good.
    #[command(
        long_about = "Record current regular-file bytes in a new baseline. This does NOT certify semantic validity. Refuses to overwrite an existing catalog. Stop dataset writers first.",
        after_help = "Examples:\n  keeper init /data/photos\n  keeper init /data/photos --resume\n\n--resume reuses checkpointed per-path reads; it is NOT a fresh full scrub."
    )]
    Init {
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Continue a checkpointed session (reuses earlier per-path reads, not a fresh scrub).
        #[arg(long)]
        resume: bool,
    },
    /// Reread every byte by default and compare with the baseline; never accept changes.
    #[command(
        long_about = "Reread every regular file and compare with the baseline without accepting changes. Reports added, missing, and changed paths. Stop dataset writers first.",
        after_help = "Examples:\n  keeper status /data/photos\n  keeper status /data/photos --resume\n\nExit codes: 0 unchanged, 1 differences, 2 operational error.\n--resume reuses earlier reads; it is NOT a fresh full scrub."
    )]
    Status {
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Continue a checkpointed session; writes disposable work state, never the baseline.
        #[arg(long)]
        resume: bool,
    },
    /// Inventory metadata and list problems without reading file contents or creating a catalog.
    #[command(
        long_about = "Inventory regular-file counts and sizes and list unsupported objects or traversal problems. Does not read file contents, create a catalog, certify readability, or test publication support.",
        after_help = "Example: keeper preflight /data/photos\nExit codes: 0 no issues, 2 issues or operational error."
    )]
    Preflight {
        #[arg(default_value = ".")]
        root: PathBuf,
    },
    /// Compare recorded catalogs, or reread both trees with --live. Path-sensitive and read-only.
    #[command(
        long_about = "Compare paths and full content IDs in two recorded catalogs. With --live, freshly read both trees without using or changing catalogs. Roots must be distinct and non-overlapping. Stop writers on both trees; sequential scans are not a simultaneous snapshot.",
        after_help = "Examples:\n  keeper compare /data/primary /mnt/backup\n  keeper compare /data/primary /mnt/backup --live\n\nExit codes: 0 matching, 1 differences, 2 operational error."
    )]
    Compare {
        left: PathBuf,
        right: PathBuf,
        /// Read all current file bytes; do not use or change catalogs. No catalog required.
        #[arg(long)]
        live: bool,
    },
    /// Inspect authoritative catalog metadata.
    Catalog {
        #[command(subcommand)]
        command: CatalogCommand,
    },
}

#[derive(Subcommand)]
enum CatalogCommand {
    /// Check checksums, canonical format, and path/object consistency; does not read dataset files.
    #[command(
        long_about = "Validate catalog checksums, canonical encoding, and path/object consistency. Dataset file contents are NOT read. Checksums detect accidental catalog damage, not coordinated malicious edits.",
        after_help = "Example: keeper catalog check /data/photos"
    )]
    Check {
        #[arg(default_value = ".")]
        root: PathBuf,
    },
}

fn root(path: &Path) -> Result<PathBuf> {
    let root = path.canonicalize().context("resolve dataset root")?;
    ensure!(root.is_dir(), "dataset root must be a directory");
    Ok(root)
}

fn run(cli: Cli, out: &mut impl Write, progress: &mut Progress<'_>) -> Result<u8> {
    match cli.command {
        Command::Init { root: path, resume } => {
            let root = root(&path)?;
            let guard = catalog::InitGuard::acquire(&root)?;
            let mut work = resume
                .then(|| checkpoint::Checkpoint::open(&root, "init", None))
                .transpose()?;
            let catalog = scan_session(&root, progress, work.as_mut(), out)?;
            cancel::check()?;
            guard.publish(&catalog)?;
            if let Some(work) = work {
                work.finish()?;
            }
            writeln!(out, "Created catalog: {} file(s).", catalog.len())?;
            writeln!(
                out,
                "Baseline records current bytes, not proof of semantic validity. Keep an independent copy of .keeper/catalog/."
            )?;
            Ok(0)
        }
        Command::Catalog {
            command: CatalogCommand::Check { root: path },
        } => {
            let catalog = catalog::load(&root(&path)?)?;
            writeln!(
                out,
                "Catalog OK: {} file(s). Dataset bytes were not checked.",
                catalog.len()
            )?;
            Ok(0)
        }
        Command::Preflight { root: path } => {
            let inventory = scan::preflight(&root(&path)?, progress)?;
            for issue in &inventory.issues {
                writeln!(out, "{issue}")?;
            }
            writeln!(
                out,
                "Preflight: {} regular file(s), {} bytes, {} issue(s). Contents not read; file readability and publication support not checked.",
                inventory.file_count(),
                inventory.bytes,
                inventory.issues.len()
            )?;
            Ok(if inventory.issues.is_empty() { 0 } else { 2 })
        }
        Command::Compare { left, right, live } => {
            let left = root(&left).context("left dataset")?;
            let right = root(&right).context("right dataset")?;
            ensure!(
                !left.starts_with(&right) && !right.starts_with(&left),
                "comparison roots must be distinct and non-overlapping (including resolved symlinks)"
            );
            let (left_catalog, right_catalog) = if live {
                // Preflight BOTH before hashing either: discover avoidable late
                // failures before spending hours reading the first drive.
                progress.set_scope("Left ");
                let left_inventory = scan::preflight(&left, progress).context("left preflight")?;
                progress.set_scope("Right ");
                let right_inventory =
                    scan::preflight(&right, progress).context("right preflight")?;
                let issues: Vec<_> = [("left", &left_inventory), ("right", &right_inventory)]
                    .into_iter()
                    .filter_map(|(side, inventory)| {
                        inventory
                            .ensure_clean()
                            .err()
                            .map(|error| format!("{side} dataset: {error:#}"))
                    })
                    .collect();
                ensure!(issues.is_empty(), "{}", issues.join("\n"));
                progress.set_scope("Left ");
                let left_catalog = scan::hash_inventory(&left, left_inventory, progress)
                    .context("left dataset")?;
                progress.set_scope("Right ");
                let right_catalog = scan::hash_inventory(&right, right_inventory, progress)
                    .context("right dataset")?;
                writeln!(
                    out,
                    "Scope: freshly read file bytes; catalogs were not used. Sequential scans, not a simultaneous snapshot."
                )?;
                (left_catalog, right_catalog)
            } else {
                let left_catalog = catalog::load(&left).context("left catalog")?;
                let right_catalog = catalog::load(&right).context("right catalog")?;
                writeln!(
                    out,
                    "Scope: recorded catalogs only. Current dataset bytes were NOT checked."
                )?;
                (left_catalog, right_catalog)
            };
            Ok(u8::from(compare::report(
                &left_catalog,
                &right_catalog,
                out,
            )?))
        }
        Command::Status { root: path, resume } => {
            let root = root(&path)?;
            // Validate metadata before reading potentially terabytes of data.
            let _guard = resume
                .then(|| catalog::InitGuard::lock(&root))
                .transpose()?;
            let expected = catalog::load(&root)?;
            let mut work = resume
                .then(|| checkpoint::Checkpoint::open(&root, "status", Some(&expected)))
                .transpose()?;
            let observed = scan_session(&root, progress, work.as_mut(), out)?;
            let (mut added, mut missing, mut changed) = (0, 0, 0);
            for (path, entry) in &expected {
                let label = encoding::encode(path);
                match observed.get(path) {
                    None => {
                        missing += 1;
                        writeln!(out, "MISSING\t{label}")?;
                    }
                    Some(current) if entry != current => {
                        changed += 1;
                        writeln!(
                            out,
                            "CHANGED\t{label}\texpected={}\tobserved={}",
                            entry.hash, current.hash
                        )?;
                    }
                    _ => {}
                }
            }
            for path in observed.keys().filter(|p| !expected.contains_key(*p)) {
                added += 1;
                writeln!(out, "ADDED\t{}", encoding::encode(path))?;
            }
            writeln!(
                out,
                "{}: {} observed file(s); {added} added, {missing} missing, {changed} changed. Baseline unchanged.",
                if resume {
                    "Checkpointed session"
                } else {
                    "Full-byte check"
                },
                observed.len()
            )?;
            out.flush()?;
            if let Some(work) = work {
                work.finish()?;
            }
            Ok(u8::from(added + missing + changed > 0))
        }
    }
}

fn scan_session(
    root: &Path,
    progress: &mut Progress<'_>,
    work: Option<&mut checkpoint::Checkpoint>,
    out: &mut impl Write,
) -> Result<catalog::Catalog> {
    match work {
        None => scan::scan(root, progress),
        Some(work) => {
            writeln!(
                out,
                "Scope: checkpointed multi-invocation session {}. Earlier per-path reads may be reused; NOT a fresh full scrub. Metadata cannot detect silent corruption. Keep writers stopped throughout the session.",
                work.session
            )?;
            out.flush()?;
            let inventory = scan::preflight(root, progress)?;
            let catalog = scan::hash_inventory_checkpointed(root, inventory, progress, Some(work))?;
            writeln!(
                out,
                "Session observations: {} file(s) reused from earlier reads; {} file(s) freshly read this invocation.",
                work.reused,
                catalog.len() as u64 - work.reused
            )?;
            Ok(catalog)
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Err(error) = cancel::install() {
        eprintln!("keeper: install cancellation handlers: {error}");
        return ExitCode::from(2);
    }
    let enabled = match cli.progress {
        ProgressMode::Auto => io::stderr().is_terminal(),
        ProgressMode::Always => true,
        ProgressMode::Never => false,
    };
    let mut err = io::stderr().lock();
    let mut progress = Progress::new(&mut err, enabled);
    let mut out = io::BufWriter::new(io::stdout().lock());
    match run(cli, &mut out, &mut progress).and_then(|code| {
        out.flush()?;
        Ok(code)
    }) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("keeper: {error:#}");
            ExitCode::from(2)
        }
    }
}
