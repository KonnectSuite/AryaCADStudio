//! Report sheets: the tabular data behind the exported reports.
//!
//! The column names match the templates the studied applications ship (`Class name`,
//! `Part number`, `Manufacturer`, `Description`, `Qty`, `Where used`, and the wire/terminal
//! equivalents), so an export can be pasted into an existing workflow without renaming
//! headers.
//!
//! Sheets are plain data: building them here keeps them testable, and lets any writer
//! (xlsx today, CSV or PDF later) consume the same rows.

use crate::component::BomRow;
use crate::geometry::{GeoWireRow, Point};
use crate::terminal::TerminalStrip;

/// One cell.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Text(String),
    Number(f64),
}

impl Cell {
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    pub fn number(value: impl Into<f64>) -> Self {
        Self::Number(value.into())
    }
}

/// A named table.
#[derive(Debug, Clone, PartialEq)]
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<Cell>>,
}

impl Sheet {
    pub fn new(name: &str, header: &[&str]) -> Self {
        Self {
            name: name.to_string(),
            rows: vec![header.iter().map(|cell| Cell::text(*cell)).collect()],
        }
    }

    pub fn push(&mut self, row: Vec<Cell>) {
        self.rows.push(row);
    }

    /// Body rows, excluding the header.
    pub fn body_len(&self) -> usize {
        self.rows.len().saturating_sub(1)
    }
}

/// Bill of material, matching the shipped `BOM.xlsx` columns.
pub fn bom_sheet(rows: &[BomRow]) -> Sheet {
    let mut sheet = Sheet::new(
        "BOM",
        &[
            "Class name",
            "Part number",
            "Manufacturer",
            "Description",
            "Qty",
            "Where used",
        ],
    );
    for row in rows {
        sheet.push(vec![
            Cell::text(row.class.clone()),
            Cell::text(row.part.clone().unwrap_or_default()),
            Cell::text(row.manufacturer.clone().unwrap_or_default()),
            Cell::text(row.description.clone().unwrap_or_default()),
            Cell::number(row.quantity as f64),
            Cell::text(row.where_used.join(", ")),
        ]);
    }
    sheet
}

/// Wire list: one row per wire, with the net it belongs to and its two endpoints.
pub fn wire_sheet(net_of_wire: &[(u64, u64, Point, Point)]) -> Sheet {
    let mut sheet = Sheet::new("Wire List", &["Wire", "Net", "From X", "From Y", "To X", "To Y"]);
    for (wire, net, from, to) in net_of_wire {
        sheet.push(vec![
            Cell::number(*wire as f64),
            Cell::number(*net as f64),
            Cell::number(from.x),
            Cell::number(from.y),
            Cell::number(to.x),
            Cell::number(to.y),
        ]);
    }
    sheet
}

/// Wire list derived from the geometry rows.
pub fn wire_sheet_from(rows: &[GeoWireRow]) -> Sheet {
    let mut sheet = Sheet::new("Wire List", &["Wire", "Net", "From X", "From Y", "To X", "To Y"]);
    for row in rows {
        sheet.push(vec![
            Cell::number(row.wire as f64),
            Cell::number(row.net as f64),
            Cell::number(row.from.x),
            Cell::number(row.from.y),
            Cell::number(row.to.x),
            Cell::number(row.to.y),
        ]);
    }
    sheet
}

/// Terminal strips: one row per terminal, with what it connects to.
pub fn terminal_sheet(strips: &[TerminalStrip]) -> Sheet {
    let mut sheet = Sheet::new("Terminal Strips", &["Strip", "Terminal", "Connected to"]);
    for strip in strips {
        for terminal in &strip.terminals {
            sheet.push(vec![
                Cell::text(strip.name.clone()),
                Cell::text(terminal.tag.clone()),
                Cell::text(if terminal.peers.is_empty() {
                    "unconnected".to_string()
                } else {
                    terminal.peers.join(", ")
                }),
            ]);
        }
    }
    sheet
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::Component;
    use crate::geometry::Point;
    use crate::terminal::terminal_strips;

    fn bom_row(class: &str, part: &str, qty: u32) -> BomRow {
        BomRow {
            class: class.to_string(),
            part: if part.is_empty() { None } else { Some(part.to_string()) },
            manufacturer: Some("Siemens".to_string()),
            description: Some("Contactor".to_string()),
            quantity: qty,
            where_used: vec!["-K1".to_string(), "-K2".to_string()],
        }
    }

    #[test]
    fn bom_sheet_uses_the_template_headers() {
        let sheet = bom_sheet(&[bom_row("Contactor", "3RT", 2)]);
        let header: Vec<&str> = sheet.rows[0]
            .iter()
            .map(|cell| match cell {
                Cell::Text(text) => text.as_str(),
                Cell::Number(_) => "",
            })
            .collect();
        assert_eq!(
            header,
            vec![
                "Class name",
                "Part number",
                "Manufacturer",
                "Description",
                "Qty",
                "Where used"
            ]
        );
    }

    #[test]
    fn bom_quantity_is_a_number_cell() {
        let sheet = bom_sheet(&[bom_row("Contactor", "3RT", 7)]);
        assert_eq!(sheet.rows[1][4], Cell::Number(7.0));
        assert_eq!(sheet.body_len(), 1);
    }

    #[test]
    fn bom_missing_values_become_empty_text() {
        let row = BomRow {
            class: "Terminal".to_string(),
            part: None,
            manufacturer: None,
            description: None,
            quantity: 1,
            where_used: vec![],
        };
        let sheet = bom_sheet(&[row]);
        assert_eq!(sheet.rows[1][1], Cell::text(""));
        assert_eq!(sheet.rows[1][5], Cell::text(""));
    }

    #[test]
    fn an_empty_bom_is_header_only() {
        let sheet = bom_sheet(&[]);
        assert_eq!(sheet.body_len(), 0);
        assert_eq!(sheet.rows.len(), 1);
    }

    #[test]
    fn wire_sheet_has_one_row_per_wire() {
        let rows = vec![
            GeoWireRow { wire: 1, net: 1, from: Point::new(0.0, 0.0), to: Point::new(10.0, 0.0) },
            GeoWireRow { wire: 2, net: 1, from: Point::new(10.0, 0.0), to: Point::new(20.0, 0.0) },
        ];
        let sheet = wire_sheet_from(&rows);
        assert_eq!(sheet.body_len(), 2);
        assert_eq!(sheet.rows[1][0], Cell::Number(1.0));
        assert_eq!(sheet.rows[2][4], Cell::Number(20.0));
    }

    #[test]
    fn terminal_sheet_lists_one_row_per_terminal() {
        let strips = terminal_strips(&[
            ("X1:1".to_string(), vec!["-K1".to_string()]),
            ("X1:2".to_string(), vec![]),
        ]);
        let sheet = terminal_sheet(&strips);
        assert_eq!(sheet.body_len(), 2);
        assert_eq!(sheet.rows[1][2], Cell::text("-K1"));
        assert_eq!(sheet.rows[2][2], Cell::text("unconnected"));
    }

    #[test]
    fn component_helper_is_available_for_future_rows() {
        // Guards against the test module drifting from the public API it exercises.
        let component = Component::new(1, "Contactor", "-K1");
        assert_eq!(component.class, "Contactor");
    }
}
