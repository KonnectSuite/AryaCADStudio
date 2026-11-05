//! Cross references: one device whose parts are spread across the drawing.
//!
//! A contactor is not one symbol. The coil sits in the control circuit, the main contacts
//! sit in the power circuit, and the auxiliary contacts sit wherever they are needed - all
//! of them share one tag. The cross-reference report is what tells an electrician that
//! `-K1` has four parts, which contact each one is, and where to find them.
//!
//! Grouping is by tag, which is the convention these drawings use: every part of a device
//! carries the device's tag, and the contact identifier lives in a separate attribute
//! (`PIN`/`CONTACT` in AutoCAD Electrical, a class property in SkyCAD). When that attribute
//! is present the report prints it, which turns "where is this device" into a table a
//! panel builder can work from.

/// One placed part of a device.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedPart {
    /// Class of the symbol, e.g. `Contactor` or `Contact`.
    pub class: String,
    /// The device tag, e.g. `-K1`.
    pub tag: String,
    /// Contact identifier when the drawing carries one, e.g. `13-14`.
    pub part_id: Option<String>,
    pub x: f64,
    pub y: f64,
}

impl PlacedPart {
    pub fn new(class: &str, tag: &str, x: f64, y: f64) -> Self {
        Self {
            class: class.to_string(),
            tag: tag.to_string(),
            part_id: None,
            x,
            y,
        }
    }

    pub fn with_part_id(mut self, part_id: &str) -> Self {
        let trimmed = part_id.trim();
        self.part_id = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        self
    }
}

/// One part in the report.
#[derive(Debug, Clone, PartialEq)]
pub struct PartRef {
    pub class: String,
    /// Contact identifier, when known.
    pub part_id: Option<String>,
    pub x: f64,
    pub y: f64,
}

/// A device and every part of it found on the drawing.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceRef {
    pub tag: String,
    pub parts: Vec<PartRef>,
}

impl DeviceRef {
    /// True when the device has more than one part, i.e. it is worth cross-referencing.
    pub fn is_multi_part(&self) -> bool {
        self.parts.len() > 1
    }

    /// Cross-reference text such as `13-14, 21-22` for the parts that carry an identifier.
    pub fn contact_list(&self) -> String {
        let mut ids: Vec<String> = self
            .parts
            .iter()
            .filter_map(|part| part.part_id.clone())
            .collect();
        ids.sort();
        ids.dedup();
        ids.join(", ")
    }

    /// What to print beside one part: the *other* parts of the same device.
    ///
    /// A contact identifier is used when the drawing carries one, otherwise the part's
    /// position - the same fallback the report uses. The part itself is excluded, because a
    /// reference that points at the symbol you are looking at is noise.
    pub fn reference_for(&self, index: usize) -> String {
        let mut others: Vec<String> = self
            .parts
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != index)
            .map(|(_, part)| match &part.part_id {
                Some(id) => id.clone(),
                None => format!("{},{}", format_coordinate(part.x), format_coordinate(part.y)),
            })
            .collect();
        others.sort();
        others.dedup();
        others.join(", ")
    }
}

/// Coordinates in cross-reference text: no trailing zeros, one decimal at most.
fn format_coordinate(value: f64) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    if (rounded - rounded.trunc()).abs() < f64::EPSILON {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}

/// Group placed parts into devices by tag.
///
/// Parts inside a device are ordered top-to-bottom then left-to-right, and devices are
/// returned in tag order, so two runs over the same drawing produce the same report.
/// Parts with an empty tag are skipped: an untagged symbol cannot be cross-referenced, and
/// inventing a group for it would hide the fact that it needs a tag.
pub fn cross_references(parts: &[PlacedPart]) -> Vec<DeviceRef> {
    use std::collections::BTreeMap;

    let mut by_tag: BTreeMap<String, Vec<PartRef>> = BTreeMap::new();
    for part in parts {
        let tag = part.tag.trim();
        if tag.is_empty() {
            continue;
        }
        by_tag.entry(tag.to_string()).or_default().push(PartRef {
            class: part.class.trim().to_string(),
            part_id: part.part_id.clone(),
            x: part.x,
            y: part.y,
        });
    }

    by_tag
        .into_iter()
        .map(|(tag, mut parts)| {
            parts.sort_by(|a, b| {
                let a_key = (-(a.y * 100.0).round() as i64, (a.x * 100.0).round() as i64);
                let b_key = (-(b.y * 100.0).round() as i64, (b.x * 100.0).round() as i64);
                a_key.cmp(&b_key)
            });
            DeviceRef { tag, parts }
        })
        .collect()
}

/// Devices with more than one part, i.e. the ones a cross-reference table lists.
pub fn multi_part(devices: &[DeviceRef]) -> Vec<&DeviceRef> {
    devices.iter().filter(|device| device.is_multi_part()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(class: &str, tag: &str, x: f64, y: f64) -> PlacedPart {
        PlacedPart::new(class, tag, x, y)
    }

    #[test]
    fn parts_of_one_device_are_grouped() {
        let input = vec![part("Contactor", "-K1", 0.0, 0.0), part("Contact", "-K1", 100.0, 0.0)];
        let devices = cross_references(&input);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].tag, "-K1");
        assert!(devices[0].is_multi_part());
        assert_eq!(devices[0].parts.len(), 2);
    }

    #[test]
    fn single_part_devices_are_kept_but_not_multi() {
        let devices = cross_references(&[part("Contactor", "-K2", 0.0, 0.0)]);
        assert_eq!(devices.len(), 1);
        assert!(!devices[0].is_multi_part());
        assert!(multi_part(&devices).is_empty());
    }

    #[test]
    fn parts_are_ordered_top_down_then_left_right() {
        let input = vec![
            part("Contact", "-K1", 100.0, 0.0),
            part("Contactor", "-K1", 0.0, 100.0),
            part("Contact", "-K1", 0.0, 0.0),
        ];
        let devices = cross_references(&input);
        let classes: Vec<&str> = devices[0].parts.iter().map(|p| p.class.as_str()).collect();
        assert_eq!(classes, vec!["Contactor", "Contact", "Contact"]);
        assert_eq!(devices[0].parts[1].x, 0.0);
        assert_eq!(devices[0].parts[2].x, 100.0);
    }

    #[test]
    fn devices_are_returned_in_tag_order() {
        let input = vec![part("Contactor", "-K2", 0.0, 0.0), part("Contactor", "-K1", 0.0, 0.0)];
        let tags: Vec<String> = cross_references(&input).iter().map(|d| d.tag.clone()).collect();
        assert_eq!(tags, vec!["-K1", "-K2"]);
    }

    #[test]
    fn untagged_parts_are_skipped() {
        let input = vec![
            part("Contactor", "", 0.0, 0.0),
            part("Contactor", "   ", 10.0, 0.0),
            part("Contactor", "-K1", 0.0, 0.0),
        ];
        let devices = cross_references(&input);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].tag, "-K1");
    }

    #[test]
    fn tags_are_trimmed_before_grouping() {
        let input = vec![part("Contactor", " -K1", 0.0, 0.0), part("Contact", "-K1 ", 10.0, 0.0)];
        let devices = cross_references(&input);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].parts.len(), 2);
    }

    #[test]
    fn contact_identifiers_are_collected_sorted_and_deduped() {
        let input = vec![
            part("Contact", "-K1", 0.0, 0.0).with_part_id("21-22"),
            part("Contact", "-K1", 10.0, 0.0).with_part_id("13-14"),
            part("Contact", "-K1", 20.0, 0.0).with_part_id("13-14"),
            part("Contactor", "-K1", 30.0, 0.0),
        ];
        let devices = cross_references(&input);
        assert_eq!(devices[0].contact_list(), "13-14, 21-22");
    }

    #[test]
    fn a_blank_contact_identifier_is_not_a_value() {
        let part = PlacedPart::new("Contact", "-K1", 0.0, 0.0).with_part_id("   ");
        assert!(part.part_id.is_none());
    }

    #[test]
    fn contact_list_is_empty_when_no_part_carries_an_identifier() {
        let devices = cross_references(&[
            part("Contactor", "-K1", 0.0, 0.0),
            part("Contact", "-K1", 10.0, 0.0),
        ]);
        assert_eq!(devices[0].contact_list(), "");
    }

    #[test]
    fn reference_for_a_part_lists_the_other_parts() {
        let input = vec![
            PlacedPart::new("Contactor", "-K1", 0.0, 0.0),
            PlacedPart::new("Contact", "-K1", 100.0, 0.0).with_part_id("13-14"),
            PlacedPart::new("Contact", "-K1", 100.0, 100.0).with_part_id("21-22"),
        ];
        let device = &cross_references(&input)[0];
        // Parts are ordered by position, so find each one instead of assuming an index.
        let coil = device
            .parts
            .iter()
            .position(|part| part.class == "Contactor")
            .unwrap();
        let first_contact = device
            .parts
            .iter()
            .position(|part| part.part_id.as_deref() == Some("13-14"))
            .unwrap();
        // The coil references both contacts by identifier.
        assert_eq!(device.reference_for(coil), "13-14, 21-22");
        // A contact references the other contact and the coil's position.
        let from_contact = device.reference_for(first_contact);
        assert!(from_contact.contains("21-22"));
        assert!(from_contact.contains("0,0"));
        assert!(!from_contact.contains("13-14"));
    }

    #[test]
    fn reference_falls_back_to_position_without_identifiers() {
        let input = vec![
            PlacedPart::new("Contactor", "-K1", 0.0, 0.0),
            PlacedPart::new("Contact", "-K1", 12.5, -3.0),
        ];
        let device = &cross_references(&input)[0];
        assert_eq!(device.reference_for(0), "12.5,-3");
    }

    #[test]
    fn a_single_part_has_no_reference() {
        let device = &cross_references(&[PlacedPart::new("Contactor", "-K1", 0.0, 0.0)])[0];
        assert_eq!(device.reference_for(0), "");
    }

    #[test]
    fn nothing_in_nothing_out() {
        assert!(cross_references(&[]).is_empty());
    }
}
