use crate::{
    catalog::Catalog,
    display::{self, bytes, label, number},
    encoding,
    scan::Inventory,
};
use std::{
    collections::BTreeMap,
    io::{self, Write},
};

struct ReportOptions {
    color: bool,
    long: bool,
    totals: [(u64, u64); 2],
    scope: &'static str,
    issues: Vec<String>,
}

/// Path-sensitive comparison. Object presence elsewhere is not an exact replica.
pub fn report(
    left: &Catalog,
    right: &Catalog,
    out: &mut impl Write,
    color: bool,
    long: bool,
    live: bool,
) -> io::Result<bool> {
    let totals = [left, right].map(|catalog| {
        (
            catalog.len() as u64,
            catalog.values().map(|entry| entry.size).sum(),
        )
    });
    let options = ReportOptions {
        color,
        long,
        totals,
        issues: Vec::new(),
        scope: if live {
            "Scope: freshly read file bytes; no catalogs. Sequential scans, not a snapshot."
        } else {
            "Scope: recorded catalogs only; dataset bytes were NOT checked."
        },
    };
    report_entries(left, right, out, &options, |entry, other| {
        format!(
            "left={}\tleft_size={}\tright={}\tright_size={}",
            entry.hash,
            bytes(entry.size),
            other.hash,
            bytes(other.size)
        )
    })
}

pub fn report_metadata(
    left: &Inventory,
    right: &Inventory,
    out: &mut impl Write,
    color: bool,
    long: bool,
) -> io::Result<u8> {
    let issues = [("Left", left), ("Right", right)]
        .into_iter()
        .flat_map(|(side, inventory)| {
            inventory
                .issues
                .iter()
                .map(move |issue| format!("{side} {issue}"))
        })
        .collect();
    let options = ReportOptions {
        color,
        long,
        issues,
        totals: [left, right].map(|inventory| (inventory.file_count(), inventory.bytes)),
        scope: "Paths + sizes only; contents unchecked. Sequential scans, not a snapshot.",
    };
    let different = report_entries(
        &left.file_sizes(),
        &right.file_sizes(),
        out,
        &options,
        |left, right| format!("left_size={}\tright_size={}", bytes(*left), bytes(*right)),
    )?;
    Ok(if options.issues.is_empty() {
        u8::from(different)
    } else {
        2
    })
}

// Conservative, case-sensitive component names. Never exclude these paths or
// downgrade their exit code: trash may contain valuable user data.
fn os_housekeeping(path: &[u8]) -> bool {
    path.split(|&byte| byte == b'/').any(|component| {
        matches!(
            component,
            b"$RECYCLE.BIN"
                | b"System Volume Information"
                | b".Trashes"
                | b".Trash"
                | b".Spotlight-V100"
                | b".fseventsd"
                | b".DS_Store"
        ) || component
            .strip_prefix(b".Trash-")
            .is_some_and(|uid| !uid.is_empty() && uid.iter().all(u8::is_ascii_digit))
    })
}

#[derive(Default)]
struct Differences {
    // Left-only, right-only, different, OS housekeeping files.
    counts: [usize; 4],
    directories: BTreeMap<Vec<u8>, [usize; 4]>,
}

impl Differences {
    fn record(&mut self, path: &[u8], kind: usize) {
        let parent = path
            .iter()
            .rposition(|&b| b == b'/')
            .map_or(b".".as_slice(), |index| &path[..index]);
        let directory = self.directories.entry(parent.to_vec()).or_default();
        directory[kind] += 1;
        self.counts[kind] += 1;
        if os_housekeeping(path) {
            directory[3] += 1;
            self.counts[3] += 1;
        }
    }
}

fn heading(out: &mut impl Write, text: &str, color: bool) -> io::Result<()> {
    writeln!(out, "\n{}", display::emphasis(text, color))
}

fn report_entries<T: PartialEq>(
    left: &BTreeMap<Vec<u8>, T>,
    right: &BTreeMap<Vec<u8>, T>,
    out: &mut impl Write,
    options: &ReportOptions,
    describe: impl Fn(&T, &T) -> String,
) -> io::Result<bool> {
    let mut differences = Differences::default();
    let mut matching = 0;
    for (path, entry) in left {
        match right.get(path) {
            None => differences.record(path, 0),
            Some(other) if entry != other => differences.record(path, 2),
            _ => matching += 1,
        }
    }
    for path in right.keys().filter(|path| !left.contains_key(*path)) {
        differences.record(path, 1);
    }
    let [only_left, only_right, different, os] = differences.counts;
    let total = only_left + only_right + different;
    let complete = options.issues.is_empty();
    let color = options.color;

    // Assessment contains data and completeness, never legends or caveats.
    display::section(out)?;
    writeln!(
        out,
        "{}: {} matching, {} only left, {} only right, {} different.",
        label(
            if complete {
                "Comparison"
            } else {
                "INCOMPLETE — Observed"
            },
            color
        ),
        number(matching),
        number(only_left),
        number(only_right),
        number(different)
    )?;
    if os > 0 {
        if complete && os == total {
            writeln!(out, "Likely junk: all {} differing paths.", number(total))?;
        } else {
            writeln!(
                out,
                "Likely junk: {} paths; other: {}.",
                number(os),
                number(total - os)
            )?;
        }
    }

    heading(out, "Inventory", color)?;
    display::table(
        out,
        &["Side", "Files", "Size"],
        &[
            vec![
                "Left".into(),
                number(options.totals[0].0),
                bytes(options.totals[0].1),
            ],
            vec![
                "Right".into(),
                number(options.totals[1].0),
                bytes(options.totals[1].1),
            ],
        ],
        &[false, true, true],
        color,
    )?;
    let [(left_files, left_bytes), (right_files, right_bytes)] = options.totals;
    writeln!(
        out,
        "Delta (R - L): {}{} files, {}{}.",
        if right_files >= left_files { "+" } else { "-" },
        number(right_files.abs_diff(left_files)),
        if right_bytes >= left_bytes { "+" } else { "-" },
        bytes(right_bytes.abs_diff(left_bytes))
    )?;

    if total > 0 {
        heading(out, "Directory differences", color)?;
        let rows = differences
            .directories
            .iter()
            .map(|(path, &[left, right, changed, os])| {
                vec![
                    number(left),
                    number(right),
                    number(changed),
                    number(os),
                    encoding::encode(path),
                ]
            })
            .collect::<Vec<_>>();
        display::table(
            out,
            &["Left-only", "Right-only", "Different", "Junk", "Directory"],
            &rows,
            &[true, true, true, true, false],
            color,
        )?;
    }

    // A second walk avoids buffering arbitrarily large detailed file reports.
    if options.long && total > 0 {
        heading(out, "File differences", color)?;
        let mut file = |kind: &str, path: &[u8], detail: String| {
            writeln!(
                out,
                "{}\t{}{detail}{}",
                label(kind, color),
                encoding::encode(path),
                if os_housekeeping(path) {
                    "\tcategory=OS_HOUSEKEEPING"
                } else {
                    ""
                }
            )
        };
        for (path, entry) in left {
            match right.get(path) {
                None => file(
                    if complete {
                        "ONLY_LEFT"
                    } else {
                        "OBSERVED_ONLY_LEFT"
                    },
                    path,
                    String::new(),
                )?,
                Some(other) if entry != other => {
                    file("DIFFERENT", path, format!("\t{}", describe(entry, other)))?
                }
                _ => {}
            }
        }
        for path in right.keys().filter(|path| !left.contains_key(*path)) {
            file(
                if complete {
                    "ONLY_RIGHT"
                } else {
                    "OBSERVED_ONLY_RIGHT"
                },
                path,
                String::new(),
            )?;
        }
    }

    heading(
        out,
        if complete { "Notes" } else { "Notes / issues" },
        color,
    )?;
    let mut bullet = |text: &str| display::note(out, &format!("- {text}"), color);
    if !complete {
        bullet("Observed files only; unreadable paths may change totals and apparent absences.")?;
        for issue in &options.issues {
            bullet(issue)?;
        }
    }
    bullet(options.scope)?;
    if total > 0 {
        bullet(if options.long {
            "File counts by direct parent."
        } else {
            "File counts by direct parent. Use --long for files."
        })?;
    }
    if os > 0 {
        bullet(
            "Junk = likely housekeeping (name-based), not safe to delete; trash may hold user data.",
        )?;
    }
    Ok(total > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conservative_housekeeping_names() {
        for path in [
            b"$RECYCLE.BIN/a".as_slice(),
            b"System Volume Information/a",
            b"dir/.Trash-1000/files/photo",
            b".Trash/a",
            b".Trashes/501/a",
            b"photos/.DS_Store",
            b".Spotlight-V100/a",
            b".fseventsd/a",
        ] {
            assert!(os_housekeeping(path), "{path:?}");
        }
        for path in [
            b"photo.jpg".as_slice(),
            b"MyTrash/a",
            b".Trash-backup/a",
            b".Trash-/a",
            b"System Volume Information backup/a",
            b".Trash-1000.jpg",
        ] {
            assert!(!os_housekeeping(path), "{path:?}");
        }
    }
}
