//! Numbering formats for device tags and wire numbers.
//!
//! Modelled on the two systems this project studied: SkyCAD's `SkyCadNumberingFormat`
//! (numerical or alphabetical, start value, order format, prefix, increment, uniqueness
//! scope, fall-through) and the drawing settings a real AutoCAD Electrical sheet carries
//! in its `WD_M` block (`%F%N` tag format, `WIRE-RSUF` sequences that skip `I`).

/// How an ordinal turns into a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Numerical,
    Alphabetical,
}

/// The alphabet used by [`Kind::Alphabetical`].
///
/// Defaults to `A..Z` with `I` and `O` removed, which is what panel shops expect on
/// terminals and wire references: a handwritten `1` must not be mistakeable for `I`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alphabet {
    letters: Vec<char>,
}

impl Default for Alphabet {
    fn default() -> Self {
        Self {
            letters: ('A'..='Z').filter(|c| !matches!(c, 'I' | 'O')).collect(),
        }
    }
}

impl Alphabet {
    /// Build an alphabet from an explicit sequence, e.g. the `,A,B,C,â€¦` list a
    /// drawing stores in `WIRE-RSUF`.
    pub fn from_sequence(sequence: &str) -> Self {
        let letters = sequence
            .split(',')
            .filter_map(|s| s.trim().chars().next())
            .collect();
        Self { letters }
    }

    pub fn letters(&self) -> &[char] {
        &self.letters
    }

    /// Letter for a zero-based ordinal, cycling through the alphabet.
    fn at(&self, ordinal: u32) -> char {
        let n = self.letters.len() as u32;
        self.letters[(ordinal % n) as usize]
    }
}

/// When a value must be unique.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uniqueness {
    /// Unique across the whole project.
    Global,
    /// Unique within the parent (sheet, location, strip).
    Local,
    /// Not enforced.
    None,
}

/// A numbering format: how the N-th item of a class is named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberingFormat {
    /// Numerical (`1`, `2`, â€¦) or alphabetical (`A`, `B`, â€¦).
    pub kind: Kind,
    /// Value of the first item. Numerical starts are usually `1`, but `0` and `10` occur.
    pub start: u32,
    /// Step between consecutive items.
    pub increment: u32,
    /// Prefix applied through the `%F` placeholder, usually the component family (`-K`).
    pub prefix: String,
    /// Format pattern. `%N` is the number/letter, `%F` the family prefix, anything else is
    /// literal text â€” `%F%N` gives `-K1`, `%N` alone gives `1`.
    pub pattern: String,
    /// Alphabet used when `kind` is [`Kind::Alphabetical`].
    pub alphabet: Alphabet,
    /// Number the whole connection web rather than each wire (SkyCAD's
    /// `NumberConnectionWeb`).
    pub number_web: bool,
    /// Continue numbering through children instead of restarting them
    /// (SkyCAD's `Fallowthrough`).
    pub fallthrough: bool,
    /// Scope the value must be unique in.
    pub uniqueness: Uniqueness,
}

impl Default for NumberingFormat {
    fn default() -> Self {
        Self {
            kind: Kind::Numerical,
            start: 1,
            increment: 1,
            prefix: String::new(),
            pattern: "%N".to_string(),
            alphabet: Alphabet::default(),
            number_web: false,
            fallthrough: false,
            uniqueness: Uniqueness::Global,
        }
    }
}

impl NumberingFormat {
    /// The tag format a real AutoCAD Electrical drawing uses by default (`TAGFMT=%F%N`).
    pub fn tag(family: &str) -> Self {
        Self {
            prefix: family.to_string(),
            pattern: "%F%N".to_string(),
            ..Self::default()
        }
    }

    /// Wire numbers default to a bare number (`WIREFMT=%N`).
    pub fn wire() -> Self {
        Self::default()
    }

    /// Alphabetical wire numbers.
    pub fn alphabetical() -> Self {
        Self {
            kind: Kind::Alphabetical,
            ..Self::default()
        }
    }

    /// Value for the `ordinal`-th item, counting from zero.
    ///
    /// `ordinal` is an index into the sequence, not the printed value: the printed value
    /// is `start + ordinal * increment`, which is what makes renumbering deterministic.
    pub fn value(&self, ordinal: u32) -> String {
        let raw = match self.kind {
            Kind::Numerical => {
                let number = self.start + ordinal.saturating_mul(self.increment);
                number.to_string()
            }
            Kind::Alphabetical => {
                let letter = self.alphabet.at(ordinal);
                let mut s = String::new();
                s.push(letter);
                s
            }
        };
        self.apply_pattern(&raw)
    }

    /// Substitute `%N` and `%F` in the pattern.
    fn apply_pattern(&self, raw: &str) -> String {
        let mut out = String::with_capacity(self.pattern.len() + raw.len());
        let mut chars = self.pattern.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            match chars.peek().copied() {
                Some('N') => {
                    chars.next();
                    out.push_str(raw);
                }
                Some('F') => {
                    chars.next();
                    out.push_str(&self.prefix);
                }
                // A literal percent escapes as `%%`; anything else keeps the `%`.
                Some('%') => {
                    chars.next();
                    out.push('%');
                }
                _ => out.push('%'),
            }
        }
        out
    }
}

/// One item in a renumbering pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Numbered {
    /// Stable identity of the object being numbered.
    pub id: u64,
    /// Current tag/wire number, if any.
    pub current: Option<String>,
    /// A frozen item keeps its value and consumes its place in the sequence.
    pub frozen: bool,
}

/// The outcome of a renumbering pass: one assigned value per input item, in input order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Renumbered {
    pub id: u64,
    pub value: String,
    pub changed: bool,
}

/// Reading order for numbering: top to bottom, then left to right.
///
/// Returns a sort key rather than a comparator so callers can use it directly. Coordinates
/// are rounded to hundredths so floating-point noise from a drawing does not reorder two
/// symbols that sit on the same line.
pub fn reading_order_key(x: f64, y: f64) -> (i64, i64) {
    let xk = (x * 100.0).round() as i64;
    let yk = (y * 100.0).round() as i64;
    (-yk, xk)
}

/// Infer the prefix a class uses for its tags, from the tags already present.
///
/// `["-K1", "-K2"]` gives `-K`; `["-K1", "-K7", "-K10"]` also gives `-K`. Tags that carry
/// a structure rather than a number (`X1:1`, a terminal in a strip) return `None`: those
/// are numbered by their own rules and must not be rewritten here.
pub fn infer_tag_prefix(tags: &[&str]) -> Option<String> {
    let mut usable = Vec::new();
    for tag in tags {
        let tag = tag.trim();
        if tag.is_empty() {
            continue;
        }
        // A colon or a slash marks a structured tag (strip:terminal, location/device).
        if tag.contains(':') || tag.contains('/') || tag.contains('.') {
            return None;
        }
        usable.push(tag);
    }
    if usable.is_empty() {
        return None;
    }
    let mut prefix = usable[0].to_string();
    for tag in &usable[1..] {
        let common: String = prefix
            .chars()
            .zip(tag.chars())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a)
            .collect();
        prefix = common;
        if prefix.is_empty() {
            return None;
        }
    }
    // Drop the numeric tail: `-K1` and `-K2` share `-K1`, but the prefix is `-K`.
    let trimmed: String = prefix
        .chars()
        .take_while(|c| !c.is_ascii_digit())
        .collect();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed)
}

/// Default prefix for a class with no existing tags, following the IEC 81346 letters the
/// studied drawings use (`-K` contactor, `-M` motor, `-X` terminal, `-F` fuse).///
/// The prefix is only a starting point: once tags exist, [`infer_tag_prefix`] takes over,
/// so a user-edited convention survives the next numbering pass.
pub fn default_prefix(class: &str) -> String {
    let key = class.trim().to_ascii_lowercase();
    let letter = if key.starts_with("contactor")
        || key.starts_with("relay")
        || key.starts_with("timer")
        || key.starts_with("overload")
    {
        'K'
    } else if key.starts_with("motor") {
        'M'
    } else if key.starts_with("terminal") {
        'X'
    } else if key.starts_with("fuse") {
        'F'
    } else if key.starts_with("circuit breaker") || key.starts_with("breaker") {
        'Q'
    } else if key.starts_with("button")
        || key.starts_with("switch")
        || key.starts_with("selector")
    {
        'S'
    } else if key.starts_with("sensor") || key.starts_with("prox") {
        'B'
    } else if key.starts_with("plc") {
        'P'
    } else if key.starts_with("power supply") {
        'G'
    } else if key.starts_with("drive") || key.starts_with("soft starter") {
        'U'
    } else if key.starts_with("transformer") {
        'T'
    } else if key.starts_with("cable") {
        'W'
    } else if key.starts_with("solenoid") || key.starts_with("valve") {
        'Y'
    } else if key.starts_with("lamp") || key.starts_with("indicator") {
        'H'
    } else {
        class
            .chars()
            .find(|c| c.is_ascii_alphabetic())
            .map(|c| c.to_ascii_uppercase())
            .unwrap_or('X')
    };
    format!("-{letter}")
}

/// The next free tag for a class, given the tags already on the drawing.
///
/// `["-K1", "-K2"]` with class `Contactor` gives `-K3`. The prefix follows the drawing when
/// it has one, so symbols inserted after a user renames a device's family stay in that
/// family. A hole in the sequence is not reused: tags identify a device for its whole life,
/// and reusing `-K2` after a deletion would silently point at a different part.
pub fn next_tag(tags: &[&str], class: &str) -> String {
    let prefix = infer_tag_prefix(tags).unwrap_or_else(|| default_prefix(class));
    let mut highest = 0u32;
    for tag in tags {
        let tag = tag.trim();
        let Some(rest) = tag.strip_prefix(prefix.as_str()) else {
            continue;
        };
        if rest.is_empty() || !rest.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Ok(value) = rest.parse::<u32>() {
            highest = highest.max(value);
        }
    }
    let format = NumberingFormat::tag(&prefix);
    format.value(highest)
}

/// Assign values to items in order, honouring frozen items.
///
/// A frozen item keeps its current value; the sequence still advances past it, so the
/// items around it keep the numbers a user would expect after fixing one tag by hand.
pub fn renumber(format: &NumberingFormat, items: &[Numbered]) -> Vec<Renumbered> {
    let mut out = Vec::with_capacity(items.len());
    for (ordinal, item) in items.iter().enumerate() {
        let assigned = match (item.frozen, &item.current) {
            (true, Some(existing)) => existing.clone(),
            _ => format.value(ordinal as u32),
        };
        out.push(Renumbered {
            id: item.id,
            changed: item.current.as_deref() != Some(assigned.as_str()),
            value: assigned,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numerical_sequence_starts_at_one() {
        let f = NumberingFormat::wire();
        assert_eq!(f.value(0), "1");
        assert_eq!(f.value(1), "2");
        assert_eq!(f.value(41), "42");
    }

    #[test]
    fn tag_format_prefixes_the_family() {
        let f = NumberingFormat::tag("-K");
        assert_eq!(f.value(0), "-K1");
        assert_eq!(f.value(9), "-K10");
    }

    #[test]
    fn custom_start_and_increment() {
        let f = NumberingFormat {
            start: 10,
            increment: 5,
            ..NumberingFormat::wire()
        };
        assert_eq!(f.value(0), "10");
        assert_eq!(f.value(1), "15");
        assert_eq!(f.value(3), "25");
    }

    #[test]
    fn alphabetical_skips_confusable_letters() {
        let f = NumberingFormat::alphabetical();
        // A B C D E F G H J K â€¦ â€” I and O are absent from the default alphabet.
        assert_eq!(f.value(0), "A");
        assert_eq!(f.value(7), "H");
        assert_eq!(f.value(8), "J");
        assert!(!f.alphabet.letters().contains(&'I'));
        assert!(!f.alphabet.letters().contains(&'O'));
    }

    #[test]
    fn explicit_suffix_sequence_is_honoured() {
        let f = NumberingFormat {
            alphabet: Alphabet::from_sequence(",A,B,C,D,E,F,G,H,J,K,L,M,N"),
            ..NumberingFormat::alphabetical()
        };
        assert_eq!(f.value(8), "J");
        assert_eq!(f.value(9), "K");
    }

    #[test]
    fn literal_text_survives_the_pattern() {
        let f = NumberingFormat {
            prefix: "W".into(),
            pattern: "W%N".into(),
            ..NumberingFormat::wire()
        };
        assert_eq!(f.value(2), "W3");
    }

    #[test]
    fn percent_escapes() {
        let f = NumberingFormat {
            pattern: "%N%%".into(),
            ..NumberingFormat::wire()
        };
        assert_eq!(f.value(0), "1%");
    }

    #[test]
    fn frozen_items_keep_their_value_and_advance_the_sequence() {
        let items = vec![
            Numbered { id: 1, current: None, frozen: false },
            Numbered { id: 2, current: Some("-K5".into()), frozen: true },
            Numbered { id: 3, current: None, frozen: false },
        ];
        let out = renumber(&NumberingFormat::tag("-K"), &items);
        assert_eq!(out[0].value, "-K1");
        assert_eq!(out[1].value, "-K5");
        assert_eq!(out[2].value, "-K3");
        assert!(out[0].changed);
        assert!(!out[1].changed);
    }
    #[test]
    fn renumbering_is_idempotent() {
        let format = NumberingFormat::tag("-K");
        let first = renumber(
            &format,
            &[
                Numbered { id: 1, current: None, frozen: false },
                Numbered { id: 2, current: None, frozen: false },
            ],
        );
        let items: Vec<Numbered> = first
            .iter()
            .map(|r| Numbered { id: r.id, current: Some(r.value.clone()), frozen: false })
            .collect();
        let second = renumber(&format, &items);
        assert_eq!(
            first.iter().map(|r| r.value.clone()).collect::<Vec<_>>(),
            second.iter().map(|r| r.value.clone()).collect::<Vec<_>>()
        );
        assert!(second.iter().all(|r| !r.changed));
    }

    #[test]
    fn prefix_is_inferred_from_existing_tags() {
        assert_eq!(infer_tag_prefix(&["-K1", "-K2"]).as_deref(), Some("-K"));
        assert_eq!(infer_tag_prefix(&["-K1", "-K7", "-K10"]).as_deref(), Some("-K"));
        assert_eq!(infer_tag_prefix(&["-K1"]).as_deref(), Some("-K"));
        assert_eq!(infer_tag_prefix(&["Q1", "Q2"]).as_deref(), Some("Q"));
    }

    #[test]
    fn structured_tags_are_not_renumbered() {
        assert_eq!(infer_tag_prefix(&["X1:1", "X1:2"]), None);
        assert_eq!(infer_tag_prefix(&["+MCC1/-K1"]), None);
        assert_eq!(infer_tag_prefix(&["-K1", "X1:1"]), None);
    }

    #[test]
    fn prefix_needs_a_non_numeric_core() {
        assert_eq!(infer_tag_prefix(&[]), None);
        assert_eq!(infer_tag_prefix(&["1", "2"]), None);
    }

    #[test]
    fn default_prefix_uses_the_iec_letter() {
        assert_eq!(default_prefix("Contactor"), "-K");
        assert_eq!(default_prefix("Relay"), "-K");
        assert_eq!(default_prefix("Terminal"), "-X");
        assert_eq!(default_prefix("Motor"), "-M");
        assert_eq!(default_prefix("Fuse holder"), "-F");
        assert_eq!(default_prefix("Circuit Breaker"), "-Q");
        assert_eq!(default_prefix("PLC Module"), "-P");
        // Unknown classes fall back to their initial, and numbers get a neutral letter.
        assert_eq!(default_prefix("Widget"), "-W");
        assert_eq!(default_prefix("123"), "-X");
    }

    #[test]
    fn reading_order_is_top_down_then_left_right() {
        let mut items = vec![(10.0, 0.0), (0.0, 0.0), (0.0, 100.0), (5.0, 100.0)];
        items.sort_by_key(|(x, y)| reading_order_key(*x, *y));
        assert_eq!(items, vec![(0.0, 100.0), (5.0, 100.0), (0.0, 0.0), (10.0, 0.0)]);
    }

    #[test]
    fn reading_order_tolerates_float_noise() {
        let a = reading_order_key(10.0, 20.0);
        let b = reading_order_key(10.000001, 19.999999);
        assert_eq!(a, b);
    }
}


    #[test]
    fn next_tag_continues_the_sequence() {
        assert_eq!(next_tag(&["-K1", "-K2"], "Contactor"), "-K3");
        assert_eq!(next_tag(&["-K1", "-K7"], "Contactor"), "-K8");
    }

    #[test]
    fn next_tag_starts_from_the_class_prefix_when_the_drawing_is_empty() {
        assert_eq!(next_tag(&[], "Contactor"), "-K1");
        assert_eq!(next_tag(&[], "Motor"), "-M1");
    }

    #[test]
    fn next_tag_follows_the_prefix_the_drawing_uses() {
        // A drawing that has renamed its family to -CR gets -CR2, not -K2.
        assert_eq!(next_tag(&["-CR1"], "Contactor"), "-CR2");
    }

    #[test]
    fn next_tag_does_not_reuse_a_gap() {
        // -K2 was deleted; the next device is -K4, not -K2.
        assert_eq!(next_tag(&["-K1", "-K3"], "Contactor"), "-K4");
    }

    #[test]
    fn next_tag_ignores_structured_tags() {
        // `X1:1` is a strip terminal rather than a free device tag, so the class default
        // (`-X` for terminals) applies instead of the tag's prefix.
        assert_eq!(next_tag(&["X1:1"], "Terminal"), "-X1");
    }
