//! Rows as aligned columns for a terminal: the header, then one line per row,
//! columns two spaces apart. A column of numbers is aligned right, any other
//! left; an empty cell stays blank. A number shows at most 6 significant
//! digits ([`number`]), a `commit` column the first 12 characters of each id
//! (`--csv` prints both whole), and a line break in a cell prints as a space,
//! so one row is always one line.

/// Characters of a commit id the `commit` column shows.
const COMMIT_WIDTH: usize = 12;

/// The table under `header`, aligned, each line ended by `\n`.
pub fn table<S: AsRef<str>>(header: &[S], rows: &[Vec<Option<String>>]) -> String {
    let header: Vec<&str> = header.iter().map(AsRef::as_ref).collect();
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .zip(&header)
                .map(|(cell, column)| shown(cell.as_deref().unwrap_or_default(), column))
                .collect()
        })
        .collect();
    let widths: Vec<usize> = (0..header.len())
        .map(|i| {
            cells
                .iter()
                .map(|row| row[i].chars().count())
                .chain([header[i].chars().count()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let numeric: Vec<bool> = (0..header.len())
        .map(|i| {
            cells.iter().any(|row| !row[i].is_empty())
                && cells
                    .iter()
                    .all(|row| row[i].is_empty() || is_number(&row[i]))
        })
        .collect();
    let mut out = String::new();
    let header: Vec<String> = header.iter().map(|h| (*h).to_owned()).collect();
    for line in std::iter::once(&header).chain(&cells) {
        let mut text = String::new();
        for (i, cell) in line.iter().enumerate() {
            if i > 0 {
                text.push_str("  ");
            }
            let pad = " ".repeat(widths[i] - cell.chars().count());
            if numeric[i] {
                text.push_str(&pad);
                text.push_str(cell);
            } else {
                text.push_str(cell);
                text.push_str(&pad);
            }
        }
        out.push_str(text.trim_end());
        out.push('\n');
    }
    out
}

/// A cell as the terminal shows it.
fn shown(cell: &str, column: &str) -> String {
    if column == "commit" && cell.len() > COMMIT_WIDTH && cell.is_ascii() {
        return cell[..COMMIT_WIDTH].to_owned();
    }
    let (sign, digits) = cell
        .strip_prefix('+')
        .map_or(("", cell), |rest| ("+", rest));
    if let (true, Ok(value)) = (spelled_as_number(digits), digits.parse::<f64>()) {
        return format!("{sign}{}", number(value));
    }
    cell.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Whether `cell` is written as a plain number: digits, a sign, a point, an
/// exponent; never a word that happens to parse (`inf`, `NaN`).
fn spelled_as_number(cell: &str) -> bool {
    cell.bytes().any(|b| b.is_ascii_digit())
        && cell
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.' | b'e' | b'E'))
}

/// `value` as a person reads it: an integer below 10^15 whole (`101529`);
/// any other value from 10^-4 to 10^15 rounded to 6 significant digits,
/// trailing zeros dropped (`474.183`, `4531.16`, `19.5846`, `-0.000123`);
/// beyond that range in exponent form with up to 6 (`1.2e-7`, `1.23457e20`).
pub fn number(value: f64) -> String {
    let magnitude = value.abs();
    if value == 0.0 {
        return "0".to_owned();
    }
    if !(1e-4..1e15).contains(&magnitude) {
        let text = format!("{value:.5e}");
        let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
        return format!("{}e{exponent}", trimmed(mantissa));
    }
    if value.fract() == 0.0 {
        return format!("{value:.0}");
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "log10 of a magnitude in 1e-4..1e15 lies in -4..15"
    )]
    let order = magnitude.log10().floor() as i32;
    let decimals = usize::try_from(5 - order).unwrap_or(0);
    trimmed(&format!("{value:.decimals$}")).to_owned()
}

/// A decimal without trailing zeros, nor a trailing point.
fn trimmed(text: &str) -> &str {
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.')
    } else {
        text
    }
}

/// A number as the rows spell one: digits with an optional sign, point,
/// exponent, or a trailing `%`.
fn is_number(cell: &str) -> bool {
    let digits = cell.strip_suffix('%').unwrap_or(cell);
    spelled_as_number(digits) && digits.parse::<f64>().is_ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn numbers_align_right_and_empty_cells_stay_blank() {
        let rows = vec![
            vec![Some("a".to_owned()), Some("10".to_owned()), None],
            vec![
                Some("bbb".to_owned()),
                Some("2".to_owned()),
                Some("x y".to_owned()),
            ],
        ];
        assert_eq!(
            super::table(&["name", "n", "note"], &rows),
            "name   n  note\na     10\nbbb    2  x y\n"
        );
    }

    #[test]
    fn numbers_show_six_significant_digits_at_most() {
        use super::number;
        for (value, shown) in [
            (0.0, "0"),
            (101_529.0, "101529"),
            (474.182_647_705_078_1, "474.183"),
            (4_531.158_249_999_999_5, "4531.16"),
            (19.584_647_280_501_027, "19.5846"),
            (0.056_824_259_457_659_565, "0.0568243"),
            (-0.3, "-0.3"),
            (-272_317.796_875, "-272318"),
            (0.000_123_456_7, "0.000123457"),
            (1.2e-7, "1.2e-7"),
            (-3.5e-9, "-3.5e-9"),
            (123_456_789_012_345_680_000.0, "1.23457e20"),
            (1e15, "1e15"),
            (5_000_000_000_000.0, "5000000000000"),
        ] {
            assert_eq!(number(value), shown, "{value}");
        }
    }

    #[test]
    fn signed_changes_keep_their_sign_and_words_stay_words() {
        let rows = vec![vec![
            Some("+0.056824259457659565".to_owned()),
            Some("-9.3%".to_owned()),
            Some("inf".to_owned()),
            Some("1.4.0".to_owned()),
        ]];
        assert_eq!(
            super::table(&["a", "b", "c", "d"], &rows),
            "         a      b  c    d\n+0.0568243  -9.3%  inf  1.4.0\n"
        );
    }

    #[test]
    fn commits_are_abbreviated_and_breaks_flattened() {
        let rows = vec![vec![Some("c".repeat(40)), Some("two\nlines".to_owned())]];
        assert_eq!(
            super::table(&["commit", "subject"], &rows),
            format!("commit        subject\n{}  two lines\n", "c".repeat(12))
        );
    }
}
