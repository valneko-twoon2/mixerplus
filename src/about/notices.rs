//! Parsing the bundled notices file and reflowing license text into paragraphs.

use super::*;

/// Parses the notices file: a header `name version - license` followed by
/// `--- file ---` sections, until the next header.
pub(super) fn parse_credits(source: &'static str) -> Vec<Credit> {
    let mut lines = Vec::new();
    let mut start = 0;
    for line in source.split_inclusive('\n') {
        lines.push((start, line.trim_end()));
        start += line.len();
    }
    let mut heads = Vec::new();
    for index in 0..lines.len().saturating_sub(1) {
        if lines[index].1.starts_with("--- ") || !lines[index + 1].1.starts_with("--- ") {
            continue;
        }
        let mut parts = lines[index].1.splitn(4, ' ');
        if let (Some(name), Some(version), Some("-"), Some(license)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        {
            heads.push((lines[index].0, name, version, license));
        }
    }
    let mut credits = Vec::new();
    for (position, &(offset, name, version, license)) in heads.iter().enumerate() {
        let end = heads.get(position + 1).map_or(source.len(), |next| next.0);
        let section = source.get(offset..end).unwrap_or("");
        // Drop the header line itself.
        let text = section
            .split_once('\n')
            .map_or("", |(_, rest)| rest)
            .trim_end();
        credits.push(Credit {
            name,
            version,
            license,
            text,
        });
    }
    credits
}

/// Splits license text into paragraphs: hard-wrapped lines of one paragraph are joined,
/// short or differently indented lines stay apart, and `--- file ---` becomes a heading.
pub(super) fn blocks(source: &str) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    let mut previous = 0;
    for line in source.lines() {
        let line = line.trim_end().replace('\t', "    ");
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            if out.last().is_some_and(|block| block.kind != Kind::Gap) {
                out.push(Block {
                    kind: Kind::Gap,
                    indent: 0,
                    text: Vec::new(),
                });
            }
            previous = 0;
            continue;
        }
        if let Some(name) = trimmed
            .strip_prefix("--- ")
            .and_then(|rest| rest.strip_suffix(" ---"))
        {
            while out.last().is_some_and(|block| block.kind == Kind::Gap) {
                out.pop();
            }
            out.push(Block {
                kind: Kind::Heading,
                indent: 0,
                text: name.encode_utf16().collect(),
            });
            previous = 0;
            continue;
        }
        let indent = (line.len() - trimmed.len()) as i32;
        let joined = previous >= 45
            && out
                .last_mut()
                .filter(|block| block.kind == Kind::Body && block.indent == indent)
                .map(|block| {
                    block.text.push(b' ' as u16);
                    block.text.extend(trimmed.encode_utf16());
                })
                .is_some();
        if !joined {
            out.push(Block {
                kind: Kind::Body,
                indent,
                text: trimmed.encode_utf16().collect(),
            });
        }
        previous = trimmed.chars().count();
    }
    while out.last().is_some_and(|block| block.kind == Kind::Gap) {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_parse_into_credits_with_license_text() {
        let credits = parse_credits(NOTICES);
        let headers = NOTICES
            .lines()
            .filter(|line| line.contains(" - ") && !line.starts_with("--- "))
            .count();
        assert!(credits.len() >= 10 && credits.len() <= headers);
        assert!(credits.iter().any(|c| c.name == "windows"));
        assert!(
            credits
                .iter()
                .all(|c| !c.version.is_empty() && c.text.starts_with("--- "))
        );
    }

    #[test]
    fn license_paragraphs_are_reflowed() {
        let parts = blocks(LICENSE);
        // The MIT text joins hard-wrapped lines, so far fewer blocks than source lines.
        assert!(parts.len() < LICENSE.lines().count() / 2);
        assert!(parts.iter().any(|block| block.kind == Kind::Body));
    }
}
