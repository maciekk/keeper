#[cfg(not(target_os = "linux"))]
compile_error!("Keeper currently supports Linux only.");

mod accept;
mod cancel;
mod catalog;
mod checkpoint;
mod compare;
mod display;
mod encoding;
mod filesystem;
mod intent;
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
    time::Duration,
};

#[derive(Parser)]
#[command(
    version,
    about = "An inspectable integrity catalog. Never silently accepts changes.",
    after_help = "Command syntax:\n  init [ROOT] [--resume]       Record a baseline of current bytes\n  status [ROOT] [--resume]     Compare file bytes against the baseline\n  preflight [ROOT]            Inspect metadata without reading contents\n  compare LEFT RIGHT [--live] Compare catalogs; --live reads bytes, --metadata-only checks paths/sizes\n  catalog check [ROOT]        Validate catalog metadata only\n  catalog export ROOT DEST   Save a catalog for offline comparison\n  catalog migrate [ROOT]      Migrate metadata to immutable intent R1\n  accept plan ROOT PLAN      Review explicit --add/--replace/--remove paths\n  accept show PLAN           Inspect saved actions without accessing data\n  accept apply ROOT PLAN     Publish reviewed changes; revalidate selected bytes\n  help [COMMAND]...           Show detailed help (e.g. help catalog check)\n\nROOT defaults to the current directory. Root-level .keeperignore defines explicit exclusions. Use 'keeper help <command>' for details."
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
        long_about = "Record current regular-file bytes in a new baseline. This does NOT certify semantic validity. If a catalog already exists, validates it and shows its recorded summary without rescanning or overwriting it. Root-level .keeperignore exclusions are recorded with the baseline. Stop dataset writers first.",
        after_help = "Examples:\n  keeper init /data/photos\n  keeper init /data/photos --resume\n\n--resume reuses checkpointed per-path reads; it is NOT a fresh full scrub."
    )]
    Init {
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Continue a checkpointed session (NTFS/FAT/exFAT/FUSE reread all files).
        #[arg(long)]
        resume: bool,
        /// Flush resume work every N seconds, checked at file completion (default: 5).
        #[arg(long, value_name = "SECONDS", requires = "resume", value_parser = clap::value_parser!(u64).range(1..))]
        checkpoint_interval: Option<u64>,
    },
    /// Reread every byte by default and compare with the baseline; never accept changes.
    #[command(
        long_about = "Reread every regular file and compare with the baseline without accepting changes. Reports added, missing, and changed paths. Requires the exact .keeperignore policy recorded at initialization; editing it cannot hide tracked files. Stop dataset writers first.",
        after_help = "Examples:\n  keeper status /data/photos\n  keeper status /data/photos --resume\n\nExit codes: 0 unchanged, 1 differences, 2 operational error.\n--resume reuses earlier reads; it is NOT a fresh full scrub."
    )]
    Status {
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Continue a checkpointed session; never changes the baseline (NTFS/FAT/exFAT/FUSE reread all files).
        #[arg(long)]
        resume: bool,
        /// Flush resume work every N seconds, checked at file completion (default: 5).
        #[arg(long, value_name = "SECONDS", requires = "resume", value_parser = clap::value_parser!(u64).range(1..))]
        checkpoint_interval: Option<u64>,
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
        long_about = "Compare paths and full content IDs in two recorded catalogs. By default LEFT and RIGHT are dataset roots, and only their .keeper/catalog directories are read. With --catalogs, LEFT and RIGHT are standalone catalog directories (exported with 'catalog export' or copied from .keeper/catalog); no original drive is accessed or required. With --live, freshly read both trees without using or changing catalogs. With --metadata-only, compare current regular-file paths and sizes without opening files or using catalogs; equal sizes do NOT establish content equality. Empty directories and metadata such as timestamps are not compared. By default, report implicated directories with counts of differing files grouped by immediate parent (not recursive totals); --long also lists individual file differences. Root-level .keeperignore explicitly selects scan scope; recorded comparison uses saved policies, and differing policies return 2. Junk classification alone never excludes paths and is only a name-based heuristic, not a judgment of data value. Roots must be distinct and non-overlapping. Stop writers on both trees; sequential scans are not a simultaneous snapshot.",
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
        /// Read standalone catalog directories, not dataset roots. Works with drives disconnected.
        #[arg(long, conflicts_with_all = ["live", "metadata_only"])]
        catalogs: bool,
        /// Add individual file differences to the default per-directory summary.
        #[arg(long)]
        long: bool,
    },
    /// Review and selectively accept changes into immutable intent revisions.
    Accept {
        #[command(subcommand)]
        command: AcceptCommand,
    },
    /// Inspect authoritative catalog metadata.
    Catalog {
        #[command(subcommand)]
        command: CatalogCommand,
    },
}

#[derive(Subcommand)]
enum AcceptCommand {
    /// Read only selected paths and save an exact-parent plan. Paths are relative to ROOT.
    #[command(
        after_help = "Example: keeper accept plan /data/photos review.plan --add new.jpg\nRepeat --add, --replace, or --remove for each explicit path. Rename is remove + add.\nNo globs, recursive enrollment, implicit deletions, or scope changes. Stop writers."
    )]
    Plan {
        root: PathBuf,
        destination: PathBuf,
        #[arg(long, value_name = "PATH")]
        add: Vec<PathBuf>,
        #[arg(long, value_name = "PATH")]
        replace: Vec<PathBuf>,
        #[arg(long, value_name = "PATH")]
        remove: Vec<PathBuf>,
    },
    /// Display exact actions, hashes and sizes without accessing the dataset.
    Show { plan: PathBuf },
    /// Authorize a reviewed plan; reread selected bytes and publish all actions or none.
    #[command(
        after_help = "Example: keeper accept apply /data/photos review.plan\nStop writers throughout. Inherited files are not reread. Stale plans fail, never rebase.\nA sync failure after switching CURRENT is uncertain publication: inspect catalog check before retrying."
    )]
    Apply { root: PathBuf, plan: PathBuf },
}

#[derive(Subcommand)]
enum CatalogCommand {
    /// Explicit metadata-only migration to R1; repeat to recover an interrupted migration.
    #[command(
        after_help = "Example: keeper catalog migrate /data/photos\nPreserves the original in .keeper/legacy-catalog. No dataset contents are read.\nDo not use older Keeper versions on migrated roots. Back up the entire intent store for history."
    )]
    Migrate {
        #[arg(default_value = ".")]
        root: PathBuf,
    },
    /// Export a validated baseline to a new local directory for offline comparison; no rehashing.
    #[command(
        after_help = "Example: keeper catalog export /mnt/DATA2 ~/catalogs/DATA2\nThe destination parent must exist. Existing destinations are never overwritten."
    )]
    Export { root: PathBuf, destination: PathBuf },
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

fn load_standalone(dir: &Path, out: &mut impl Write) -> Result<catalog::Catalog> {
    if intent::is_revision(dir)? {
        let revision = intent::read_revision(dir)?;
        writeln!(
            out,
            "Exported intent R{} ({}) at {}",
            revision.number,
            revision.checksum,
            encoding::encode(dir.as_os_str().as_encoded_bytes())
        )?;
        Ok(revision.catalog)
    } else {
        catalog::read_generation(dir)
    }
}

fn load_baseline(root: &Path, out: &mut impl Write) -> Result<catalog::Catalog> {
    if intent::exists(root)? {
        let revision = intent::load(root)?;
        writeln!(
            out,
            "Intent R{} ({}) at {}",
            revision.number,
            revision.checksum,
            encoding::encode(root.as_os_str().as_encoded_bytes())
        )?;
        Ok(revision.catalog)
    } else {
        catalog::load(root)
    }
}

fn run(cli: Cli, out: &mut impl Write, progress: &mut Progress<'_>, color: bool) -> Result<u8> {
    match cli.command {
        Command::Accept { command } => {
            match command {
                AcceptCommand::Plan {
                    root: path,
                    destination,
                    add,
                    replace,
                    remove,
                } => {
                    accept::plan(&root(&path)?, &destination, add, replace, remove, progress)?;
                    accept::show(&destination, out)?;
                    writeln!(
                        out,
                        "Plan saved. Review with keeper accept show; authorize with keeper accept apply."
                    )?;
                }
                AcceptCommand::Show { plan } => accept::show(&plan, out)?,
                AcceptCommand::Apply { root: path, plan } => {
                    let revision = accept::apply(&root(&path)?, &plan, progress)?;
                    writeln!(
                        out,
                        "Published intent R{} ({}) with {} file(s).\nPrior revisions preserved. Inherited files were NOT reread; no fresh verification is implied.",
                        revision.number,
                        revision.checksum,
                        number(revision.catalog.len())
                    )?;
                }
            }
            Ok(0)
        }
        Command::Catalog {
            command: CatalogCommand::Migrate { root: path },
        } => {
            let revision = intent::migrate(&root(&path)?)?;
            writeln!(
                out,
                "Migrated baseline to intent R{} ({}). Dataset bytes were not read.\nOriginal retained in .keeper/legacy-catalog. Back up .keeper/intent/ to preserve history.\nDo not use older Keeper builds on this root.",
                revision.number, revision.checksum
            )?;
            Ok(0)
        }
        Command::Init {
            root: path,
            resume,
            checkpoint_interval,
        } => {
            let root = root(&path)?;
            let guard = catalog::InitGuard::lock(&root)?;
            if intent::exists(&root)? || guard.catalog_exists()? {
                let catalog = catalog::load(&root)
                    .context("catalog already exists but could not be read; init never overwrites a baseline")?;
                writeln!(
                    out,
                    "{}: {} file(s). Baseline left unchanged.",
                    label("Catalog already exists", color),
                    number(catalog.len())
                )?;
                display::summary(out, "Recorded data", &catalog, color)?;
                writeln!(
                    out,
                    "Dataset bytes were not checked. Run keeper status to verify current bytes."
                )?;
                return Ok(0);
            }
            let mut work = resume
                .then(|| {
                    checkpoint::Checkpoint::open(&root, "init", None).map(|work| {
                        work.with_sync_interval(Duration::from_secs(
                            checkpoint_interval.unwrap_or(5),
                        ))
                    })
                })
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
            let root = root(&path)?;
            let catalog = load_baseline(&root, out)?;
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
        Command::Catalog {
            command:
                CatalogCommand::Export {
                    root: path,
                    destination,
                },
        } => {
            let root = root(&path)?;
            let catalog = if intent::exists(&root)? {
                let revision = intent::export(&root, &destination)?;
                writeln!(
                    out,
                    "Exported intent R{} ({}) from {}. History and physical observations are not included.",
                    revision.number,
                    revision.checksum,
                    encoding::encode(root.as_os_str().as_encoded_bytes())
                )?;
                revision.catalog
            } else {
                let catalog = catalog::load(&root)?;
                catalog::export(&catalog, &destination)?;
                catalog
            };
            writeln!(
                out,
                "Exported catalog: {} file(s). Dataset bytes were not read.",
                number(catalog.len())
            )?;
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
            catalogs,
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
            let (left_catalog, right_catalog) = if catalogs {
                (
                    load_standalone(&left, out).context("left standalone catalog")?,
                    load_standalone(&right, out).context("right standalone catalog")?,
                )
            } else if live {
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
                let left_catalog = load_baseline(&left, out).context("left catalog")?;
                let right_catalog = load_baseline(&right, out).context("right catalog")?;
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
        Command::Status {
            root: path,
            resume,
            checkpoint_interval,
        } => {
            let root = root(&path)?;
            // Validate metadata before reading potentially terabytes of data.
            let _guard = resume
                .then(|| catalog::InitGuard::lock(&root))
                .transpose()?;
            let expected = load_baseline(&root, out)?;
            ensure!(
                expected.policy == policy::Policy::load(&root)?,
                ".keeperignore differs from recorded catalog scope; restore the original policy before status (never delete a catalog to change scope)"
            );
            let mut work = resume
                .then(|| {
                    checkpoint::Checkpoint::open(&root, "status", Some(&expected)).map(|work| {
                        work.with_sync_interval(Duration::from_secs(
                            checkpoint_interval.unwrap_or(5),
                        ))
                    })
                })
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
                    "Scope: checkpointed multi-invocation session {}.\n  - Earlier per-path reads may be reused; NOT a fresh full scrub.\n  - NTFS/FAT/exFAT/FUSE files are always reread; checkpoint reuse is disabled.\n  - Equal metadata does not prove unchanged bytes or detect every concurrent edit.\n  - Keep writers stopped throughout the session.",
                    work.session
                ),
                color,
            )?;
            writeln!(out)?;
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
            let message = format!("keeper: {error:#}");
            if cancel::is_cancellation(&error) {
                eprintln!("{}", display::cancellation(&message, progress_color));
            } else {
                eprintln!("{message}");
            }
            ExitCode::from(2)
        }
    }
}
