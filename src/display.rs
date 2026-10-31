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

pub fn label(text: &str, color: bool) -> String {
    if !color {
        return text.to_owned();
    }
    let style = match text {
        "CHANGED" | "DIFFERENT" | "MISSING" => "1;31",
        "ADDED" | "ONLY_LEFT" | "ONLY_RIGHT" => "1;33",
        _ if text.starts_with("ISSUE\t") => "1;33",
        _ => "1;36",
    };
    format!("\x1b[{style}m{text}\x1b[0m")
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
    text.split(';')
        .enumerate()
        .map(|(index, part)| {
            if index == 0 {
                let style = if completed { "1;32" } else { "1;36" };
                format!("\x1b[{style}m{part}\x1b[0m")
            } else if let Some((key, value)) = part.split_once('=') {
                format!("\x1b[2m{key}=\x1b[0m{}", emphasis(value, true))
            } else {
                emphasis(part, true)
            }
        })
        .collect::<Vec<_>>()
        .join(";")
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
