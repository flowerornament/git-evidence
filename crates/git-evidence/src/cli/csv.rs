//! Rows as CSV (RFC 4180): a header line, then one line per row. A cell the
//! answer does not hold is an empty field; a field holding a comma, a quote
//! or a line break is quoted, its quotes doubled (README "Output").

/// The table under `header` as CSV text, each line ended by `\n`.
pub fn table<S: AsRef<str>>(header: &[S], rows: &[Vec<Option<String>>]) -> String {
    let mut out = String::new();
    line(&mut out, header.iter().map(|h| Some(h.as_ref())));
    for row in rows {
        line(&mut out, row.iter().map(Option::as_deref));
    }
    out
}

fn line<'a>(out: &mut String, cells: impl Iterator<Item = Option<&'a str>>) {
    for (i, cell) in cells.enumerate() {
        if i > 0 {
            out.push(',');
        }
        let cell = cell.unwrap_or_default();
        if cell.contains([',', '"', '\n', '\r']) {
            out.push('"');
            out.push_str(&cell.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(cell);
        }
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    #[test]
    fn fields_are_quoted_only_when_they_must_be() {
        let rows = vec![vec![
            Some("plain".to_owned()),
            None,
            Some("a,b".to_owned()),
            Some("say \"hi\"".to_owned()),
        ]];
        assert_eq!(
            super::table(&["w", "x", "y", "z"], &rows),
            "w,x,y,z\nplain,,\"a,b\",\"say \"\"hi\"\"\"\n"
        );
    }
}
