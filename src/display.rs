//! Human-facing presentation only; authoritative catalog encodings stay unchanged.
use crate::catalog::Catalog;
use std::{
    collections::BTreeSet,
    fmt::Display,
    io::{self, IsTerminal, Write},
};

pub fn number(value: impl Display) -> String {
    let raw = value.to_string();
    if let Ok(value) = raw.parse::<u128>()
        && value >= 1_000_000
    {
        let units = ["M", "G", "T", "P", "E", "Z", "Y", "R", "Q"];
        let mut scaled = value as f64 / 1_000_000.0;
        let mut unit = 0;
        while scaled >= 999.995 && unit < units.len() - 1 {
            scaled /= 1000.0;
            unit += 1;
        }
        let rounded = format!("{scaled:.2}");
        return format!(
            "{}{}",
            rounded.trim_end_matches('0').trim_end_matches('.'),
            units[unit]
        );
    }
    let mut result = String::new();
    for (index, ch) in raw.chars().enumerate() {
        if index > 0 && (raw.len() - index).is_multiple_of(3) {
            result.push(',');
        }
        result.push(ch);
    }
    result
}

pub fn bytes(value: u64) -> String {
    let mut scaled = value as f64;
    let units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    let mut unit = 0;
    while scaled >= 1024.0 && unit < units.len() - 1 {
        scaled /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} B", number(value))
    } else {
        format!("{scaled:.2} {}", units[unit])
    }
}

/// Single-unit elapsed time or ETA; promote only at two minutes or two hours.
pub fn duration(seconds: f64) -> String {
    if seconds >= 7_200.0 {
        format!("{:.0}h", seconds / 3_600.0)
    } else if seconds >= 120.0 {
        format!("{:.0}m", seconds / 60.0)
    } else {
        format!("{seconds:.0}s")
    }
}

pub fn label(text: &str, color: bool) -> String {
    if !color {
        return text.to_owned();
    }
    let style = match text {
        "CHANGED" | "DIFFERENT" | "MISSING" => "1;31",
        "ADDED" | "ONLY_LEFT" | "ONLY_RIGHT" => "1;33",
        _ if text.starts_with("ISSUE\t") || text.starts_with("INCOMPLETE") => "1;33",
        _ => "1;36",
    };
    format!("\x1b[{style}m{text}\x1b[0m")
}

pub fn dim(text: &str, color: bool) -> String {
    if color {
        format!("\x1b[2m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

pub fn cancellation(text: &str, color: bool) -> String {
    if !color {
        return text.to_owned();
    }
    // Reset intensity around the token so the surrounding dim style cannot
    // diminish the highlight, then restore dim (not bold) for the rest.
    format!(
        "\x1b[2m{}\x1b[0m",
        text.replace("--resume", "\x1b[0;1;32m--resume\x1b[0;2m")
    )
}

pub fn note(out: &mut impl Write, text: &str, color: bool) -> io::Result<()> {
    writeln!(out, "{}", dim(text, color))
}

/// Compact plain-text table. Only a final left-aligned text column may be
/// shortened on a TTY; numeric columns and preceding labels retain full values.
/// Style after padding to preserve alignment.
pub fn table(
    out: &mut impl Write,
    headers: &[&str],
    rows: &[Vec<String>],
    right_align: &[bool],
    color: bool,
) -> io::Result<()> {
    let columns = if io::stdout().is_terminal() {
        terminal_columns()
    } else {
        None
    };
    table_with_width(out, headers, rows, right_align, color, columns)
}

fn table_with_width(
    out: &mut impl Write,
    headers: &[&str],
    rows: &[Vec<String>],
    right_align: &[bool],
    color: bool,
    columns: Option<usize>,
) -> io::Result<()> {
    let mut widths: Vec<_> = headers.iter().map(|text| text.chars().count()).collect();
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    if let (Some(columns), Some((&last, preceding))) = (columns, widths.split_last())
        && right_align.last() == Some(&false)
    {
        let used = preceding.iter().sum::<usize>() + 2 * preceding.len();
        let limit = columns.saturating_sub(used + 1).max(3);
        *widths.last_mut().unwrap() = last.min(limit);
    }
    let format_row = |cells: &[String], data: bool| {
        cells
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let width = widths[index];
                let text = if text.chars().count() > width {
                    format!(
                        "{}...",
                        text.chars()
                            .take(width.saturating_sub(3))
                            .collect::<String>()
                    )
                } else {
                    text.clone()
                };
                let cell = if right_align[index] {
                    format!("{text:>width$}")
                } else if index + 1 == cells.len() {
                    text
                } else {
                    format!("{text:<width$}")
                };
                if data
                    && color
                    && (headers[index].starts_with("Root") || headers[index] == "Directory")
                {
                    format!("\x1b[0;34m{cell}\x1b[0m")
                } else {
                    cell
                }
            })
            .collect::<Vec<_>>()
            .join("  ")
    };
    let header = format_row(
        &headers.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
        false,
    );
    note(out, &header, color)?;
    for row in rows {
        writeln!(out, "{}", format_row(row, true))?;
    }
    Ok(())
}

fn terminal_columns() -> Option<usize> {
    let mut size = std::mem::MaybeUninit::<libc::winsize>::zeroed();
    // SAFETY: valid writable winsize pointer; read only after successful ioctl.
    if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, size.as_mut_ptr()) } == 0 {
        let columns = unsafe { size.assume_init() }.ws_col as usize;
        (columns > 0).then_some(columns)
    } else {
        None
    }
}

pub fn emphasis(text: &str, color: bool) -> String {
    if color {
        format!("\x1b[1m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

/// Style a plain ASCII progress line only after fitting it to the terminal width.
pub fn progress_line(text: &str, color: bool, completed: bool) -> String {
    if !color {
        return text.to_owned();
    }
    text.split_inclusive([';', '|'])
        .enumerate()
        .map(|(index, segment)| {
            let (part, separator) = if segment.ends_with([';', '|']) {
                segment.split_at(segment.len() - 1)
            } else {
                (segment, "")
            };
            let styled = if part.trim().ends_with('%') {
                format!("\x1b[1;32m{part}\x1b[0m")
            } else if part.trim_start().starts_with('[') {
                part.replace('*', "\x1b[32m*\x1b[0m")
                    .replace('.', "\x1b[2m.\x1b[0m")
            } else if let Some((key, value)) = part.split_once('=') {
                let value = if key.trim() == "ETA" {
                    format!("\x1b[1;32m{value}\x1b[0m")
                } else {
                    emphasis(value, true)
                };
                format!("\x1b[2m{key}=\x1b[0m{value}")
            } else if index == 0 {
                if let Some((phase, state)) = part.split_once(": ") {
                    let style = if completed { "1;32" } else { "1;33" };
                    format!("\x1b[1;36m{phase}\x1b[0m: \x1b[{style}m{state}\x1b[0m")
                } else {
                    emphasis(part, true)
                }
            } else {
                emphasis(part, true)
            };
            format!("{styled}{separator}")
        })
        .collect()
}

/// Keep report sections airy on terminals without adding padding to pipes/logs.
pub fn section(out: &mut impl Write) -> io::Result<()> {
    if io::stdout().is_terminal() {
        writeln!(out)?;
    }
    Ok(())
}

pub fn summary(
    out: &mut impl Write,
    title: &str,
    catalog: &Catalog,
    color: bool,
) -> io::Result<()> {
    section(out)?;
    let total: u64 = catalog.values().map(|entry| entry.size).sum();
    let objects: BTreeSet<_> = catalog
        .values()
        .map(|entry| (&entry.hash, entry.size))
        .collect();
    let unique: u64 = objects.iter().map(|(_, size)| size).sum();
    writeln!(
        out,
        "{}: {} file(s); {}.\n  {} unique content object(s), {} unique content.",
        label(title, color),
        number(catalog.len()),
        bytes(total),
        number(objects.len()),
        bytes(unique)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tables_align_numbers_and_shorten_only_terminal_paths() {
        let rows = vec![
            vec!["1".into(), "photos/a-very-long-directory".into()],
            vec!["12,345".into(), "short".into()],
        ];
        let mut plain = Vec::new();
        table_with_width(
            &mut plain,
            &["Files", "Directory"],
            &rows,
            &[true, false],
            false,
            None,
        )
        .unwrap();
        let plain = String::from_utf8(plain).unwrap();
        assert!(plain.contains("     1  photos/a-very-long-directory\n12,345  short\n"));
        assert!(!plain.contains('\x1b'));
        let mut narrow = Vec::new();
        table_with_width(
            &mut narrow,
            &["Files", "Directory"],
            &rows,
            &[true, false],
            true,
            Some(24),
        )
        .unwrap();
        let narrow = String::from_utf8(narrow).unwrap();
        assert!(narrow.starts_with("\x1b[2m"));
        assert!(narrow.contains("     1  \x1b[0;34mphotos/a-ver..."));
        assert!(!narrow.contains("long-directory"));
        assert_eq!(dim("note", false), "note");
        assert_eq!(dim("note", true), "\x1b[2mnote\x1b[0m");
    }

    #[test]
    fn inventory_keeps_full_roots_and_numeric_columns_on_narrow_terminals() {
        let root = "/mnt/a-long-root-name";
        let rows = vec![vec![root.into(), "123".into(), "1.23 TiB".into()]];
        let mut output = Vec::new();
        table_with_width(
            &mut output,
            &["Root (L / R)", "Files", "Size"],
            &rows,
            &[false, true, true],
            true,
            Some(20),
        )
        .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains(&format!("\x1b[0;34m{root}")));
        assert!(output.contains("123  1.23 TiB"));
        assert!(!output.contains("..."));
    }

    #[test]
    fn durations_use_one_unit_and_promote_only_at_two() {
        for (seconds, expected) in [
            (0.0, "0s"),
            (1.0, "1s"),
            (60.0, "60s"),
            (119.0, "119s"),
            (120.0, "2m"),
            (180.0, "3m"),
            (3_600.0, "60m"),
            (6_900.0, "115m"),
            (7_200.0, "2h"),
            (9_490.0, "3h"),
            (86_400.0, "24h"),
        ] {
            assert_eq!(duration(seconds), expected);
        }
    }

    #[test]
    fn progress_phase_and_state_have_distinct_styles() {
        for phase in ["Hashing", "Left Hashing", "Preflight"] {
            for (state, completed, style) in [
                ("started", false, "1;33"),
                ("running", false, "1;33"),
                ("completed", true, "1;32"),
            ] {
                let text = format!("{phase}: {state}; elapsed=1s");
                assert_eq!(progress_line(&text, false, completed), text);
                let styled = progress_line(&text, true, completed);
                assert!(styled.starts_with(&format!(
                    "\x1b[1;36m{phase}\x1b[0m: \x1b[{style}m{state}\x1b[0m;"
                )));
            }
        }
    }

    #[test]
    fn file_progress_uses_coloured_stars_without_changing_plain_output() {
        let text = "Hashing: running; ETA=2h; [*****.....]";
        assert_eq!(progress_line(text, false, false), text);
        let styled = progress_line(text, true, false);
        assert_eq!(styled.matches("\x1b[32m*\x1b[0m").count(), 5);
        assert_eq!(styled.matches("\x1b[2m.\x1b[0m").count(), 5);
        assert!(styled.contains("ETA="));
    }

    #[test]
    fn progress_highlights_values_and_dims_labels_after_bar() {
        for text in [
            "Hashing: running; elapsed=1m; 50.0%; ETA=2h; [*****.....] | files=1/2; read=1 B/2 B",
            "Hashing: running; 50.0%; ETA=2h | files=1/2",
            "50.0%; ETA=2h",
            "ETA=2h",
        ] {
            assert_eq!(progress_line(text, false, false), text);
            let styled = progress_line(text, true, false);
            assert!(styled.contains("ETA=\x1b[0m\x1b[1;32m2h"));
            if text.contains("50.0%") {
                assert!(styled.contains("50.0%\x1b[0m"));
                assert!(styled.contains("\x1b[1;32m50.0%") || styled.contains("\x1b[1;32m 50.0%"));
            }
            if text.contains("files=") {
                assert!(styled.contains("|\x1b[2m files=\x1b[0m\x1b[1m1/2"));
            }
        }
    }

    #[test]
    fn cancellation_is_dim_with_a_bold_green_resume_token() {
        let text = "keeper: cancelled; when using --resume, completed files are retained";
        assert_eq!(cancellation(text, false), text);
        let styled = cancellation(text, true);
        assert!(styled.starts_with("\x1b[2mkeeper: cancelled"));
        assert!(styled.contains("\x1b[0;1;32m--resume\x1b[0;2m, completed"));
        assert!(styled.ends_with("\x1b[0m"));
    }

    #[test]
    fn readable_numbers_and_sizes() {
        assert_eq!(number(0), "0");
        assert_eq!(number(123), "123");
        assert_eq!(number(999999), "999,999");
        assert_eq!(number(1000000), "1M");
        assert_eq!(number(1234567), "1.23M");
        assert_eq!(number(999999999), "1G");
        assert_eq!(number(1000000000000_u64), "1T");
        assert_eq!(bytes(1024), "1.00 KiB");
        assert_eq!(bytes(42), "42 B");
        assert_eq!(bytes(100000), "97.66 KiB");
        assert_eq!(label("CHANGED", false), "CHANGED");
        assert_eq!(label("CHANGED", true), "\x1b[1;31mCHANGED\x1b[0m");
    }
}
