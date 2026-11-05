//! Terminal strips: grouping terminals by strip and ordering them by position.
//!
//! A terminal in these drawings is a component whose tag is `strip:index` - `X1:12` is
//! terminal 12 of strip X1. The strip list is one of the reports an electrician actually
//! builds from, so it is derived from the drawing rather than typed twice.

/// A terminal parsed out of its tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalRef {
    /// Strip name, e.g. `X1`.
    pub strip: String,
    /// Terminal number within the strip.
    pub index: u32,
}

/// Parse `strip:index`.
///
/// Accepts an optional leading `-` (the tag style `-X1:1` some drawings use) and rejects
/// anything that is not a plain name plus a positive integer: `-K1`, `X1`, `X1:` and `:1`
/// are not terminals.
pub fn parse_terminal_tag(tag: &str) -> Option<TerminalRef> {
    let tag = tag.trim();
    let (strip, index) = tag.rsplit_once(':')?;
    let strip = strip.trim_start_matches('-');
    if strip.is_empty() || !strip.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return None;
    }
    if index.is_empty() || !index.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let index: u32 = index.parse().ok()?;
    if index == 0 {
        return None;
    }
    Some(TerminalRef {
        strip: strip.to_string(),
        index,
    })
}

/// One terminal in a strip listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripTerminal {
    /// The terminal's full tag, e.g. `X1:3`.
    pub tag: String,
    pub index: u32,
    /// Devices reaching this terminal through its net, excluding the terminal itself.
    /// Empty when the terminal is wired to nothing.
    pub peers: Vec<String>,
}

/// A terminal strip and its terminals, ordered by terminal number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalStrip {
    pub name: String,
    pub terminals: Vec<StripTerminal>,
}

/// Group terminals into strips.
///
/// `terminals` carries each terminal's tag and the labels of the devices sharing its net.
/// Peer lists are sorted and deduplicated, and a terminal repeated in the input keeps only
/// its first entry, so a drawing that lists the same symbol twice does not inflate the
/// strip.
pub fn terminal_strips(terminals: &[(String, Vec<String>)]) -> Vec<TerminalStrip> {
    use std::collections::BTreeMap;

    let mut by_strip: BTreeMap<String, BTreeMap<u32, StripTerminal>> = BTreeMap::new();
    for (tag, peers) in terminals {
        let Some(parsed) = parse_terminal_tag(tag) else {
            continue;
        };
        let mut peers: Vec<String> = peers
            .iter()
            .map(|peer| peer.trim().to_string())
            .filter(|peer| !peer.is_empty() && !peer.eq_ignore_ascii_case(tag.trim()))
            .collect();
        peers.sort();
        peers.dedup();
        by_strip
            .entry(parsed.strip.clone())
            .or_default()
            .entry(parsed.index)
            .or_insert(StripTerminal {
                tag: tag.trim().to_string(),
                index: parsed.index,
                peers,
            });
    }

    by_strip
        .into_iter()
        .map(|(name, terminals)| TerminalStrip {
            name,
            terminals: terminals.into_values().collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_plain_terminal_tag() {
        let parsed = parse_terminal_tag("X1:12").unwrap();
        assert_eq!(parsed.strip, "X1");
        assert_eq!(parsed.index, 12);
    }

    #[test]
    fn parses_a_dash_prefixed_tag() {
        let parsed = parse_terminal_tag("-X1:3").unwrap();
        assert_eq!(parsed.strip, "X1");
        assert_eq!(parsed.index, 3);
    }

    #[test]
    fn rejects_non_terminal_tags() {
        assert!(parse_terminal_tag("-K1").is_none());
        assert!(parse_terminal_tag("X1").is_none());
        assert!(parse_terminal_tag("X1:").is_none());
        assert!(parse_terminal_tag(":1").is_none());
        assert!(parse_terminal_tag("X1:abc").is_none());
        assert!(parse_terminal_tag("X1:0").is_none());
        assert!(parse_terminal_tag("").is_none());
    }

    #[test]
    fn groups_by_strip_and_sorts_by_index() {
        let input = vec![
            ("X1:10".to_string(), vec![]),
            ("X1:2".to_string(), vec![]),
            ("X2:1".to_string(), vec![]),
            ("X1:1".to_string(), vec![]),
        ];
        let strips = terminal_strips(&input);
        assert_eq!(strips.len(), 2);
        assert_eq!(strips[0].name, "X1");
        let indices: Vec<u32> = strips[0].terminals.iter().map(|t| t.index).collect();
        assert_eq!(indices, vec![1, 2, 10]);
        assert_eq!(strips[1].name, "X2");
    }

    #[test]
    fn peers_exclude_the_terminal_itself_and_are_deduped() {
        let input = vec![(
            "X1:1".to_string(),
            vec!["-K1".to_string(), "X1:1".to_string(), "-K1".to_string()],
        )];
        let strips = terminal_strips(&input);
        assert_eq!(strips[0].terminals[0].peers, vec!["-K1"]);
    }

    #[test]
    fn a_duplicate_terminal_keeps_its_first_entry() {
        let input = vec![
            ("X1:1".to_string(), vec!["-K1".to_string()]),
            ("X1:1".to_string(), vec!["-K2".to_string()]),
        ];
        let strips = terminal_strips(&input);
        assert_eq!(strips[0].terminals.len(), 1);
        assert_eq!(strips[0].terminals[0].peers, vec!["-K1"]);
    }

    #[test]
    fn non_terminals_are_ignored() {
        let input = vec![
            ("-K1".to_string(), vec!["X1:1".to_string()]),
            ("X1:1".to_string(), vec!["-K1".to_string()]),
        ];
        let strips = terminal_strips(&input);
        assert_eq!(strips.len(), 1);
        assert_eq!(strips[0].terminals.len(), 1);
    }

    #[test]
    fn an_empty_input_gives_no_strips() {
        assert!(terminal_strips(&[]).is_empty());
    }
}
