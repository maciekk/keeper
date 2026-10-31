#[cfg(not(target_os = "linux"))]
compile_error!("This first Keeper milestone currently supports Linux only.");

mod catalog;
mod encoding;
mod scan;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

#[derive(Parser)]
#[command(
    version,
    about = "An inspectable integrity catalog. Never silently accepts changes."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
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
    anyhow::ensure!(root.is_dir(), "dataset root must be a directory");
    Ok(root)
}

fn run(cli: Cli, out: &mut impl Write) -> Result<u8> {
    match cli.command {
        Command::Init { root: path } => {
            let root = root(&path)?;
            let guard = catalog::InitGuard::acquire(&root)?;
            let catalog = scan::scan(&root)?;
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
        Command::Status { root: path } => {
            let root = root(&path)?;
            // Validate metadata before reading potentially terabytes of data.
            let expected = catalog::load(&root)?;
            let observed = scan::scan(&root)?;
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
    let mut out = io::BufWriter::new(io::stdout().lock());
    match run(cli, &mut out).and_then(|code| {
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
