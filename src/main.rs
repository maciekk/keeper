#[cfg(not(target_os = "linux"))]
compile_error!("Keeper currently supports Linux only.");

mod catalog;
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
    about = "An inspectable integrity catalog. Never silently accepts changes."
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
    Init {
        #[arg(default_value = ".")]
        root: PathBuf,
    },
    /// Read every byte and compare with the baseline (read-only; no metadata shortcuts).
    Status {
        #[arg(default_value = ".")]
        root: PathBuf,
    },
    /// Inventory metadata and list problems without reading file contents or creating a catalog.
    Preflight {
        #[arg(default_value = ".")]
        root: PathBuf,
    },
    /// Compare recorded catalogs, or reread both trees with --live. Path-sensitive and read-only.
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
        Command::Init { root: path } => {
            let root = root(&path)?;
            let guard = catalog::InitGuard::acquire(&root)?;
            let catalog = scan::scan(&root, progress)?;
            guard.publish(&catalog)?;
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
        Command::Status { root: path } => {
            let root = root(&path)?;
            // Validate metadata before reading potentially terabytes of data.
            let expected = catalog::load(&root)?;
            let observed = scan::scan(&root, progress)?;
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
                "Full-byte check: {} observed file(s); {added} added, {missing} missing, {changed} changed. Baseline unchanged.",
                observed.len()
            )?;
            Ok(u8::from(added + missing + changed > 0))
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
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
