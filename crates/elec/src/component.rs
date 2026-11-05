//! Components (symbols with attributes) and the bills of material built from them.
//!
//! A component is what a drawing block insert becomes once the electrical layer owns it:
//! a class (Contactor, Terminal, PLC module), a tag, a part number and a set of attribute
//! values. Everything else in the ECAD — reports, cross references, terminal strips — is
//! derived from this record plus the connectivity graph.

use std::collections::BTreeMap;

use crate::expr::{Ref, Resolve, Step};

/// One placed electrical component.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Component {
    /// Stable identity (the entity handle in the host document).
    pub id: u64,
    /// Class name, e.g. `Contactor` or `PLC Module`.
    pub class: String,
    /// Device tag, e.g. `-K1`.
    pub tag: String,
    /// Catalogue part number.
    pub part: Option<String>,
    /// Manufacturer.
    pub manufacturer: Option<String>,
    /// Free description shown in reports.
    pub description: Option<String>,
    /// Location (`+MCC1`, `=A1`) the component sits in.
    pub location: Option<String>,
    /// Number of identical units this record stands for (assemblies, multi-packs).
    pub quantity: u32,
    /// Remaining attributes, keyed by attribute name as the drawing stores it
    /// (`DESC1`, `MFG`, `CAT`, custom class properties).
    pub attributes: BTreeMap<String, String>,
}

impl Component {
    pub fn new(id: u64, class: &str, tag: &str) -> Self {
        Self {
            id,
            class: class.to_string(),
            tag: tag.to_string(),
            quantity: 1,
            ..Self::default()
        }
    }

    pub fn with_part(mut self, part: &str) -> Self {
        self.part = Some(part.to_string());
        self
    }

    pub fn with_manufacturer(mut self, manufacturer: &str) -> Self {
        self.manufacturer = Some(manufacturer.to_string());
        self
    }

    pub fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    pub fn with_location(mut self, location: &str) -> Self {
        self.location = Some(location.to_string());
        self
    }

    pub fn with_quantity(mut self, quantity: u32) -> Self {
        self.quantity = quantity.max(1);
        self
    }

    pub fn with_attribute(mut self, name: &str, value: &str) -> Self {
        self.attributes.insert(name.to_string(), value.to_string());
        self
    }

    /// Look an attribute up by its drawing name, falling back to the well-known fields.
    pub fn attribute(&self, name: &str) -> Option<&str> {
        match name {
            "Tag" | "TAG" | "TAG1" | "WorkID" => Some(self.tag.as_str()),
            "Part" | "PART" | "CAT" | "CatalogueID" => self.part.as_deref(),
            "Manufacturer" | "MFG" => self.manufacturer.as_deref(),
            "Description" | "DESC" | "DESC1" => self.description.as_deref(),
            "Location" | "LOC" | "INST" => self.location.as_deref(),
            "Class" | "ClassName" => Some(self.class.as_str()),
            other => self.attributes.get(other).map(String::as_str),
        }
    }
}

/// Resolve `<r:Parent.Attribute>` and `<r:Attribute>` labels against a component.
///
/// This is what makes a symbol's tag, description or rating text live: the label is
/// stored once in the symbol and evaluates against whichever component owns it.
impl Resolve for Component {
    fn value(&self, path: &Ref) -> Option<String> {
        let names: Vec<&str> = path
            .steps
            .iter()
            .filter_map(|step| match step {
                Step::Prop(name) => Some(name.as_str()),
                Step::Call(_, _) => None,
            })
            .collect();
        // Accept `Parent.X`, `Parent.WorkObject.X` and a bare `X`.
        let name = match names.as_slice() {
            [single] => *single,
            [.., last] => *last,
            [] => return None,
        };
        let base = self.attribute(name)?.to_string();
        match &path.join {
            None => Some(base),
            Some(join) => {
                let mut out = base;
                for prop in &join.props {
                    out.push_str(&join.separator);
                    out.push_str(self.attribute(prop).unwrap_or(""));
                }
                Some(out)
            }
        }
    }
}

/// A line of the bill of material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BomRow {
    pub class: String,
    pub part: Option<String>,
    pub manufacturer: Option<String>,
    pub description: Option<String>,
    /// Total quantity across every component folded into this row.
    pub quantity: u32,
    /// Tags and locations that contributed, sorted and deduplicated — the "where used"
    /// column of the BOM.
    pub where_used: Vec<String>,
}

/// Aggregate components into bill-of-material rows.
///
/// Rows are keyed by `(class, part number)`, matching how the studied report templates
/// group (`Class name` + `Part number` columns). Components without a part number are
/// still reported, grouped by class, because an unassigned part is a finding rather than
/// something to drop. Output is sorted by class then part so exports are stable.
pub fn bill_of_materials(components: &[Component]) -> Vec<BomRow> {
    let mut rows: BTreeMap<(String, String), BomRow> = BTreeMap::new();

    for component in components {
        let key = (
            component.class.clone(),
            component.part.clone().unwrap_or_default(),
        );
        let entry = rows.entry(key).or_insert_with(|| BomRow {
            class: component.class.clone(),
            part: component.part.clone(),
            manufacturer: component.manufacturer.clone(),
            description: component.description.clone(),
            quantity: 0,
            where_used: Vec::new(),
        });
        entry.quantity += component.quantity.max(1);
        // A row keeps the first non-empty description/manufacturer it sees.
        if entry.description.is_none() {
            entry.description = component.description.clone();
        }
        if entry.manufacturer.is_none() {
            entry.manufacturer = component.manufacturer.clone();
        }
        entry.where_used.push(where_used_label(component));
    }

    let mut out: Vec<BomRow> = rows.into_values().collect();
    for row in &mut out {
        row.where_used.sort();
        row.where_used.dedup();
    }
    out
}

/// What goes in the "Where used" column: the location when known, else the tag.
fn where_used_label(component: &Component) -> String {
    match (&component.location, component.tag.is_empty()) {
        (Some(location), false) => format!("{}:{}", location, component.tag),
        (Some(location), true) => location.clone(),
        (None, false) => component.tag.clone(),
        (None, true) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Template;

    fn contactor(tag: &str, part: &str, location: &str) -> Component {
        Component::new(1, "Contactor", tag)
            .with_part(part)
            .with_manufacturer("Siemens")
            .with_description("Contactor 9A")
            .with_location(location)
    }

    #[test]
    fn label_renders_component_attributes() {
        let component = contactor("-K1", "3RT2016-1AB01", "+MCC1");
        let template = Template::parse("<r:Parent.Tag> <r:Parent.Description>");
        assert_eq!(template.render(&component), "-K1 Contactor 9A");
    }

    #[test]
    fn label_accepts_well_known_attribute_aliases() {
        let component = contactor("-K2", "3RT2016-1AB01", "+MCC1");
        assert_eq!(Template::parse("<r:Parent.CAT>").render(&component), "3RT2016-1AB01");
        assert_eq!(Template::parse("<r:Parent.TAG1>").render(&component), "-K2");
        // Unknown attributes render empty rather than failing the label.
        assert_eq!(Template::parse("<r:Parent.Nope>").render(&component), "");
    }

    #[test]
    fn custom_attributes_are_reachable() {
        let component = contactor("-K3", "X", "+MCC1").with_attribute("Current", "9");
        assert_eq!(Template::parse("<r:Parent.Current>").render(&component), "9");
    }

    #[test]
    fn bom_folds_identical_parts_and_sums_quantity() {
        let items = vec![
            contactor("-K1", "3RT2016-1AB01", "+MCC1"),
            contactor("-K2", "3RT2016-1AB01", "+MCC1"),
            contactor("-K3", "3RT2016-1AB01", "+MCC2"),
        ];
        let bom = bill_of_materials(&items);
        assert_eq!(bom.len(), 1);
        assert_eq!(bom[0].quantity, 3);
        assert_eq!(bom[0].part.as_deref(), Some("3RT2016-1AB01"));
        assert_eq!(
            bom[0].where_used,
            vec!["+MCC1:-K1", "+MCC1:-K2", "+MCC2:-K3"]
        );
    }

    #[test]
    fn bom_separates_parts_of_the_same_class() {
        let items = vec![
            contactor("-K1", "A", "+MCC1"),
            contactor("-K2", "B", "+MCC1"),
        ];
        let bom = bill_of_materials(&items);
        assert_eq!(bom.len(), 2);
        assert_eq!(bom[0].part.as_deref(), Some("A"));
        assert_eq!(bom[1].part.as_deref(), Some("B"));
    }

    #[test]
    fn components_without_a_part_are_still_reported() {
        let items = vec![Component::new(9, "Terminal", "X1:1")];
        let bom = bill_of_materials(&items);
        assert_eq!(bom.len(), 1);
        assert!(bom[0].part.is_none());
        assert_eq!(bom[0].quantity, 1);
        assert_eq!(bom[0].where_used, vec!["X1:1"]);
    }

    #[test]
    fn quantity_multiplies_assembly_units() {
        let items = vec![
            Component::new(1, "Terminal", "X1:1").with_part("P").with_quantity(4),
            Component::new(2, "Terminal", "X1:2").with_part("P").with_quantity(2),
        ];
        let bom = bill_of_materials(&items);
        assert_eq!(bom[0].quantity, 6);
    }

    #[test]
    fn bom_ordering_is_stable() {
        let items = vec![
            Component::new(1, "Relay", "K9").with_part("Z"),
            Component::new(2, "Contactor", "K1").with_part("B"),
            Component::new(3, "Contactor", "K2").with_part("A"),
        ];
        let bom = bill_of_materials(&items);
        let keys: Vec<(String, String)> = bom
            .iter()
            .map(|r| (r.class.clone(), r.part.clone().unwrap_or_default()))
            .collect();
        assert_eq!(
            keys,
            vec![
                ("Contactor".to_string(), "A".to_string()),
                ("Contactor".to_string(), "B".to_string()),
                ("Relay".to_string(), "Z".to_string()),
            ]
        );
    }
}
