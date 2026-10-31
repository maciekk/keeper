#[cfg(not(target_os = "linux"))]
compile_error!("Keeper currently supports Linux only.");

mod cancel;
mod catalog;
mod checkpoint;
mod compare;
mod display;
mod encoding;
mod policy;
mod progress;
mod scan;

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use display::{label, number};
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
    after_help = "Command syntax:\n  init [ROOT] [--resume]       Record a baseline of current bytes\n  status [ROOT] [--resume]     Compare file bytes against the baseline\n  preflight [ROOT]            Inspect metadata without reading contents\n  compare LEFT RIGHT [--live] Compare catalogs; --live reads bytes, --metadata-only checks paths/sizes\n  catalog check [ROOT]        Validate catalog metadata only\n  help [COMMAND]...           Show detailed help (e.g. help catalog check)\n\nROOT defaults to the current directory. Root-level .keeperignore defines explicit exclusions. Use 'keeper help <command>' for details."
)]
struct Cli {
    /// Progress on stderr: auto enables it only on a terminal.
    #[arg(long, value_enum, default_value = "auto", global = true)]
    progress: ProgressMode,
    /// Colour and emphasis: auto uses a terminal, respecting NO_COLOR.
    #[arg(long, value_enum, default_value = "auto", global = true)]
    color: ProgressMode,
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
        long_about = "Record current regular-file bytes in a new baseline. This does NOT certify semantic validity. Refuses to overwrite an existing catalog. Root-level .keeperignore exclusions are recorded with the baseline. Stop dataset writers first.",
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
        long_about = "Reread every regular file and compare with the baseline without accepting changes. Reports added, missing, and changed paths. Requires the exact .keeperignore policy recorded at initialization; editing it cannot hide tracked files. Stop dataset writers first.",
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
        long_about = "Inventory regular-file counts and sizes and list unsupported objects or traversal problems. Reads root-level .keeperignore if present, but no dataset contents. Does not create a catalog, certify readability, or test publication support.",
        after_help = "Example: keeper preflight /data/photos\nExit codes: 0 no issues, 2 issues or operational error."
    )]
    Preflight {
        #[arg(default_value = ".")]
        root: PathBuf,
    },
    /// Compare catalogs, live bytes, or metadata only. Path-sensitive and read-only.
    #[command(
        long_about = "Compare paths and full content IDs in two recorded catalogs. With --live, freshly read both trees without using or changing catalogs. With --metadata-only, compare current regular-file paths and sizes without opening files or using catalogs; equal sizes do NOT establish content equality. Empty directories and metadata such as timestamps are not compared. By default, report implicated directories with counts of differing files grouped by immediate parent (not recursive totals); --long also lists individual file differences. Root-level .keeperignore explicitly selects scan scope; recorded comparison uses saved policies, and differing policies return 2. Junk classification alone never excludes paths and is only a name-based heuristic, not a judgment of data value. Roots must be distinct and non-overlapping. Stop writers on both trees; sequential scans are not a simultaneous snapshot.",
        after_help = "Examples:\n  keeper compare /data/primary /mnt/backup\n  keeper compare /data/primary /mnt/backup --live\n  keeper compare /data/primary /mnt/backup --metadata-only\n\nExit codes: 0 matching selected scope, 1 differences, 2 operational error or scope-policy mismatch."
    )]
    Compare {
        left: PathBuf,
        right: PathBuf,
        /// Read all current file bytes; do not use or change catalogs. No catalog required.
        #[arg(long, conflicts_with = "metadata_only")]
        live: bool,
        /// Compare current file paths and sizes only; no hashing, catalogs, or writes.
        #[arg(long)]
        metadata_only: bool,
        /// Add individual file differences to the default per-directory summary.
        #[arg(long)]
        long: bool,
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

fn run(cli: Cli, out: &mut impl Write, progress: &mut Progress<'_>, color: bool) -> Result<u8> {
    match cli.command {
        Command::Init { root: path, resume } => {
            let root = root(&path)?;
            let guard = catalog::InitGuard::acquire(&root)?;
            let mut work = resume
                .then(|| checkpoint::Checkpoint::open(&root, "init", None))
                .transpose()?;
            let catalog = scan_session(&root, progress, work.as_mut(), out, color)?;
            cancel::check()?;
            catalog.policy.ensure_current(&root)?;
            guard.publish(&catalog)?;
            if let Some(work) = work {
                work.finish()?;
            }
            writeln!(
                out,
                "{}: {} file(s).",
                label("Created catalog", color),
                number(catalog.len())
            )?;
            display::summary(out, "Recorded data", &catalog, color)?;
            writeln!(
                out,
                "Baseline records current bytes, not proof of semantic validity.\nKeep an independent copy of .keeper/catalog/."
            )?;
            Ok(0)
        }
        Command::Catalog {
            command: CatalogCommand::Check { root: path },
        } => {
            let catalog = catalog::load(&root(&path)?)?;
            writeln!(
                out,
                "Catalog OK: {} file(s).\nDataset bytes were not checked.",
                number(catalog.len())
            )?;
            display::summary(out, "Recorded data", &catalog, color)?;
            for note in catalog.policy.notes(None) {
                display::note(out, &format!("- {note}"), color)?;
            }
            Ok(0)
        }
        Command::Preflight { root: path } => {
            let inventory = scan::preflight(&root(&path)?, progress)?;
            for issue in &inventory.issues {
                writeln!(out, "{}", display::label(issue, color))?;
            }
            for note in inventory.policy.notes(Some(inventory.excluded.len())) {
                display::note(out, &format!("- {note}"), color)?;
            }
            if !inventory.issues.is_empty() {
                display::section(out)?;
            }
            writeln!(
                out,
                "{}: {} regular file(s), {}, {} issue(s).\nContents not read; file readability and publication support not checked.",
                label("Preflight", color),
                display::emphasis(&number(inventory.file_count()), color),
                display::emphasis(&display::bytes(inventory.bytes), color),
                display::emphasis(&number(inventory.issues.len()), color)
            )?;
            Ok(if inventory.issues.is_empty() { 0 } else { 2 })
        }
        Command::Compare {
            left,
            right,
            live,
            metadata_only,
            long,
        } => {
            let roots =
                [&left, &right].map(|path| encoding::encode(path.as_os_str().as_encoded_bytes()));
            let left = root(&left).context("left dataset")?;
            let right = root(&right).context("right dataset")?;
            ensure!(
                !left.starts_with(&right) && !right.starts_with(&left),
                "comparison roots must be distinct and non-overlapping (including resolved symlinks)"
            );
            if metadata_only {
                progress.set_scope("Left ");
                let left_inventory = scan::preflight(&left, progress).context("left preflight")?;
                progress.set_scope("Right ");
                let right_inventory =
                    scan::preflight(&right, progress).context("right preflight")?;
                return Ok(compare::report_metadata(
                    &left_inventory,
                    &right_inventory,
                    out,
                    color,
                    long,
                    roots,
                )?);
            }
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
                (left_catalog, right_catalog)
            } else {
                let left_catalog = catalog::load(&left).context("left catalog")?;
                let right_catalog = catalog::load(&right).context("right catalog")?;
                (left_catalog, right_catalog)
            };
            Ok(compare::report(
                &left_catalog,
                &right_catalog,
                out,
                color,
                long,
                live,
                roots,
            )?)
        }
        Command::Status { root: path, resume } => {
            let root = root(&path)?;
            // Validate metadata before reading potentially terabytes of data.
            let _guard = resume
                .then(|| catalog::InitGuard::lock(&root))
                .transpose()?;
            let expected = catalog::load(&root)?;
            ensure!(
                expected.policy == policy::Policy::load(&root)?,
                ".keeperignore differs from recorded catalog scope; restore the original policy before status (never delete a catalog to change scope)"
            );
            let mut work = resume
                .then(|| checkpoint::Checkpoint::open(&root, "status", Some(&expected)))
                .transpose()?;
            let observed = scan_session(&root, progress, work.as_mut(), out, color)?;
            ensure!(
                expected.policy == observed.policy,
                "scan scope differs from recorded catalog scope"
            );
            let (mut added, mut missing, mut changed) = (0, 0, 0);
            for (path, entry) in &expected {
                let label = encoding::encode(path);
                match observed.get(path) {
                    None => {
                        missing += 1;
                        writeln!(out, "{}\t{label}", display::label("MISSING", color))?;
                    }
                    Some(current) if entry != current => {
                        changed += 1;
                        writeln!(
                            out,
                            "{}\t{label}\texpected={}\tobserved={}\texpected_size={}\tobserved_size={}",
                            display::label("CHANGED", color),
                            entry.hash,
                            current.hash,
                            display::bytes(entry.size),
                            display::bytes(current.size)
                        )?;
                    }
                    _ => {}
                }
            }
            for path in observed.keys().filter(|p| !expected.contains_key(*p)) {
                added += 1;
                writeln!(out, "{}\t{}", label("ADDED", color), encoding::encode(path))?;
            }
            if added + missing + changed > 0 {
                display::section(out)?;
            }
            writeln!(
                out,
                "{}: {} observed file(s); {} added, {} missing, {} changed.\nBaseline unchanged.",
                if resume {
                    "Checkpointed session"
                } else {
                    "Full-byte check"
                },
                number(observed.len()),
                number(added),
                number(missing),
                number(changed)
            )?;
            display::summary(out, "Observed data", &observed, color)?;
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
    color: bool,
) -> Result<catalog::Catalog> {
    let inventory = scan::preflight(root, progress)?;
    for note in inventory.policy.notes(Some(inventory.excluded.len())) {
        display::note(out, &format!("- {note}"), color)?;
    }
    out.flush()?;
    match work {
        None => scan::hash_inventory(root, inventory, progress),
        Some(work) => {
            display::note(
                out,
                &format!(
                    "Scope: checkpointed multi-invocation session {}.\n  - Earlier per-path reads may be reused; NOT a fresh full scrub.\n  - Metadata cannot detect silent corruption.\n  - Keep writers stopped throughout the session.",
                    work.session
                ),
                color,
            )?;
            out.flush()?;
            let catalog = scan::hash_inventory_checkpointed(root, inventory, progress, Some(work))?;
            writeln!(
                out,
                "Session observations:\n  - {} file(s) reused from earlier reads.\n  - {} file(s) freshly read this invocation.",
                number(work.reused),
                number(catalog.len() as u64 - work.reused)
            )?;
            Ok(catalog)
        }
    }
}

fn main() -> ExitCode {
    // One opening separator for every invocation, including clap help/errors.
    // Flush before parsing or progress so stdout/stderr cannot reorder it.
    let opening = {
        let mut out = io::stdout().lock();
        writeln!(out).and_then(|_| out.flush())
    };
    if let Err(error) = opening {
        eprintln!("keeper: write output: {error}");
        return ExitCode::from(2);
    }
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
    let color = match cli.color {
        ProgressMode::Auto => io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        ProgressMode::Always => true,
        ProgressMode::Never => false,
    };
    let progress_color = match cli.color {
        ProgressMode::Auto => io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        ProgressMode::Always => true,
        ProgressMode::Never => false,
    };
    let mut err = io::stderr().lock();
    let mut progress = Progress::new(&mut err, enabled)
        .terminal(io::stderr().is_terminal())
        .color(progress_color);
    let mut out = io::BufWriter::new(io::stdout().lock());
    let result = run(cli, &mut out, &mut progress, color).and_then(|code| {
        out.flush()?;
        Ok(code)
    });
    drop(progress);
    drop(err);
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("keeper: {error:#}");
            ExitCode::from(2)
        }
    }
}
