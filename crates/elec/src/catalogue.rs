//! Parts catalogue: the data behind a bill of material.
//!
//! A drawing knows a part number; it rarely knows the manufacturer, the description or the
//! order code. Those come from a parts catalogue, which in a real workshop is a spreadsheet
//! exported from the ERP or typed up once and reused. This module reads that sheet and
//! enriches drawing components with it.
//!
//! The CSV columns match the shipped report templates: `Part number`, `Manufacturer`,
//! `Description`. Headers are matched case-insensitively and a few obvious synonyms are
//! accepted, because a hand-made sheet will not spell them the way the template does.

use std::collections::BTreeMap;

use crate::component::Component;

/// One catalogue entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartRecord {
    /// Part number as written in the sheet.
    pub part: String,
    pub manufacturer: Option<String>,
    pub description: Option<String>,
}

/// A lookup table of parts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalogue {
    by_part: BTreeMap<String, PartRecord>,
}

/// Why a catalogue could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogueError {
    /// No header carried a part-number column.
    MissingPartColumn,
    /// The sheet had no rows at all.
    Empty,
}

impl std::fmt::Display for CatalogueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingPartColumn => f.write_str("no part-number column found in the header row"),
            Self::Empty => f.write_str("the catalogue is empty"),
        }
    }
}

impl std::error::Error for CatalogueError {}

impl Catalogue {
    /// Read a catalogue from CSV text.
    ///
    /// The first non-empty line is the header. Rows with an empty part number are skipped:
    /// a blank part is not a part, and keeping it would make every unmatched component look
    /// up an empty record. When a part appears twice the later row wins, which matches how a
    /// spreadsheet behaves when someone appends a correction.
    pub fn from_csv(text: &str) -> Result<Self, CatalogueError> {
        // Excel writes a UTF-8 byte-order mark, and a spreadsheet exported from almost any
        // tool may carry one. Left in place it becomes part of the first header cell, so
        // "Part number" stops matching and the sheet looks like it has no part column.
        let text = text.trim_start_matches('\u{feff}');
        let mut rows = parse_csv(text).into_iter();
        let header = rows.next().ok_or(CatalogueError::Empty)?;
        let part_column = find_column(&header, &["part number", "part", "partnumber", "cat"])
            .ok_or(CatalogueError::MissingPartColumn)?;
        let manufacturer_column =
            find_column(&header, &["manufacturer", "mfg", "mfr", "brand"]);
        let description_column = find_column(&header, &["description", "desc", "details"]);

        let mut by_part = BTreeMap::new();
        for row in rows {
            let part = row.get(part_column).map(|v| v.trim()).unwrap_or("");
            if part.is_empty() {
                continue;
            }
            let manufacturer = manufacturer_column
                .and_then(|column| row.get(column))
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            let description = description_column
                .and_then(|column| row.get(column))
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            by_part.insert(
                part.to_ascii_uppercase(),
                PartRecord {
                    part: part.to_string(),
                    manufacturer,
                    description,
                },
            );
        }

        if by_part.is_empty() {
            return Err(CatalogueError::Empty);
        }
        Ok(Self { by_part })
    }

    /// Look a part number up, ignoring case.
    pub fn lookup(&self, part: &str) -> Option<&PartRecord> {
        self.by_part.get(&part.trim().to_ascii_uppercase())
    }

    pub fn len(&self) -> usize {
        self.by_part.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_part.is_empty()
    }

    /// Fill in a component's missing catalogue fields.
    ///
    /// Values already on the drawing are never overwritten: the drawing is the authority on
    /// what was fitted, and a catalogue row that disagrees is a data problem to look at, not
    /// something to silently correct here. Returns true when anything was filled.
    pub fn enrich(&self, component: &mut Component) -> bool {
        let Some(part) = component.part.as_deref() else {
            return false;
        };
        let Some(record) = self.lookup(part) else {
            return false;
        };
        let mut changed = false;
        if component.manufacturer.is_none() {
            if let Some(manufacturer) = &record.manufacturer {
                component.manufacturer = Some(manufacturer.clone());
                changed = true;
            }
        }
        if component.description.is_none() {
            if let Some(description) = &record.description {
                component.description = Some(description.clone());
                changed = true;
            }
        }
        changed
    }
}

/// Index of the first header cell matching any of `names` (case-insensitive).
fn find_column(header: &[String], names: &[&str]) -> Option<usize> {
    header.iter().position(|cell| {
        let cell = cell.trim().to_ascii_lowercase();
        names.iter().any(|name| cell == *name)
    })
}

/// Parse CSV into rows of fields.
///
/// Handles quoted fields, embedded commas, embedded newlines inside quotes and doubled
/// quotes, which is what Excel and LibreOffice write. Blank lines are dropped.
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' => {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        field.push('"');
                    } else {
                        in_quotes = false;
                    }
                }
                other => field.push(other),
            }
            continue;
        }
        match c {
            '"' => in_quotes = true,
            ',' => {
                row.push(std::mem::take(&mut field));
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                push_row(&mut rows, std::mem::take(&mut row));
            }
            '\n' => {
                row.push(std::mem::take(&mut field));
                push_row(&mut rows, std::mem::take(&mut row));
            }
            other => field.push(other),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        push_row(&mut rows, row);
    }
    rows
}

fn push_row(rows: &mut Vec<Vec<String>>, row: Vec<String>) {
    let blank = row.iter().all(|cell| cell.trim().is_empty());
    if !blank {
        rows.push(row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component(part: &str) -> Component {
        Component::new(1, "Contactor", "-K1").with_part(part)
    }

    #[test]
    fn reads_the_template_columns() {
        let csv = "Part number,Manufacturer,Description\n3RT2016-1AB01,Siemens,Contactor 9A\n";
        let catalogue = Catalogue::from_csv(csv).unwrap();
        let record = catalogue.lookup("3RT2016-1AB01").unwrap();
        assert_eq!(record.manufacturer.as_deref(), Some("Siemens"));
        assert_eq!(record.description.as_deref(), Some("Contactor 9A"));
        assert_eq!(catalogue.len(), 1);
    }

    #[test]
    fn header_matching_is_forgiving() {
        let csv = " PART , Mfg , Desc \nX1,AB,Thing\n";
        let catalogue = Catalogue::from_csv(csv).unwrap();
        let record = catalogue.lookup("X1").unwrap();
        assert_eq!(record.manufacturer.as_deref(), Some("AB"));
    }

    #[test]
    fn quoted_fields_with_commas_are_one_value() {
        let csv = "Part number,Manufacturer,Description\nA1,ACME,\"Terminal, 2.5mm\"\n";
        let catalogue = Catalogue::from_csv(csv).unwrap();
        assert_eq!(
            catalogue.lookup("A1").unwrap().description.as_deref(),
            Some("Terminal, 2.5mm")
        );
    }

    #[test]
    fn doubled_quotes_are_unescaped() {
        let csv = "Part number,Description\nA1,\"He said \"\"hi\"\"\"\n";
        let catalogue = Catalogue::from_csv(csv).unwrap();
        assert_eq!(
            catalogue.lookup("A1").unwrap().description.as_deref(),
            Some("He said \"hi\"")
        );
    }

    #[test]
    fn crlf_line_endings_work() {
        let csv = "Part number,Manufacturer\r\nA1,ACME\r\n";
        let catalogue = Catalogue::from_csv(csv).unwrap();
        assert_eq!(catalogue.lookup("A1").unwrap().manufacturer.as_deref(), Some("ACME"));
    }

    #[test]
    fn a_utf8_byte_order_mark_is_tolerated() {
        // What Excel writes, and what PowerShell's UTF8 encoding writes by default.
        let csv = "\u{feff}Part number,Manufacturer\nA1,ACME\n";
        let catalogue = Catalogue::from_csv(csv).unwrap();
        assert_eq!(catalogue.lookup("A1").unwrap().manufacturer.as_deref(), Some("ACME"));
    }

    #[test]
    fn a_missing_part_column_is_an_error() {
        let csv = "Manufacturer,Description\nSiemens,X\n";
        assert_eq!(
            Catalogue::from_csv(csv),
            Err(CatalogueError::MissingPartColumn)
        );
    }

    #[test]
    fn an_empty_sheet_is_an_error() {
        assert_eq!(Catalogue::from_csv(""), Err(CatalogueError::Empty));
        assert_eq!(
            Catalogue::from_csv("Part number,Manufacturer\n"),
            Err(CatalogueError::Empty)
        );
    }

    #[test]
    fn rows_without_a_part_are_skipped() {
        let csv = "Part number,Manufacturer\n,ACME\nA1,ACME\n";
        let catalogue = Catalogue::from_csv(csv).unwrap();
        assert_eq!(catalogue.len(), 1);
    }

    #[test]
    fn a_repeated_part_keeps_the_later_row() {
        let csv = "Part number,Manufacturer\nA1,Old\nA1,New\n";
        let catalogue = Catalogue::from_csv(csv).unwrap();
        assert_eq!(catalogue.lookup("A1").unwrap().manufacturer.as_deref(), Some("New"));
    }

    #[test]
    fn lookup_ignores_case_and_padding() {
        let csv = "Part number,Manufacturer\nabc-1,ACME\n";
        let catalogue = Catalogue::from_csv(csv).unwrap();
        assert!(catalogue.lookup("ABC-1").is_some());
        assert!(catalogue.lookup(" abc-1 ").is_some());
    }

    #[test]
    fn enrichment_fills_only_missing_fields() {
        let csv = "Part number,Manufacturer,Description\nP,Siemens,Contactor\n";
        let catalogue = Catalogue::from_csv(csv).unwrap();

        let mut blank = component("P");
        assert!(catalogue.enrich(&mut blank));
        assert_eq!(blank.manufacturer.as_deref(), Some("Siemens"));
        assert_eq!(blank.description.as_deref(), Some("Contactor"));

        let mut drawing_says = component("P").with_manufacturer("ABB");
        assert!(catalogue.enrich(&mut drawing_says));
        assert_eq!(drawing_says.manufacturer.as_deref(), Some("ABB"));
    }

    #[test]
    fn enrichment_of_an_unknown_part_changes_nothing() {
        let catalogue = Catalogue::from_csv("Part number,Manufacturer\nP,Siemens\n").unwrap();
        let mut item = component("OTHER");
        assert!(!catalogue.enrich(&mut item));
        assert!(item.manufacturer.is_none());
    }

    #[test]
    fn enrichment_of_a_component_without_a_part_changes_nothing() {
        let catalogue = Catalogue::from_csv("Part number,Manufacturer\nP,Siemens\n").unwrap();
        let mut item = Component::new(1, "Terminal", "X1:1");
        assert!(!catalogue.enrich(&mut item));
    }
}
