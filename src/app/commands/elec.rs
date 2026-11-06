//! Electrical (ECAD) commands.
//!
//! These are the host-side features of the electrical layer. They read placed symbols
//! from the drawing, turn each block insert and its attributes into an `elec::Component`,
//! and report from that model. Everything heavier (numbering, terminal strips, cross
//! references) builds on the same extraction.
//!
//! Commands are dispatched by name like every other CAD command, so they are reachable
//! from the command line and from a ribbon tool.

use super::{Message, OpenCADStudio};
use ::elec::{bill_of_materials, Component};
use iced::Task;
use std::collections::{BTreeMap, BTreeSet};

/// The parts catalogue loaded for this session, if any.
///
/// A catalogue belongs to the user's machine rather than to one drawing, so it lives beside
/// the app instead of inside a document. It is loaded explicitly with `ELECPART` and stays
/// until replaced; `ELECBOM` uses it to fill manufacturer and description for parts the
/// drawing names but does not describe.
static CATALOGUE: std::sync::OnceLock<std::sync::RwLock<Option<::elec::Catalogue>>> =
    std::sync::OnceLock::new();

fn catalogue() -> &'static std::sync::RwLock<Option<::elec::Catalogue>> {
    CATALOGUE.get_or_init(|| std::sync::RwLock::new(None))
}

/// Folder the last `ELECLIB` loaded symbols from.
static LIBRARY: std::sync::OnceLock<std::sync::RwLock<Option<std::path::PathBuf>>> =
    std::sync::OnceLock::new();

fn library_dir() -> &'static std::sync::RwLock<Option<std::path::PathBuf>> {
    LIBRARY.get_or_init(|| std::sync::RwLock::new(None))
}

/// Where the last library folder is remembered between runs.
///
/// A library belongs to the machine, not the drawing, so it should not have to be retyped
/// every session - and a path is exactly the thing that is painful to type in the GUI.
fn library_config_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("APPDATA").map(std::path::PathBuf::from)?;
    Some(base.join("AryaCAD").join("elec_library.txt"))
}

fn remember_library(directory: &std::path::Path) {
    let Some(path) = library_config_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, directory.display().to_string());
}

fn remembered_library() -> Option<std::path::PathBuf> {
    let path = library_config_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    let directory = std::path::PathBuf::from(text.trim());
    if directory.is_dir() {
        Some(directory)
    } else {
        None
    }
}

/// One symbol's catalogue entry: which class it belongs to and which attributes an
/// instance of it carries.
#[derive(Debug, Clone, Default)]
struct LibraryEntry {
    /// The symbol's name as the library spells it, for the palette.
    name: String,
    class: String,
    attributes: Vec<String>,
    /// Attribute text height that suits this symbol's own size, measured by `ELECLIB`
    /// from the loaded geometry. Zero means "not measured".
    label_height: f64,
    /// Scale the source drawings insert this block at; 1.0 when they use none.
    scale: f64,
}

/// Folder the last `ELECPROJECT` scanned, so module addressing can continue the
/// project's I/O sequence instead of restarting it on every sheet.
static PROJECT_DIR: std::sync::OnceLock<std::sync::RwLock<Option<std::path::PathBuf>>> =
    std::sync::OnceLock::new();

/// One drawing in a project: where it is, and the metadata the project file gives it.
#[derive(Debug, Clone)]
struct ProjectSheet {
    path: std::path::PathBuf,
    number: String,
    title: String,
}

/// The project file currently open, if any.
static PROJECT_FILE: std::sync::OnceLock<std::sync::RwLock<Option<std::path::PathBuf>>> =
    std::sync::OnceLock::new();

fn project_file() -> &'static std::sync::RwLock<Option<std::path::PathBuf>> {
    PROJECT_FILE.get_or_init(|| std::sync::RwLock::new(None))
}

fn project_dir() -> &'static std::sync::RwLock<Option<std::path::PathBuf>> {
    PROJECT_DIR.get_or_init(|| std::sync::RwLock::new(None))
}

static LIBRARY_INDEX: std::sync::OnceLock<
    std::sync::RwLock<std::collections::BTreeMap<String, LibraryEntry>>,
> = std::sync::OnceLock::new();

fn library_index() -> &'static std::sync::RwLock<std::collections::BTreeMap<String, LibraryEntry>> {
    LIBRARY_INDEX.get_or_init(|| std::sync::RwLock::new(std::collections::BTreeMap::new()))
}

/// Read `library.tsv`: symbol, class, `|`-separated attribute tags.
///
/// The tags matter because these symbols keep their attributes on the *insert*, not as
/// attribute definitions inside the block, so the set has to be recorded next to the
/// geometry or an inserted symbol comes in mute.
fn read_library_manifest(
    directory: &std::path::Path,
) -> std::collections::BTreeMap<String, LibraryEntry> {
    let mut index = std::collections::BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(directory.join("library.tsv")) else {
        return index;
    };
    for (number, line) in text.lines().enumerate() {
        if number == 0 || line.trim().is_empty() {
            continue;
        }
        let mut parts = line.split('\t');
        let symbol = parts.next().unwrap_or("").trim();
        let class = parts.next().unwrap_or("").trim();
        let attributes = parts.next().unwrap_or("").trim();
        // Older manifests have no scale column; one means "as drawn".
        let scale = parts
            .next()
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|value| *value > 0.0)
            .unwrap_or(1.0);
        if symbol.is_empty() {
            continue;
        }
        index.insert(
            symbol.to_ascii_uppercase(),
            LibraryEntry {
                name: symbol.to_string(),
                class: class.to_string(),
                attributes: if attributes.is_empty() {
                    Vec::new()
                } else {
                    attributes.split('|').map(|tag| tag.to_string()).collect()
                },
                // Measured once the geometry is loaded, not by the manifest reader.
                label_height: 0.0,
                scale,
            },
        );
    }
    index
}

/// Attribute names that carry the well-known component fields.
///
/// The set covers the conventions this project has to read: AutoCAD Electrical's
/// `TAG1`/`CAT`/`MFG`/`DESC`/`LOC`, plain `TAG`/`PART`, and SkyCAD-style long names.
fn field_for_attribute(tag: &str) -> Option<&'static str> {
    match tag.trim().to_ascii_uppercase().as_str() {
        "TAG" | "TAG1" | "TAG2" | "TAGSTR" | "WORKID" => Some("tag"),
        "CAT" | "PART" | "PARTNUMBER" | "CATALOGUEID" | "CATALOG" => Some("part"),
        "MFG" | "MANUFACTURER" | "MFR" => Some("manufacturer"),
        "DESC" | "DESC1" | "DESC2" | "DESCRIPTION" => Some("description"),
        "LOC" | "LOCATION" | "INST" | "INSTALLATION" => Some("location"),
        _ => None,
    }
}

impl OpenCADStudio {
    /// Turn every block insert in the active tab into a component.
    ///
    /// Inserts with no attributes are skipped: a bare block is drawing furniture (a
    /// border, a title block, a graphic) rather than an electrical device, and counting
    /// them would make the bill of material meaningless.
/// Components of any document, so the same extraction serves one drawing and a project.
fn components_of(document: &codec::Document) -> Vec<Component> {
    let model_space = document.header.model_space_block_handle;
    let mut out = Vec::new();
    let mut next_id = 0u64;
    for entity in document.entities() {
        if entity.common().owner_handle != model_space {
            continue;
        }
        let codec::EntityType::Insert(insert) = entity else {
            continue;
        };
        if insert.attributes.is_empty() || is_setup_block(&insert.block_name) {
            continue;
        }
        next_id += 1;
        let mut component = Component::new(next_id, &insert.block_name, "");
        for attribute in &insert.attributes {
            let tag = attribute.tag.trim();
            if tag.is_empty() {
                continue;
            }
            let value = attribute.value.trim();
            if value.is_empty() {
                continue;
            }
            match field_for_attribute(tag) {
                Some("tag") => component.tag = value.to_string(),
                Some("part") => component.part = Some(value.to_string()),
                Some("manufacturer") => component.manufacturer = Some(value.to_string()),
                Some("description") => component.description = Some(value.to_string()),
                Some("location") => component.location = Some(value.to_string()),
                _ => {}
            }
            component = component.with_attribute(tag, value);
        }
        out.push(component);
    }
    out
}


    fn elec_components(&self, i: usize) -> Vec<Component> {
        Self::components_of(&self.tabs[i].scene.document)
    }


    /// `ELECBOM` prints the bill of material of the active drawing.
    ///
    /// `ELECBOM <file.csv>` also writes the rows to a CSV file, using the column names
    /// the shipped report templates use, so a report can be produced without a GUI.
    pub(super) fn elec_bom(&mut self, i: usize, path: Option<&str>) -> Task<Message> {
        let mut components = self.elec_components(i);
        if components.is_empty() {
            self.command_line.push_output(
                "ELECBOM: no electrical components found (no block insert with attributes).",
            );
            return Task::none();
        }
        // A loaded catalogue supplies the manufacturer and description the drawing does not
        // carry. Values on the drawing win, so this only fills gaps.
        let mut enriched = 0usize;
        if let Ok(guard) = catalogue().read() {
            if let Some(catalogue) = guard.as_ref() {
                for component in &mut components {
                    if catalogue.enrich(component) {
                        enriched += 1;
                    }
                }
            }
        }
        let rows = bill_of_materials(&components);
        let summary = format!(
            "ELECBOM: {} component(s) -> {} bill-of-material row(s).",
            components.len(),
            rows.len()
        );
        self.command_line.push_output(&summary);
        if enriched > 0 {
            let line = format!("  {enriched} component(s) completed from the parts catalogue");
            self.command_line.push_output(&line);
        }
        let header = format!(
            "{:<22} {:<26} {:<16} {:>4}  {:<28} {}",
            "CLASS", "PART", "MANUFACTURER", "QTY", "WHERE USED", "DESCRIPTION"
        );
        self.command_line.push_output(&header);
        for row in &rows {
            let where_used = row.where_used.join(", ");
            let line = format!(
                "{:<22} {:<26} {:<16} {:>4}  {:<28} {}",
                row.class,
                row.part.clone().unwrap_or_else(|| "-".to_string()),
                row.manufacturer.clone().unwrap_or_else(|| "-".to_string()),
                row.quantity,
                truncate(&where_used, 28),
                row.description.clone().unwrap_or_default(),
            );
            self.command_line.push_output(&line);
        }
        if let Some(path) = path {
            match write_bom_csv(path, &rows) {
                Ok(()) => {
                    let written = format!("ELECBOM: wrote {} row(s) to {path}", rows.len());
                    self.command_line.push_output(&written);
                }
                Err(error) => {
                    let message = format!("ELECBOM: cannot write {path}: {error}");
                    self.command_line.push_output(&message);
                }
            }
        }
        Task::none()
    }

    /// `ELECLIST` lists the components the extractor found, one per line.
    pub(super) fn elec_list(&mut self, i: usize) -> Task<Message> {
        let components = self.elec_components(i);
        if components.is_empty() {
            self.command_line
                .push_output("ELECLIST: no electrical components found.");
            return Task::none();
        }
        let header = format!(
            "{:<22} {:<12} {:<26} {:<20} {}",
            "CLASS", "TAG", "PART", "LOCATION", "DESCRIPTION"
        );
        self.command_line.push_output(&header);
        for component in &components {
            let line = format!(
                "{:<22} {:<12} {:<26} {:<20} {}",
                component.class,
                component.tag,
                component.part.clone().unwrap_or_else(|| "-".to_string()),
                component.location.clone().unwrap_or_else(|| "-".to_string()),
                component.description.clone().unwrap_or_default(),
            );
            self.command_line.push_output(&line);
        }
        Task::none()
    }

    /// `ELECNUMBER` assigns device tags to the components of the active drawing.
    ///
    /// Components are grouped by class, ordered top-to-bottom then left-to-right, and
    /// numbered with the prefix already used on the sheet (or the class's IEC letter when
    /// there is none). Structured tags such as `X1:1` are left alone. `ELECNUMBER ?` only
    /// reports what would change.
    pub(super) fn elec_number(&mut self, i: usize, dry_run: bool) -> Task<Message> {
        use ::elec::{default_prefix, infer_tag_prefix, reading_order_key, renumber, Numbered, NumberingFormat};
        use std::collections::BTreeMap;

        struct Item {
            handle: codec::Handle,
            class: String,
            tag: String,
            x: f64,
            y: f64,
        }

        let mut items: Vec<Item> = Vec::new();
        {
            let doc = &self.tabs[i].scene.document;
            let model_space = doc.header.model_space_block_handle;
            for entity in doc.entities() {
                if entity.common().owner_handle != model_space { continue; }
                let codec::EntityType::Insert(insert) = entity else {
                    continue;
                };
                if insert.attributes.is_empty() || is_setup_block(&insert.block_name) {
                    continue;
                }
                let tag = insert
                    .attributes
                    .iter()
                    .find(|attribute| is_tag_attribute(&attribute.tag))
                    .map(|attribute| attribute.value.trim().to_string())
                    .unwrap_or_default();
                items.push(Item {
                    handle: insert.common.handle,
                    class: insert.block_name.clone(),
                    tag,
                    x: insert.insert_point.x,
                    y: insert.insert_point.y,
                });
            }
        }
        if items.is_empty() {
            self.command_line
                .push_output("ELECNUMBER: no electrical components found.");
            return Task::none();
        }

        let mut by_class: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        // Group by tag *prefix*, not by block or class: a relay (HCR1) and a contactor
        // (CONT-NO) are both -K devices and must share one sequence, or each family
        // restarts at -K1 and two different devices end up with the same tag.
        let class_of = |block: &str| -> String {
            library_index()
                .read()
                .ok()
                .and_then(|index| {
                    index
                        .get(&block.to_ascii_uppercase())
                        .map(|entry| entry.class.clone())
                })
                .unwrap_or_else(|| block.to_string())
        };
        for (index, item) in items.iter().enumerate() {
            let block_class = class_of(&item.class);
            let prefix = if item.tag.is_empty() {
                default_prefix(&block_class)
            } else {
                infer_tag_prefix(&[item.tag.as_str()])
                    .unwrap_or_else(|| default_prefix(&block_class))
            };
            by_class.entry(prefix).or_default().push(index);
        }

        let mut changes: Vec<(codec::Handle, String)> = Vec::new();
        let mut report: Vec<String> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();

        for (class, mut indices) in by_class {
            let existing: Vec<&str> = indices
                .iter()
                .map(|index| items[*index].tag.as_str())
                .filter(|tag| !tag.is_empty())
                .collect();
            let prefix = match infer_tag_prefix(&existing) {
                Some(prefix) => prefix,
                None if existing.is_empty() => default_prefix(&class),
                None => {
                    // Structured tags (strip:terminal, location/device) keep their own rules.
                    skipped.push(class.clone());
                    continue;
                }
            };
            // Parts of one device share a tag - a coil and its contacts are one device. Group
            // by tag first so renumbering assigns one number per *device*; numbering each
            // part separately split a device into three and renamed its parts -K2, -K3, -K1.
            let mut devices: BTreeMap<String, Vec<usize>> = BTreeMap::new();
            for index in indices.drain(..) {
                let key = {
                    let tag = items[index].tag.trim();
                    if tag.is_empty() {
                        format!("#{index}")
                    } else {
                        tag.to_ascii_uppercase()
                    }
                };
                devices.entry(key).or_default().push(index);
            }
            let mut device_list: Vec<Vec<usize>> = devices.into_values().collect();
            device_list.sort_by_key(|parts| {
                let first = parts[0];
                reading_order_key(items[first].x, items[first].y)
            });
            let format = NumberingFormat::tag(&prefix);
            let numbered: Vec<Numbered> = device_list
                .iter()
                .enumerate()
                .map(|(device, parts)| {
                    let item = &items[parts[0]];
                    Numbered {
                        id: device as u64,
                        current: if item.tag.is_empty() {
                            None
                        } else {
                            Some(item.tag.clone())
                        },
                        frozen: false,
                    }
                })
                .collect();
            let results = renumber(&format, &numbered);
            let mut assigned = Vec::new();
            let mut changed = 0usize;
            for result in &results {
                // One result per device; every part of that device takes the same tag.
                let parts = &device_list[result.id as usize];
                for part in parts {
                    let item = &items[*part];
                    if item.tag != result.value {
                        changed += 1;
                        changes.push((item.handle, result.value.clone()));
                    }
                }
                assigned.push(result.value.clone());
            }
            report.push(format!(
                "  {class}: {} ({changed} changed)",
                assigned.join(", ")
            ));
        }

        let verb = if dry_run { "would write" } else { "wrote" };
        let summary = format!(
            "ELECNUMBER: {} component(s) in {} class(es); {verb} {} tag(s).",
            items.len(),
            report.len(),
            changes.len()
        );
        self.command_line.push_output(&summary);
        for line in &report {
            self.command_line.push_output(line);
        }
        for class in &skipped {
            let line = format!("  {class}: skipped (structured tags)");
            self.command_line.push_output(&line);
        }

        if dry_run || changes.is_empty() {
            return Task::none();
        }
        self.push_undo_snapshot(i, "ELECNUMBER");
        {
            let doc = &mut self.tabs[i].scene.document;
            for (handle, value) in &changes {
                let Some(codec::EntityType::Insert(insert)) = doc.get_entity_mut(*handle) else {
                    continue;
                };
                for attribute in &mut insert.attributes {
                    if is_tag_attribute(&attribute.tag) {
                        attribute.value = value.clone();
                    }
                }
            }
        }
        self.tabs[i].dirty = true;
        Task::none()
    }

    /// `ELECWIRELIST` derives connectivity from the drawn wires and reports the nets.
    ///
    /// Wires are LINE and LWPOLYLINE entities; a component anchors to a net where its
    /// insertion point meets a wire endpoint. Points within `tolerance` are the same node,
    /// so a wire drawn to a symbol pin connects without pixel-perfect coordinates.
    pub(super) fn elec_wirelist(&mut self, i: usize) -> Task<Message> {
        use ::elec::geometry::{netlist_from_geometry, Anchor, Point, WireSeg};
        use ::elec::infer_tag_prefix;

        const TOLERANCE: f64 = 0.05;

        let mut wires: Vec<WireSeg> = Vec::new();
        let mut anchors: Vec<Anchor> = Vec::new();
        let mut labels: std::collections::BTreeMap<u64, String> = std::collections::BTreeMap::new();

        {
            let doc = &self.tabs[i].scene.document;
            let model_space = doc.header.model_space_block_handle;
            let mut next_wire = 0u64;
            let mut next_anchor = 0u64;
            for entity in doc.entities() {
                if entity.common().owner_handle != model_space { continue; }
                match entity {
                    codec::EntityType::Line(line) => {
                        next_wire += 1;
                        wires.push(WireSeg::new(
                            next_wire,
                            Point::new(line.start.x, line.start.y),
                            Point::new(line.end.x, line.end.y),
                        ));
                    }
                    codec::EntityType::LwPolyline(polyline) => {
                        let points: Vec<Point> = polyline
                            .vertices
                            .iter()
                            .map(|vertex| Point::new(vertex.location.x, vertex.location.y))
                            .collect();
                        for pair in points.windows(2) {
                            next_wire += 1;
                            wires.push(WireSeg::new(next_wire, pair[0], pair[1]));
                        }
                        if polyline.is_closed && points.len() > 2 {
                            next_wire += 1;
                            wires.push(WireSeg::new(
                                next_wire,
                                *points.last().unwrap(),
                                points[0],
                            ));
                        }
                    }
                    codec::EntityType::Insert(insert)
                        if !insert.attributes.is_empty() && !is_setup_block(&insert.block_name) =>
                    {
                        next_anchor += 1;
                        let tag = insert
                            .attributes
                            .iter()
                            .find(|attribute| is_tag_attribute(&attribute.tag))
                            .map(|attribute| attribute.value.trim().to_string())
                            .unwrap_or_default();
                        let label = if tag.is_empty() {
                            insert.block_name.clone()
                        } else {
                            tag
                        };
                        labels.insert(next_anchor, label.clone());
                        anchors.push(Anchor::new(
                            next_anchor,
                            &label,
                            Point::new(insert.insert_point.x, insert.insert_point.y),
                        ));
                    }
                    _ => {}
                }
            }
        }

        if wires.is_empty() {
            self.command_line
                .push_output("ELECWIRELIST: no wires found (LINE or LWPOLYLINE entities).");
            return Task::none();
        }

        let netlist = netlist_from_geometry(&wires, &anchors, TOLERANCE);
        let summary = format!(
            "ELECWIRELIST: {} wire segment(s), {} component(s) -> {} net(s); {} unconnected component(s), {} loose wire(s).",
            wires.len(),
            anchors.len(),
            netlist.nets.len(),
            netlist.loose_anchors.len(),
            netlist.loose_wires.len()
        );
        self.command_line.push_output(&summary);

        for net in &netlist.nets {
            let members: Vec<&str> = net
                .anchors
                .iter()
                .map(|id| labels.get(id).map(String::as_str).unwrap_or("?"))
                .collect();
            let line = format!(
                "  net {}: {} wire(s) -> {}",
                net.id,
                net.wires.len(),
                if members.is_empty() {
                    "(no components)".to_string()
                } else {
                    members.join(", ")
                }
            );
            self.command_line.push_output(&line);
        }

        if !netlist.loose_anchors.is_empty() {
            let names: Vec<&str> = netlist
                .loose_anchors
                .iter()
                .map(|id| labels.get(id).map(String::as_str).unwrap_or("?"))
                .collect();
            let line = format!("  unconnected: {}", names.join(", "));
            self.command_line.push_output(&line);
        }

        // A drawing that names its devices is a drawing we can sanity-check: report the
        // prefixes in use so a numbering mistake is visible from the same command.
        let tags: Vec<&str> = labels.values().map(String::as_str).collect();
        if let Some(prefix) = infer_tag_prefix(&tags) {
            let line = format!("  device prefix: {prefix}");
            self.command_line.push_output(&line);
        }

        Task::none()
    }

    /// `ELECWIRENUMBER` numbers the nets and labels each one on the drawing.
    ///
    /// Nets are numbered in reading order (top-to-bottom, then left-to-right) using the
    /// wire numbering format, and a text label is placed at the midpoint of the net's first
    /// wire on layer `WIRENO`. An existing label at the same point is updated rather than
    /// duplicated, so the command is safe to run again after a design change.
    pub(super) fn elec_wirenumber(&mut self, i: usize) -> Task<Message> {
        use ::elec::geometry::{netlist_from_geometry, Anchor, Point, WireSeg};
        use ::elec::{reading_order_key, NumberingFormat};

        const TOLERANCE: f64 = 0.05;
        const LABEL_LAYER: &str = "WIRENO";
        const LABEL_HEIGHT: f64 = 2.5;

        let mut wires: Vec<WireSeg> = Vec::new();
        let mut anchors: Vec<Anchor> = Vec::new();
        let mut existing: Vec<(codec::Handle, String, f64, f64)> = Vec::new();
        {
            let doc = &self.tabs[i].scene.document;
            let model_space = doc.header.model_space_block_handle;
            let mut next_wire = 0u64;
            let mut next_anchor = 0u64;
            for entity in doc.entities() {
                if entity.common().owner_handle != model_space { continue; }
                match entity {
                    codec::EntityType::Line(line) => {
                        next_wire += 1;
                        wires.push(WireSeg::new(
                            next_wire,
                            Point::new(line.start.x, line.start.y),
                            Point::new(line.end.x, line.end.y),
                        ));
                    }
                    codec::EntityType::LwPolyline(polyline) => {
                        let points: Vec<Point> = polyline
                            .vertices
                            .iter()
                            .map(|vertex| Point::new(vertex.location.x, vertex.location.y))
                            .collect();
                        for pair in points.windows(2) {
                            next_wire += 1;
                            wires.push(WireSeg::new(next_wire, pair[0], pair[1]));
                        }
                    }
                    codec::EntityType::Insert(insert)
                        if !insert.attributes.is_empty() && !is_setup_block(&insert.block_name) =>
                    {
                        next_anchor += 1;
                        let label = insert
                            .attributes
                            .iter()
                            .find(|attribute| is_tag_attribute(&attribute.tag))
                            .map(|attribute| attribute.value.trim().to_string())
                            .filter(|value| !value.is_empty())
                            .unwrap_or_else(|| insert.block_name.clone());
                        anchors.push(Anchor::new(
                            next_anchor,
                            &label,
                            Point::new(insert.insert_point.x, insert.insert_point.y),
                        ));
                    }
                    codec::EntityType::Text(text)
                        if text.common.layer.eq_ignore_ascii_case(LABEL_LAYER) =>
                    {
                        existing.push((
                            text.common.handle,
                            text.value.clone(),
                            text.insertion_point.x,
                            text.insertion_point.y,
                        ));
                    }
                    _ => {}
                }
            }
        }

        let netlist = netlist_from_geometry(&wires, &anchors, TOLERANCE);
        if netlist.nets.is_empty() {
            self.command_line
                .push_output("ELECWIRENUMBER: no connected nets found.");
            return Task::none();
        }

        // Order nets by where their first wire sits, so numbers follow the drawing.
        let mut ordered: Vec<(i64, i64, u64)> = Vec::new();
        for net in &netlist.nets {
            // A net of coincident symbols has no wires and nothing to label.
            let Some(first_wire) = net.wires.first().copied() else {
                continue;
            };
            let Some(first) = wires.iter().find(|wire| wire.id == first_wire) else {
                continue;
            };
            let mid_x = (first.from.x + first.to.x) / 2.0;
            let mid_y = (first.from.y + first.to.y) / 2.0;
            let (row, column) = reading_order_key(mid_x, mid_y);
            ordered.push((row, column, net.id));
        }
        ordered.sort_unstable();

        let format = NumberingFormat::wire();
        let mut updates: Vec<(codec::Handle, String)> = Vec::new();
        let mut creates: Vec<(String, f64, f64)> = Vec::new();
        let mut report: Vec<String> = Vec::new();

        for (ordinal, (_row, _column, net_id)) in ordered.iter().enumerate() {
            let net = netlist.nets.iter().find(|net| net.id == *net_id).unwrap();
            let number = format.value(ordinal as u32);
            let Some(first_wire) = net.wires.first().copied() else {
                continue;
            };
            let Some(first) = wires.iter().find(|wire| wire.id == first_wire) else {
                continue;
            };
            let x = (first.from.x + first.to.x) / 2.0;
            let y = (first.from.y + first.to.y) / 2.0 + 1.0;
            let reuse = existing.iter().find(|(_, _, ex, ey)| {
                (ex - x).abs() <= 0.1 && (ey - y).abs() <= 0.1
            });
            let names: Vec<&str> = net
                .anchors
                .iter()
                .map(|id| {
                    anchors
                        .iter()
                        .find(|anchor| anchor.id == *id)
                        .map(|anchor| anchor.label.as_str())
                        .unwrap_or("?")
                })
                .collect();
            match reuse {
                Some((handle, current, _, _)) if current == &number => {
                    report.push(format!("  {number}: net {net_id} (unchanged)"));
                }
                Some((handle, _, _, _)) => {
                    updates.push((*handle, number.clone()));
                    report.push(format!(
                        "  {number}: net {net_id} ({} -> {number})",
                        names.join(", ")
                    ));
                }
                None => {
                    creates.push((number.clone(), x, y));
                    report.push(format!("  {number}: net {net_id} ({})", names.join(", ")));
                }
            }
        }

        let summary = format!(
            "ELECWIRENUMBER: {} net(s); {} label(s) created, {} updated.",
            ordered.len(),
            creates.len(),
            updates.len()
        );
        self.command_line.push_output(&summary);
        for line in &report {
            self.command_line.push_output(line);
        }

        if creates.is_empty() && updates.is_empty() {
            return Task::none();
        }
        self.push_undo_snapshot(i, "ELECWIRENUMBER");
        self.tabs[i].scene.ensure_layer(LABEL_LAYER);
        for (handle, value) in updates {
            let doc = &mut self.tabs[i].scene.document;
            if let Some(codec::EntityType::Text(text)) = doc.get_entity_mut(handle) {
                text.value = value;
            }
        }
        for (value, x, y) in creates {
            let mut text = codec::entities::Text::new();
            text.value = value;
            text.insertion_point = codec::types::Vector3::new(x, y, 0.0);
            text.height = LABEL_HEIGHT;
            text.common.layer = LABEL_LAYER.to_string();
            self.tabs[i].scene.add_entity(codec::EntityType::Text(text));
        }
        self.tabs[i].dirty = true;
        Task::none()
    }

    /// Collect the drawing's wire segments and component anchors.
    fn elec_collect(
        &self,
        i: usize,
    ) -> (
        Vec<::elec::geometry::WireSeg>,
        Vec<::elec::geometry::Anchor>,
    ) {
        use ::elec::geometry::{Anchor, Point, WireSeg};
        let doc = &self.tabs[i].scene.document;
        // Model space only. `doc.entities()` also walks block definitions, and a drawing
        // with a symbol library loaded holds hundreds of block-geometry lines: counting
        // those as wires produced 24,950 "wire segments" on a three-wire drawing.
        let model_space = doc.header.model_space_block_handle;
        let mut wires = Vec::new();
        let mut anchors = Vec::new();
        let mut next_wire = 0u64;
        let mut next_anchor = 0u64;
        for entity in doc.entities() {
            if entity.common().owner_handle != model_space {
                continue;
            }
            match entity {
                codec::EntityType::Line(line) => {
                    next_wire += 1;
                    wires.push(WireSeg::new(
                        next_wire,
                        Point::new(line.start.x, line.start.y),
                        Point::new(line.end.x, line.end.y),
                    ));
                }
                codec::EntityType::LwPolyline(polyline) => {
                    let points: Vec<Point> = polyline
                        .vertices
                        .iter()
                        .map(|vertex| Point::new(vertex.location.x, vertex.location.y))
                        .collect();
                    for pair in points.windows(2) {
                        next_wire += 1;
                        wires.push(WireSeg::new(next_wire, pair[0], pair[1]));
                    }
                }
                codec::EntityType::Insert(insert)
                        if !insert.attributes.is_empty() && !is_setup_block(&insert.block_name) =>
                    {
                    next_anchor += 1;
                    let label = insert
                        .attributes
                        .iter()
                        .find(|attribute| is_tag_attribute(&attribute.tag))
                        .map(|attribute| attribute.value.trim().to_string())
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| insert.block_name.clone());
                    anchors.push(Anchor::new(
                        next_anchor,
                        &label,
                        Point::new(insert.insert_point.x, insert.insert_point.y),
                    ));
                }
                _ => {}
            }
        }
        (wires, anchors)
    }

    /// `ELECTERMINALS` reports the terminal strips in the drawing.
    ///
    /// Terminals are components tagged `strip:index` (`X1:12`). Each strip is listed in
    /// terminal order with the devices its terminals reach through their nets.
    pub(super) fn elec_terminals(&mut self, i: usize) -> Task<Message> {
        use ::elec::geometry::netlist_from_geometry;
        use ::elec::terminal_strips;

        const TOLERANCE: f64 = 0.05;

        let (wires, anchors) = self.elec_collect(i);
        let netlist = netlist_from_geometry(&wires, &anchors, TOLERANCE);
        let label_of = |id: u64| {
            anchors
                .iter()
                .find(|anchor| anchor.id == id)
                .map(|anchor| anchor.label.clone())
                .unwrap_or_else(|| "?".to_string())
        };

        let mut terminals: Vec<(String, Vec<String>)> = Vec::new();
        let mut others: Vec<String> = Vec::new();
        for anchor in &anchors {
            let peers: Vec<String> = netlist
                .nets
                .iter()
                .find(|net| net.anchors.contains(&anchor.id))
                .map(|net| {
                    net.anchors
                        .iter()
                        .filter(|id| **id != anchor.id)
                        .map(|id| label_of(*id))
                        .collect()
                })
                .unwrap_or_default();
            if ::elec::parse_terminal_tag(&anchor.label).is_some() {
                terminals.push((anchor.label.clone(), peers));
            } else {
                others.push(anchor.label.clone());
            }
        }

        if terminals.is_empty() {
            self.command_line.push_output(
                "ELECTERMINALS: no terminals found (no component tagged strip:index).",
            );
            return Task::none();
        }

        let strips = terminal_strips(&terminals);
        let summary = format!(
            "ELECTERMINALS: {} strip(s), {} terminal(s).",
            strips.len(),
            terminals.len()
        );
        self.command_line.push_output(&summary);

        for strip in &strips {
            let header = format!("  {} ({} terminal(s))", strip.name, strip.terminals.len());
            self.command_line.push_output(&header);
            for terminal in &strip.terminals {
                let detail = if terminal.peers.is_empty() {
                    "unconnected".to_string()
                } else {
                    terminal.peers.join(", ")
                };
                let line = format!("    {}: {}", terminal.tag, detail);
                self.command_line.push_output(&line);
            }
        }

        if !others.is_empty() {
            let line = format!("  not terminals: {}", others.join(", "));
            self.command_line.push_output(&line);
        }

        Task::none()
    }

    /// `ELECXREF` reports devices whose parts are spread across the drawing.
    ///
    /// Every part of a device carries the device's tag, so grouping by tag recovers the
    /// device. The report lists multi-part devices first, with each part's class and
    /// position, which is the information a cross-reference table exists to carry.
    pub(super) fn elec_xref(&mut self, i: usize) -> Task<Message> {
        use ::elec::{cross_references, multi_part, PlacedPart};

        let mut parts: Vec<PlacedPart> = Vec::new();
        {
            let doc = &self.tabs[i].scene.document;
            let model_space = doc.header.model_space_block_handle;
            for entity in doc.entities() {
                if entity.common().owner_handle != model_space { continue; }
                let codec::EntityType::Insert(insert) = entity else {
                    continue;
                };
                if insert.attributes.is_empty() || is_setup_block(&insert.block_name) {
                    continue;
                }
                let attribute_value = |names: &[&str]| {
                    insert
                        .attributes
                        .iter()
                        .find(|attribute| {
                            let tag = attribute.tag.trim().to_ascii_uppercase();
                            names.iter().any(|name| tag == *name)
                        })
                        .map(|attribute| attribute.value.trim().to_string())
                        .unwrap_or_default()
                };
                let tag = attribute_value(&["TAG", "TAG1", "TAG2", "TAGSTR", "WORKID"]);
                let part_id = attribute_value(&["PIN", "CONTACT", "CONTACTNUMBER", "PARTNUMBER2"]);
                let mut placed = PlacedPart::new(
                    &insert.block_name,
                    &tag,
                    insert.insert_point.x,
                    insert.insert_point.y,
                );
                if !part_id.is_empty() {
                    placed = placed.with_part_id(&part_id);
                }
                parts.push(placed);
            }
        }

        if parts.is_empty() {
            self.command_line
                .push_output("ELECXREF: no electrical components found.");
            return Task::none();
        }

        let devices = cross_references(&parts);
        let multi = multi_part(&devices);
        let summary = format!(
            "ELECXREF: {} device(s); {} with more than one part.",
            devices.len(),
            multi.len()
        );
        self.command_line.push_output(&summary);

        if multi.is_empty() {
            self.command_line
                .push_output("  (no multi-part devices on this drawing)");
            return Task::none();
        }

        for device in multi {
            let contacts = device.contact_list();
            let header = if contacts.is_empty() {
                format!("  {} ({} part(s))", device.tag, device.parts.len())
            } else {
                format!(
                    "  {} ({} part(s)): {}",
                    device.tag,
                    device.parts.len(),
                    contacts
                )
            };
            self.command_line.push_output(&header);
            for part in &device.parts {
                let contact = part.part_id.clone().unwrap_or_else(|| "-".to_string());
                let line = format!(
                    "    {:<20} {:<8} x={:.2} y={:.2}",
                    part.class, contact, part.x, part.y
                );
                self.command_line.push_output(&line);
            }
        }

        let untagged = parts.iter().filter(|part| part.tag.trim().is_empty()).count();
        if untagged > 0 {
            let line = format!("  {untagged} untagged symbol(s) cannot be cross-referenced");
            self.command_line.push_output(&line);
        }

        Task::none()
    }

    /// `ELECPART <file.csv>` loads the parts catalogue for this session.
    ///
    /// The sheet is the one the report templates describe: `Part number`, `Manufacturer`,
    /// `Description`, with any extra columns ignored. Loading a second file replaces the
    /// first.
    pub(super) fn elec_parts(&mut self, path: &str) -> Task<Message> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                let message = format!("ELECPART: cannot read {path}: {error}");
                self.command_line.push_output(&message);
                return Task::none();
            }
        };
        match ::elec::Catalogue::from_csv(&text) {
            Ok(loaded) => {
                let count = loaded.len();
                if let Ok(mut guard) = catalogue().write() {
                    *guard = Some(loaded);
                }
                let message = format!("ELECPART: loaded {count} part(s) from {path}");
                self.command_line.push_output(&message);
            }
            Err(error) => {
                let message = format!("ELECPART: {path}: {error}");
                self.command_line.push_output(&message);
            }
        }
        Task::none()
    }

    /// Collect the drawing's placed parts (class, tag, contact id, position).
    fn elec_placed_parts(&self, i: usize) -> Vec<::elec::PlacedPart> {
        use ::elec::PlacedPart;
        let doc = &self.tabs[i].scene.document;
        let model_space = doc.header.model_space_block_handle;
        let mut parts = Vec::new();
        for entity in doc.entities() {
            if entity.common().owner_handle != model_space { continue; }
            let codec::EntityType::Insert(insert) = entity else {
                continue;
            };
            if insert.attributes.is_empty() || is_setup_block(&insert.block_name) {
                continue;
            }
            let attribute_value = |names: &[&str]| {
                insert
                    .attributes
                    .iter()
                    .find(|attribute| {
                        let tag = attribute.tag.trim().to_ascii_uppercase();
                        names.iter().any(|name| tag == *name)
                    })
                    .map(|attribute| attribute.value.trim().to_string())
                    .unwrap_or_default()
            };
            let tag = attribute_value(&["TAG", "TAG1", "TAG2", "TAGSTR", "WORKID"]);
            let part_id = attribute_value(&["PIN", "CONTACT", "CONTACTNUMBER", "PARTNUMBER2"]);
            let mut placed = PlacedPart::new(
                &insert.block_name,
                &tag,
                insert.insert_point.x,
                insert.insert_point.y,
            );
            if !part_id.is_empty() {
                placed = placed.with_part_id(&part_id);
            }
            parts.push(placed);
        }
        parts
    }

    /// `ELECXREFWRITE` annotates each part of a multi-part device with the references to
    /// its other parts, the way an ECAD prints `13-14` beside a coil.
    ///
    /// Text goes on layer `XREF`, offset from the symbol's insertion point. A label at the
    /// same spot is updated rather than duplicated, so the command can be re-run after the
    /// drawing changes.
    pub(super) fn elec_xref_write(&mut self, i: usize) -> Task<Message> {
        use ::elec::{cross_references, multi_part};

        const LABEL_LAYER: &str = "XREF";
        const LABEL_HEIGHT: f64 = 2.0;
        const OFFSET: f64 = 3.0;

        let parts = self.elec_placed_parts(i);
        if parts.is_empty() {
            self.command_line
                .push_output("ELECXREFWRITE: no electrical components found.");
            return Task::none();
        }

        let mut existing: Vec<(codec::Handle, String, f64, f64)> = Vec::new();
        {
            let doc = &self.tabs[i].scene.document;
            let model_space = doc.header.model_space_block_handle;
            for entity in doc.entities() {
                if entity.common().owner_handle != model_space { continue; }
                if let codec::EntityType::Text(text) = entity {
                    if text.common.layer.eq_ignore_ascii_case(LABEL_LAYER) {
                        existing.push((
                            text.common.handle,
                            text.value.clone(),
                            text.insertion_point.x,
                            text.insertion_point.y,
                        ));
                    }
                }
            }
        }

        let devices = cross_references(&parts);
        let multi = multi_part(&devices);

        let mut updates: Vec<(codec::Handle, String)> = Vec::new();
        let mut creates: Vec<(String, f64, f64)> = Vec::new();
        let mut unchanged = 0usize;

        for device in &multi {
            for (index, part) in device.parts.iter().enumerate() {
                let reference = device.reference_for(index);
                if reference.is_empty() {
                    continue;
                }
                let x = part.x + OFFSET;
                let y = part.y + OFFSET;
                let reuse = existing
                    .iter()
                    .find(|(_, _, ex, ey)| (ex - x).abs() <= 0.1 && (ey - y).abs() <= 0.1);
                match reuse {
                    Some((handle, current, _, _)) if current == &reference => unchanged += 1,
                    Some((handle, _, _, _)) => updates.push((*handle, reference)),
                    None => creates.push((reference, x, y)),
                }
            }
        }

        let summary = format!(
            "ELECXREFWRITE: {} device(s); {} label(s) created, {} updated, {} unchanged.",
            multi.len(),
            creates.len(),
            updates.len(),
            unchanged
        );
        self.command_line.push_output(&summary);

        if creates.is_empty() && updates.is_empty() {
            return Task::none();
        }
        self.push_undo_snapshot(i, "ELECXREFWRITE");
        self.tabs[i].scene.ensure_layer(LABEL_LAYER);
        for (handle, value) in updates {
            let doc = &mut self.tabs[i].scene.document;
            if let Some(codec::EntityType::Text(text)) = doc.get_entity_mut(handle) {
                text.value = value;
            }
        }
        for (value, x, y) in creates {
            let mut text = codec::entities::Text::new();
            text.value = value;
            text.insertion_point = codec::types::Vector3::new(x, y, 0.0);
            text.height = LABEL_HEIGHT;
            text.common.layer = LABEL_LAYER.to_string();
            self.tabs[i].scene.add_entity(codec::EntityType::Text(text));
        }
        self.tabs[i].dirty = true;
        Task::none()
    }

    /// `ELECREPORT <file.xlsx>` writes the drawing's reports into one workbook.
    ///
    /// Three sheets, matching the column names of the shipped templates: the bill of
    /// material (enriched from the parts catalogue when one is loaded), the wire list, and
    /// the terminal strips.
    pub(super) fn elec_report(&mut self, i: usize, path: &str) -> Task<Message> {
        use ::elec::geometry::netlist_from_geometry;
        use ::elec::{bom_sheet, terminal_sheet, wire_rows, wire_sheet_from};

        const TOLERANCE: f64 = 0.05;

        let mut components = self.elec_components(i);
        if let Ok(guard) = catalogue().read() {
            if let Some(catalogue) = guard.as_ref() {
                for component in &mut components {
                    catalogue.enrich(component);
                }
            }
        }
        let rows = bill_of_materials(&components);

        let (wires, anchors) = self.elec_collect(i);
        let netlist = netlist_from_geometry(&wires, &anchors, TOLERANCE);
        let wire_rows = wire_rows(&wires, &netlist);

        let label_of = |id: u64| {
            anchors
                .iter()
                .find(|anchor| anchor.id == id)
                .map(|anchor| anchor.label.clone())
                .unwrap_or_else(|| "?".to_string())
        };
        let mut terminals: Vec<(String, Vec<String>)> = Vec::new();
        for anchor in &anchors {
            if ::elec::parse_terminal_tag(&anchor.label).is_none() {
                continue;
            }
            let peers: Vec<String> = netlist
                .nets
                .iter()
                .find(|net| net.anchors.contains(&anchor.id))
                .map(|net| {
                    net.anchors
                        .iter()
                        .filter(|id| **id != anchor.id)
                        .map(|id| label_of(*id))
                        .collect()
                })
                .unwrap_or_default();
            terminals.push((anchor.label.clone(), peers));
        }
        let strips = ::elec::terminal_strips(&terminals);

        let sheets = vec![
            bom_sheet(&rows),
            wire_sheet_from(&wire_rows),
            terminal_sheet(&strips),
        ];

        match write_xlsx(path, &sheets) {
            Ok(()) => {
                let summary = format!(
                    "ELECREPORT: wrote {path} (BOM {} row(s), wire list {} row(s), terminals {} row(s)).",
                    sheets[0].body_len(),
                    sheets[1].body_len(),
                    sheets[2].body_len()
                );
                self.command_line.push_output(&summary);
            }
            Err(error) => {
                let message = format!("ELECREPORT: cannot write {path}: {error}");
                self.command_line.push_output(&message);
            }
        }
        Task::none()
    }

    /// `ELECINSERT <block> [part]` places another instance of a symbol already in the
    /// drawing.
    ///
    /// This is the cheapest honest way to get a new component on the sheet: the block comes
    /// from the drawing itself, so the geometry is whatever the project's symbols are. The
    /// new instance is tagged with the next free number for its class and, when a part
    /// number is given, priced from the parts catalogue.
    pub(super) fn elec_insert(
        &mut self,
        i: usize,
        block: &str,
        part: Option<&str>,
    ) -> Task<Message> {
        let mut template: Option<codec::entities::Insert> = None;
        let mut tagged: Vec<(String, String)> = Vec::new();
        let mut blocks_in_drawing: Vec<String> = Vec::new();
        let mut rightmost = f64::MIN;
        let mut lowest = f64::MAX;
        let mut placed = 0usize;
        {
            let doc = &self.tabs[i].scene.document;
            let model_space = doc.header.model_space_block_handle;
            for entity in doc.entities() {
                if entity.common().owner_handle != model_space { continue; }
                let codec::EntityType::Insert(insert) = entity else {
                    continue;
                };
                if is_setup_block(&insert.block_name) {
                    continue;
                }
                if !blocks_in_drawing
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(&insert.block_name))
                {
                    blocks_in_drawing.push(insert.block_name.clone());
                }
                // Placement clears *every* symbol, not just the same block: inserting a
                // motor next to a contactor must not drop it on top of the contactor.
                placed += 1;
                rightmost = rightmost.max(insert.insert_point.x);
                lowest = lowest.min(insert.insert_point.y);
                // Tags are collected per *class*, not per block: HCR1 and CONT-NO are both
                // contactors, so they share the -K sequence and must not both start at -K1.
                let tag = insert
                    .attributes
                    .iter()
                    .find(|attribute| is_tag_attribute(&attribute.tag))
                    .map(|attribute| attribute.value.trim().to_string())
                    .unwrap_or_default();
                tagged.push((insert.block_name.clone(), tag));
                if !insert.block_name.eq_ignore_ascii_case(block) {
                    continue;
                }
                if template.is_none() {
                    template = Some(insert.clone());
                }
            }
        }
        // A block's class comes from the library; without one, the block name is the class.
        let class_of = |block_name: &str| {
            library_index()
                .read()
                .ok()
                .and_then(|index| {
                    index
                        .get(&block_name.to_ascii_uppercase())
                        .map(|entry| entry.class.clone())
                })
                .unwrap_or_else(|| block_name.to_string())
        };

        let template = match template {
            Some(template) => Some(template),
            None => {
                // No instance in the drawing, but the block may exist (a symbol library
                // loads its geometry as block definitions).
                let block_exists = self.tabs[i]
                    .scene
                    .document
                    .block_records
                    .get(block)
                    .is_some();
                if !block_exists {
                    let available = if blocks_in_drawing.is_empty() {
                        "none".to_string()
                    } else {
                        blocks_in_drawing.join(", ")
                    };
                    let message = format!(
                        "ELECINSERT: no block named \"{block}\" in this drawing (available: {available}). Load a symbol library with ELECLIB <folder>."
                    );
                    self.command_line.push_output(&message);
                    return Task::none();
                }
                None
            }
        };

        // The library knows what class a symbol is, and the class decides the tag prefix:
        // a relay gets -K, a drive -U, a terminal -X.
        let class = class_of(block);
        // Numbering shares one sequence per *prefix*, not per class: a relay (HCR1) and a
        // contactor (CONT-NO) are both -K devices, so they must not both start at -K1.
        let prefix = ::elec::default_prefix(&class);
        let tags: Vec<String> = tagged
            .iter()
            .filter(|(block_name, _)| ::elec::default_prefix(&class_of(block_name)) == prefix)
            .map(|(_, tag)| tag.clone())
            .collect();
        let tag_refs: Vec<&str> = tags.iter().map(String::as_str).collect();
        let new_tag = ::elec::next_tag(&tag_refs, &class);
        // A drawing with no components of this block has no anchor to place beside, and
        // `f64::MIN` is finite, so the count - not the sentinel - decides.
        let new_position = if placed == 0 {
            codec::types::Vector3::ZERO
        } else {
            codec::types::Vector3::new(rightmost + 40.0, lowest, 0.0)
        };

        let record = part.and_then(|part| {
            catalogue()
                .read()
                .ok()
                .and_then(|guard| guard.as_ref().and_then(|cat| cat.lookup(part)).cloned())
        });

        // Either clone the instance already on the sheet, or build one from the block's
        // attribute definitions when the geometry exists but has never been placed here.
        let mut insert = match template {
            Some(template) => {
                let delta = codec::types::Vector3::new(
                    new_position.x - template.insert_point.x,
                    new_position.y - template.insert_point.y,
                    0.0,
                );
                let mut insert = template;
                for attribute in &mut insert.attributes {
                    attribute.insertion_point.x += delta.x;
                    attribute.insertion_point.y += delta.y;
                    attribute.alignment_point.x += delta.x;
                    attribute.alignment_point.y += delta.y;
                }
                insert.insert_point = new_position;
                insert
            }
            None => {
                let mut insert = codec::entities::Insert::new(block, new_position);
                let definitions = self.elec_block_definitions(i, block);
                if definitions.is_empty() {
                    // A library symbol may be plain geometry with no attribute definitions.
                    // Without attributes it would not be a component at all, so use the
                    // attribute set the library recorded for it - that is the set the
                    // project's own drawings carry (TAG1, CAT, MFG, DESC1, TERM01, XREF...).
                    let mut template: Vec<String> = library_index()
                        .read()
                        .ok()
                        .and_then(|index| {
                            index
                                .get(&block.to_ascii_uppercase())
                                .map(|entry| entry.attributes.clone())
                        })
                        .filter(|tags| !tags.is_empty())
                        .unwrap_or_else(|| {
                            vec![
                                "TAG1".to_string(),
                                "CAT".to_string(),
                                "MFG".to_string(),
                                "DESC1".to_string(),
                            ]
                        });
                    // A symbol whose recorded set has no tag attribute (a terminal block
                    // template that only lists X1TERM01...) would insert untagged and
                    // vanish from every report. Give it one.
                    if !template.iter().any(|tag| is_tag_attribute(tag)) {
                        template.insert(0, "TAG1".to_string());
                    }
                    let insert_height = library_index()
                        .read()
                        .ok()
                        .and_then(|index| {
                            index
                                .get(&block.to_ascii_uppercase())
                                .map(|entry| entry.label_height)
                        })
                        .filter(|height| *height > 0.0)
                        .unwrap_or(2.0);
                    let step = insert_height * 1.25;
                    for (row, tag) in template.iter().enumerate() {
                        let mut attribute =
                            codec::entities::AttributeEntity::new(tag.clone(), String::new());
                        attribute.height = insert_height;
                        attribute.insertion_point = codec::types::Vector3::new(
                            new_position.x,
                            new_position.y - insert_height - row as f64 * step,
                            0.0,
                        );
                        attribute.alignment_point = attribute.insertion_point;
                        insert.attributes.push(attribute);
                    }
                } else {
                    for definition in definitions {
                        // `None` takes the definition's own default value.
                        let mut attribute =
                            codec::entities::AttributeEntity::from_definition(&definition, None);
                        attribute.insertion_point.x += new_position.x;
                        attribute.insertion_point.y += new_position.y;
                        attribute.alignment_point.x += new_position.x;
                        attribute.alignment_point.y += new_position.y;
                        insert.attributes.push(attribute);
                    }
                }
                insert
            }
        };

        for attribute in &mut insert.attributes {
            let name = attribute.tag.trim().to_ascii_uppercase();
            if matches!(
                name.as_str(),
                "TAG" | "TAG1" | "TAG2" | "TAGSTR" | "WORKID"
            ) {
                attribute.value = new_tag.clone();
            } else if let Some(part) = part {
                if matches!(name.as_str(), "CAT" | "PART" | "PARTNUMBER" | "CATALOGUEID") {
                    attribute.value = part.to_string();
                }
            }
            if let Some(record) = record.as_ref() {
                if matches!(name.as_str(), "MFG" | "MANUFACTURER" | "MFR") {
                    if let Some(manufacturer) = &record.manufacturer {
                        attribute.value = manufacturer.clone();
                    }
                }
                if matches!(name.as_str(), "DESC" | "DESC1" | "DESC2" | "DESCRIPTION") {
                    if let Some(description) = &record.description {
                        attribute.value = description.clone();
                    }
                }
            }
        }

        self.push_undo_snapshot(i, "ELECINSERT");
        self.tabs[i]
            .scene
            .add_entity(codec::EntityType::Insert(insert));
        self.tabs[i].dirty = true;

        let detail = match part {
            Some(part) => format!(" with part {part}"),
            None => String::new(),
        };
        let message = format!(
            "ELECINSERT: placed {block} as {new_tag}{detail} at {:.2},{:.2}.",
            new_position.x, new_position.y
        );
        self.command_line.push_output(&message);
        Task::none()
    }

    /// Attribute definitions belonging to one block, as owned values.
    fn elec_block_definitions(
        &self,
        i: usize,
        block: &str,
    ) -> Vec<codec::entities::AttributeDefinition> {
        let doc = &self.tabs[i].scene.document;
        let Some(record) = doc.block_records.get(block) else {
            return Vec::new();
        };
        // Attribute definitions are owned by the block record, not by the BLOCK entity.
        let record_handle = record.handle;
        let entity_handles = record.entity_handles.clone();
        doc.entities()
            .filter_map(|entity| match entity {
                codec::EntityType::AttributeDefinition(definition)
                    if definition.common.owner_handle == record_handle
                        || entity_handles.contains(&definition.common.handle) =>
                {
                    Some(definition.clone())
                }
                _ => None,
            })
            .collect()
    }

    /// `ELECLIB <folder>` loads a symbol library: one DXF per symbol, the file name giving
    /// the block name.
    ///
    /// A library folder of plain drawings is the simplest thing that can work - no
    /// manifest, no metadata, just "CONTACTOR.dxf draws a contactor". Each drawing's model
    /// space becomes a block definition, so `ELECINSERT CONTACTOR` can then place it even
    /// on a drawing that has never contained one.
    pub(super) fn elec_lib(&mut self, i: usize, folder: &str) -> Task<Message> {
        let directory = std::path::Path::new(folder);
        if !directory.is_dir() {
            let message = format!("ELECLIB: {folder} is not a folder.");
            self.command_line.push_output(&message);
            return Task::none();
        }

        let mut paths: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("dxf"))
                {
                    paths.push(path);
                }
            }
        }
        paths.sort();

        let mut loaded = 0usize;
        let mut skipped = 0usize;
        let mut failed: Vec<String> = Vec::new();
        let manifest = read_library_manifest(directory);

        for path in paths {
            let Some(stem) = path
                .file_stem()
                .and_then(|name| name.to_str())
                .map(|name| name.to_string())
            else {
                continue;
            };
            if self.tabs[i].scene.document.block_records.get(&stem).is_some() {
                skipped += 1;
                continue;
            }
            match crate::io::load_file(&path) {
                Ok(document) => {
                    let mut entities: Vec<codec::EntityType> =
                        document.entities().cloned().collect();
                    if entities.is_empty() {
                        // A symbol file keeps its geometry inside the block definition
                        // rather than in model space, which is how a harvested library is
                        // written. Take the block's own entities, attribute definitions
                        // included.
                        let first = document
                            .block_records
                            .iter()
                            .map(|record| record.name.clone())
                            .find(|name| !name.starts_with('*'));
                        if let Some(block_name) = first {
                            if let Some(record) = document.block_records.get(&block_name) {
                                let handle = record.block_entity_handle;
                                entities = document
                                    .entities()
                                    .filter(|entity| entity.common().owner_handle == handle)
                                    .cloned()
                                    .collect();
                            }
                        }
                    }
                    if entities.is_empty() {
                        failed.push(format!("{stem}: no geometry"));
                        continue;
                    }
                    // Give the block attribute *definitions* from the library template, so
                    // the symbol is self-describing: the app's own INSERT can create the
                    // attributes, and `ATTDEF`/`ATTMAN` have something to show. Harvested
                    // symbols carry their attributes on the insert and have none of their
                    // own, which is exactly why they arrive mute.
                    let template = manifest
                        .get(&stem.to_ascii_uppercase())
                        .map(|entry| entry.attributes.clone())
                        .unwrap_or_default();
                    let already_defined = entities.iter().any(|entity| {
                        matches!(entity, codec::EntityType::AttributeDefinition(_))
                    });
                    if !already_defined && !template.is_empty() {
                        for (row, tag) in template.iter().enumerate() {
                            let mut definition = codec::entities::AttributeDefinition::new(
                                tag.clone(),
                                tag.clone(),
                                String::new(),
                            );
                            definition.height = 2.0;
                            definition.insertion_point = codec::types::Vector3::new(
                                0.0,
                                4.0 - row as f64 * 2.5,
                                0.0,
                            );
                            definition.alignment_point = definition.insertion_point;
                            entities.push(codec::EntityType::AttributeDefinition(definition));
                        }
                    }
                    match self.tabs[i].scene.define_block_from_owned_entities(
                        entities,
                        &stem,
                        glam::DVec3::ZERO,
                    ) {
                        Ok(_) => loaded += 1,
                        Err(error) => failed.push(format!("{stem}: {error}")),
                    }
                }
                Err(error) => failed.push(format!("{stem}: {error}")),
            }
        }

        if let Ok(mut guard) = library_dir().write() {
            *guard = Some(directory.to_path_buf());
        }
        remember_library(directory);
        let manifest_count = manifest.len();
        if let Ok(mut guard) = library_index().write() {
            *guard = manifest;
        }

        // Measure each symbol so its attribute text suits its own size: a drive and a
        // terminal should not get the same label height.
        if let Ok(mut guard) = library_index().write() {
            for entry in guard.values_mut() {
                let segments = self.elec_symbol_segments(i, &entry.name);
                let (mut min_x, mut min_y) = (f64::MAX, f64::MAX);
                let (mut max_x, mut max_y) = (f64::MIN, f64::MIN);
                for ((ax, ay), (bx, by)) in &segments {
                    min_x = min_x.min(*ax).min(*bx);
                    min_y = min_y.min(*ay).min(*by);
                    max_x = max_x.max(*ax).max(*bx);
                    max_y = max_y.max(*ay).max(*by);
                }
                let span = if min_x <= max_x {
                    (max_x - min_x).max(max_y - min_y)
                } else {
                    0.0
                };
                entry.label_height = if span > 0.0 {
                    // The project sheets set text at roughly one eighth of a symbol (their
                    // text is 0.125 tall against symbols under a unit). A "drawing style"
                    // floor of 0.8 was ~16x their text and dwarfed every small symbol.
                    (span / 8.0).clamp(0.01, 50.0)
                } else {
                    0.125
                };
            }
        }

        // How much geometry the palette can preview. Zero here means the thumbnails are
        // empty - worth knowing without opening the palette and squinting at it.
        let preview_segments: usize = self
            .elec_library_symbols()
            .iter()
            .map(|row| row.2.len())
            .sum();
        let summary = format!(
            "ELECLIB: loaded {loaded} symbol(s) from {folder}; {skipped} already in this drawing, {} failed, {manifest_count} in the catalogue, {preview_segments} preview segment(s).",
            failed.len()
        );
        self.command_line.push_output(&summary);
        for failure in &failed {
            let line = format!("  {failure}");
            self.command_line.push_output(&line);
        }
        Task::none()
    }

    /// `ELECTSIDES` reports what each terminal connects to on each of its two sides.
    ///
    /// A terminal is a bridge between two nets, not a node in one. The geometric netlist
    /// cannot say this - it only knows points - so this pass walks the wires *without*
    /// crossing the terminal and reports each side separately.
    pub(super) fn elec_sides(&mut self, i: usize) -> Task<Message> {
        use ::elec::terminal_sides;

        const TOLERANCE: f64 = 0.05;

        let (wires, anchors) = self.elec_collect(i);
        if anchors.is_empty() {
            self.command_line
                .push_output("ELECTSIDES: no components found.");
            return Task::none();
        }

        let terminals: Vec<u64> = anchors
            .iter()
            .filter(|anchor| ::elec::parse_terminal_tag(&anchor.label).is_some())
            .map(|anchor| anchor.id)
            .collect();
        if terminals.is_empty() {
            self.command_line
                .push_output("ELECTSIDES: no terminals found (no component tagged strip:index).");
            return Task::none();
        }

        let label_of = |id: u64| {
            anchors
                .iter()
                .find(|anchor| anchor.id == id)
                .map(|anchor| anchor.label.clone())
                .unwrap_or_else(|| "?".to_string())
        };

        let results = terminal_sides(&wires, &anchors, &terminals, TOLERANCE);
        let through = results.iter().filter(|entry| entry.is_through()).count();
        let single = results
            .iter()
            .filter(|entry| entry.sides.len() == 1)
            .count();
        let unconnected = results
            .iter()
            .filter(|entry| entry.is_unconnected())
            .count();

        let summary = format!(
            "ELECTSIDES: {} terminal(s); {through} through, {single} single-sided, {unconnected} unconnected.",
            results.len()
        );
        self.command_line.push_output(&summary);

        for entry in &results {
            if entry.is_unconnected() {
                let line = format!("  {}: unconnected", entry.label);
                self.command_line.push_output(&line);
                continue;
            }
            let mut parts: Vec<String> = Vec::new();
            for (index, side) in entry.sides.iter().enumerate() {
                let name = if index == 0 { "A" } else { "B" };
                let peers: Vec<String> = side.anchors.iter().map(|id| label_of(*id)).collect();
                let peer_text = if peers.is_empty() {
                    "(nothing)".to_string()
                } else {
                    peers.join(", ")
                };
                parts.push(format!("{name}: {peer_text} [{} wire(s)]", side.wires.len()));
            }
            if entry.sides.len() == 1 {
                parts.push("B: (nothing)".to_string());
            }
            let line = format!("  {}: {}", entry.label, parts.join(" | "));
            self.command_line.push_output(&line);
        }

        Task::none()
    }
    /// `ELECPLACE <block>` starts click-to-place for a library symbol.
    ///
    /// Everything the command needs - the attribute template, the next free tag, the
    /// catalogue values - is resolved here, and the palette modal is closed so the canvas
    /// takes the next click.
    pub(super) fn elec_place(&mut self, i: usize, block: &str) -> Task<Message> {
        use crate::modules::electrical::place::{PlaceSymbolCommand, PlacedAttribute};

        let class = library_index()
            .read()
            .ok()
            .and_then(|index| {
                index
                    .get(&block.to_ascii_uppercase())
                    .map(|entry| entry.class.clone())
            })
            .unwrap_or_else(|| block.to_string());

        // Tags already in this family, so the new instance joins the sequence.
        let prefix = ::elec::default_prefix(&class);
        let (_, anchors) = self.elec_collect(i);
        let tags: Vec<String> = anchors
            .iter()
            .filter(|anchor| anchor.label.starts_with(prefix.as_str()))
            .map(|anchor| anchor.label.clone())
            .collect();
        let tag_refs: Vec<&str> = tags.iter().map(String::as_str).collect();
        let new_tag = ::elec::next_tag(&tag_refs, &class);

        let mut template: Vec<String> = library_index()
            .read()
            .ok()
            .and_then(|index| {
                index
                    .get(&block.to_ascii_uppercase())
                    .map(|entry| entry.attributes.clone())
            })
            .filter(|attributes| !attributes.is_empty())
            .unwrap_or_else(|| vec!["TAG1".to_string(), "CAT".to_string()]);
        if !template.iter().any(|tag| is_tag_attribute(tag)) {
            template.insert(0, "TAG1".to_string());
        }

        let attributes: Vec<PlacedAttribute> = template
            .iter()
            .map(|tag| PlacedAttribute {
                tag: tag.clone(),
                value: if is_tag_attribute(tag) {
                    new_tag.clone()
                } else {
                    String::new()
                },
            })
            .collect();

        self.active_modal = None;
        // Label height comes from the symbol's own size, measured when the library loaded;
        // the scale is the one the source drawings insert it at.
        let (label_height, symbol_scale) = library_index()
            .read()
            .ok()
            .and_then(|index| {
                index
                    .get(&block.to_ascii_uppercase())
                    .map(|entry| (entry.label_height, entry.scale))
            })
            .unwrap_or((0.125, 1.0));
        let label_height = if label_height > 0.0 { label_height } else { 0.125 };
        let command = PlaceSymbolCommand::new(
            block.to_string(),
            attributes,
            label_height,
            symbol_scale,
        );
        let prompt = format!("ELECPLACE {block}  Specify insertion point:");
        self.tabs[i].active_cmd = Some(Box::new(command));
        self.command_line.push_output(&prompt);
        Task::none()
    }



    /// `ELECSETATTR <tag> <NAME=value>[,<NAME=value>...]` edits a placed component's
    /// attributes.
    ///
    /// Attributes are addressed by tag rather than by selection so the edit is scriptable
    /// and repeatable - the same reason the rest of this layer is command driven. All
    /// inserts carrying the tag change together, which is what you want when a device has
    /// several parts (a coil and its contacts share `-K1`).
    pub(super) fn elec_set_attr(&mut self, i: usize, tag: &str, assignments: &str) -> Task<Message> {
        let tag = tag.trim();
        if tag.is_empty() {
            self.command_line
                .push_output("ELECSETATTR: give a component tag, e.g. ELECSETATTR -K1 CAT=3RT2016-1AB01");
            return Task::none();
        }

        let mut pairs: Vec<(String, String)> = Vec::new();
        for part in assignments.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let Some((name, value)) = part.split_once('=') else {
                let message = format!("ELECSETATTR: \"{part}\" is not NAME=value.");
                self.command_line.push_output(&message);
                return Task::none();
            };
            let name = name.trim().to_ascii_uppercase();
            if name.is_empty() {
                let message = format!("ELECSETATTR: \"{part}\" has no attribute name.");
                self.command_line.push_output(&message);
                return Task::none();
            }
            pairs.push((name, value.trim().to_string()));
        }
        if pairs.is_empty() {
            self.command_line
                .push_output("ELECSETATTR: nothing to set (expected NAME=value).");
            return Task::none();
        }

        let targets: Vec<codec::Handle> = {
            let doc = &self.tabs[i].scene.document;
            doc.entities()
                .filter_map(|entity| match entity {
                    codec::EntityType::Insert(insert) => {
                        let matches = insert.attributes.iter().any(|attribute| {
                            is_tag_attribute(&attribute.tag)
                                && attribute.value.trim().eq_ignore_ascii_case(tag)
                        });
                        matches.then_some(insert.common.handle)
                    }
                    _ => None,
                })
                .collect()
        };

        if targets.is_empty() {
            let message = format!("ELECSETATTR: no component tagged {tag}.");
            self.command_line.push_output(&message);
            return Task::none();
        }

        self.push_undo_snapshot(i, "ELECSETATTR");
        let mut changed = 0usize;
        let mut added = 0usize;
        for handle in &targets {
            let doc = &mut self.tabs[i].scene.document;
            let Some(codec::EntityType::Insert(insert)) = doc.get_entity_mut(*handle) else {
                continue;
            };
            for (name, value) in &pairs {
                if let Some(attribute) = insert
                    .attributes
                    .iter_mut()
                    .find(|attribute| attribute.tag.trim().eq_ignore_ascii_case(name))
                {
                    if attribute.value != *value {
                        attribute.value = value.clone();
                        changed += 1;
                    }
                } else {
                    // The symbol did not carry this attribute; add it rather than silently
                    // dropping the edit, so a spare symbol can be completed by hand.
                    // Height follows the symbol: a fixed 2.0 dwarfs a sub-unit symbol, which
                    // is the bug visible in the drafted schematic.
                    let added_height = library_index()
                        .read()
                        .ok()
                        .and_then(|index| {
                            index
                                .get(&insert.block_name.to_ascii_uppercase())
                                .map(|entry| entry.label_height)
                        })
                        .filter(|height| *height > 0.0)
                        .unwrap_or(0.125);
                    let position = insert.insert_point;
                    let mut attribute = codec::entities::AttributeEntity::new(
                        name.clone(),
                        value.clone(),
                    );
                    attribute.height = added_height;
                    attribute.insertion_point = codec::types::Vector3::new(
                        position.x,
                        position.y
                            - added_height
                            - insert.attributes.len() as f64 * added_height * 1.25,
                        0.0,
                    );
                    attribute.alignment_point = attribute.insertion_point;
                    insert.attributes.push(attribute);
                    added += 1;
                }
            }
        }
        self.tabs[i].dirty = true;

        let listing: Vec<String> = pairs
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        let message = format!(
            "ELECSETATTR: {tag}: set {} on {} part(s); {changed} value(s) changed, {added} attribute(s) added.",
            listing.join(", "),
            targets.len()
        );
        self.command_line.push_output(&message);
        Task::none()
    }


    /// `ELECWIRE` starts the wire tool.
    pub(super) fn elec_wire(&mut self, i: usize) -> Task<Message> {
        use crate::modules::electrical::wire::WireCommand;

        let command = WireCommand::new();
        self.tabs[i].active_cmd = Some(Box::new(command));
        self.command_line
            .push_output("ELECWIRE  Specify start point:");
        Task::none()
    }

    /// `ELECJUNCTIONS` puts a dot wherever wires actually meet.
    ///
    /// Two wires touching end to end are one conductor; three or more, or a wire ending on
    /// another wire, must be drawn with a dot - on a schematic the dot is what tells an
    /// electrician the wires are connected rather than crossing. Crossing without a shared
    /// point gets nothing, because it is not a connection.
    pub(super) fn elec_junctions(&mut self, i: usize) -> Task<Message> {
        const TOLERANCE: f64 = 0.05;
        const DOT_LAYER: &str = "JUNCTIONS";
        const DOT_RADIUS: f64 = 1.0;

        let (wires, _) = self.elec_collect(i);
        if wires.is_empty() {
            self.command_line
                .push_output("ELECJUNCTIONS: no wires found.");
            return Task::none();
        }

        let points = ::elec::junction_points(&wires, TOLERANCE);
        if points.is_empty() {
            let message = format!(
                "ELECJUNCTIONS: {} wire segment(s); no point where three or more meet, nothing to draw.",
                wires.len()
            );
            self.command_line.push_output(&message);
            return Task::none();
        }

        // Existing dots, so a second run does not stack another circle on the same point.
        let existing: Vec<(f64, f64)> = {
            let doc = &self.tabs[i].scene.document;
            doc.entities()
                .filter_map(|entity| match entity {
                    codec::EntityType::Circle(circle)
                        if circle.common.layer.eq_ignore_ascii_case(DOT_LAYER) =>
                    {
                        Some((circle.center.x, circle.center.y))
                    }
                    _ => None,
                })
                .collect()
        };

        let mut created = 0usize;
        let mut already = 0usize;
        let mut pending: Vec<(f64, f64)> = Vec::new();
        for point in &points {
            if existing
                .iter()
                .any(|(x, y)| (x - point.x).abs() <= TOLERANCE && (y - point.y).abs() <= TOLERANCE)
            {
                already += 1;
                continue;
            }
            pending.push((point.x, point.y));
        }

        if pending.is_empty() {
            let message = format!(
                "ELECJUNCTIONS: {} junction(s), all already drawn.",
                points.len()
            );
            self.command_line.push_output(&message);
            return Task::none();
        }

        self.push_undo_snapshot(i, "ELECJUNCTIONS");
        self.tabs[i].scene.ensure_layer(DOT_LAYER);
        for (x, y) in &pending {
            let mut circle = codec::entities::Circle::new();
            circle.center = codec::types::Vector3::new(*x, *y, 0.0);
            circle.radius = DOT_RADIUS;
            circle.common.layer = DOT_LAYER.to_string();
            self.tabs[i]
                .scene
                .add_entity(codec::EntityType::Circle(circle));
            created += 1;
        }
        self.tabs[i].dirty = true;

        let message = format!(
            "ELECJUNCTIONS: {} wire segment(s); {created} junction(s) drawn, {already} already there.",
            wires.len()
        );
        self.command_line.push_output(&message);
        Task::none()
    }


    /// Bounding box of a set of entities, so a new symbol can be based at its own corner
    /// and labelled at a sensible size.
    fn elec_entity_bounds(entities: &[codec::EntityType]) -> Option<(f64, f64, f64, f64)> {
        let mut min_x = f64::MAX;
        let mut min_y = f64::MAX;
        let mut max_x = f64::MIN;
        let mut max_y = f64::MIN;
        let mut take = |x: f64, y: f64| {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        };
        for entity in entities {
            match entity {
                codec::EntityType::Line(line) => {
                    take(line.start.x, line.start.y);
                    take(line.end.x, line.end.y);
                }
                codec::EntityType::Circle(circle) => {
                    let r = circle.radius.abs();
                    take(circle.center.x - r, circle.center.y - r);
                    take(circle.center.x + r, circle.center.y + r);
                }
                codec::EntityType::Arc(arc) => {
                    let r = arc.radius.abs();
                    take(arc.center.x - r, arc.center.y - r);
                    take(arc.center.x + r, arc.center.y + r);
                }
                codec::EntityType::Text(text) => {
                    take(text.insertion_point.x, text.insertion_point.y);
                }
                codec::EntityType::LwPolyline(polyline) => {
                    for vertex in &polyline.vertices {
                        take(vertex.location.x, vertex.location.y);
                    }
                }
                codec::EntityType::Insert(insert) => {
                    take(insert.insert_point.x, insert.insert_point.y);
                }
                _ => {}
            }
        }
        if min_x > max_x || min_y > max_y {
            None
        } else {
            Some((min_x, min_y, max_x, max_y))
        }
    }

    /// `ELECMAKESYMBOL <name>` turns the selected geometry into a reusable symbol.
    ///
    /// The block is defined in this drawing and written to the loaded library as its own
    /// DXF, so it can be inserted into any drawing afterwards - that is what makes it a
    /// module rather than a one-off. The geometry is not consumed: the originals stay where
    /// they were, because deleting a user's work on a naming typo is not recoverable in a
    /// way they would thank you for.
    pub(super) fn elec_make_symbol(&mut self, i: usize, name: &str) -> Task<Message> {
        const DEFAULT_HEIGHT: f64 = 2.0;

        let name = name.trim();
        if name.is_empty() {
            self.command_line
                .push_output("ELECMAKESYMBOL: give the symbol a name.");
            return Task::none();
        }

        let handles: Vec<codec::Handle> = self.tabs[i]
            .scene
            .selected_entities()
            .into_iter()
            .map(|(handle, _)| handle)
            .collect();
        if handles.is_empty() {
            let message = format!(
                "ELECMAKESYMBOL: select the geometry for \"{name}\" first, then run the command."
            );
            self.command_line.push_output(&message);
            return Task::none();
        }
        if self.tabs[i].scene.document.block_records.get(name).is_some() {
            let message = format!("ELECMAKESYMBOL: a block named \"{name}\" already exists.");
            self.command_line.push_output(&message);
            return Task::none();
        }

        let entities: Vec<codec::EntityType> = {
            let doc = &self.tabs[i].scene.document;
            doc.entities()
                .filter(|entity| handles.contains(&entity.common().handle))
                .cloned()
                .collect()
        };
        let Some((min_x, min_y, max_x, max_y)) = Self::elec_entity_bounds(&entities) else {
            self.command_line.push_output(
                "ELECMAKESYMBOL: the selection has no measurable geometry.",
            );
            return Task::none();
        };

        // Add the attribute definitions the electrical layer needs, sized to the symbol so
        // the tag does not dwarf the drawing.
        let span = (max_x - min_x).max(max_y - min_y);
        let height = (span / 8.0).clamp(1.0, 4.0).min(if span > 0.0 { span } else { 1.0 });
        let mut with_attributes = entities.clone();
        for (row, (tag, prompt)) in [
            ("TAG1", "Tag"),
            ("CAT", "Part number"),
            ("MFG", "Manufacturer"),
            ("DESC1", "Description"),
        ]
        .iter()
        .enumerate()
        {
            let mut definition = codec::entities::AttributeDefinition::new(
                (*tag).to_string(),
                (*prompt).to_string(),
                String::new(),
            );
            definition.height = height;
            definition.insertion_point = codec::types::Vector3::new(
                min_x,
                min_y - height * 1.6 * (row as f64 + 1.0),
                0.0,
            );
            definition.alignment_point = definition.insertion_point;
            with_attributes.push(codec::EntityType::AttributeDefinition(definition));
        }
        let _ = DEFAULT_HEIGHT;

        match self.tabs[i].scene.define_block_from_owned_entities(
            with_attributes.clone(),
            name,
            glam::DVec3::new(min_x, min_y, 0.0),
        ) {
            Ok(_) => {}
            Err(error) => {
                let message = format!("ELECMAKESYMBOL: cannot define \"{name}\": {error}");
                self.command_line.push_output(&message);
                return Task::none();
            }
        }
        self.tabs[i].dirty = true;

        // Write the symbol into the library so it outlives this drawing.
        let folder = library_dir()
            .read()
            .ok()
            .and_then(|guard| guard.clone())
            .or_else(remembered_library);
        let mut written: Option<String> = None;
        if let Some(folder) = folder {
            let safe: String = name
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' { c } else { '_' })
                .collect();
            let path = folder.join(format!("{safe}.dxf"));
            let mut document = codec::Document::new();
            let translate = crate::command::EntityTransform::Translate(glam::DVec3::new(-min_x, -min_y, 0.0));
            let mut ok = true;
            for entity in &with_attributes {
                let mut copy = entity.clone();
                crate::scene::view::dispatch::apply_transform(&mut copy, &translate);
                if document.add_entity(copy).is_err() {
                    ok = false;
                    break;
                }
            }
            if ok && crate::io::save(&document, &path).is_ok() {
                let tags = "TAG1|CAT|MFG|DESC1";
                let manifest = folder.join("library.tsv");
                let mut text = std::fs::read_to_string(&manifest).unwrap_or_else(|_| {
                    "symbol\tclass\tattributes\tsheets\n".to_string()
                });
                if !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&format!("{safe}\tSymbol\t{tags}\t1\n"));
                let _ = std::fs::write(&manifest, text);
                written = Some(path.display().to_string());
            }
        }

        let message = match written {
            Some(path) => format!(
                "ELECMAKESYMBOL: \"{name}\" defined from {} object(s) and written to {path}.",
                entities.len()
            ),
            None => format!(
                "ELECMAKESYMBOL: \"{name}\" defined in this drawing from {} object(s); no library folder loaded, so it was not saved for reuse.",
                entities.len()
            ),
        };
        self.command_line.push_output(&message);
        Task::none()
    }


    /// The loaded symbols with the segments the palette draws, grouped by class then name.
    ///
    /// The geometry comes from the block definitions that `ELECLIB` created, so the
    /// thumbnail is the real symbol rather than a placeholder.
    /// Segments of one symbol's block geometry, for the palette thumbnail.
    ///
    /// Returns empty when the block is missing or its entities are not owned by the block
    /// record - the two reasons a thumbnail can silently draw nothing, which is why the
    /// palette shows the count next to the class while this is being finished.
    fn elec_symbol_segments(&self, i: usize, name: &str) -> Vec<((f64, f64), (f64, f64))> {
        const MAX_SEGMENTS: usize = 400;

        let doc = &self.tabs[i].scene.document;
        let Some(record) = doc.block_records.get(name) else {
            return Vec::new();
        };
        // Block contents are owned by the block **record** (or listed in its
        // entity_handles); `block_entity_handle` is the BLOCK entity itself. Asking the
        // wrong one yields an empty symbol that still looks fine in the palette.
        let record_handle = record.handle;
        let entity_handles = record.entity_handles.clone();
        let mut segments: Vec<((f64, f64), (f64, f64))> = Vec::new();
        for entity in doc.entities() {
            let owned = entity.common().owner_handle == record_handle
                || entity_handles.contains(&entity.common().handle);
            if !owned || segments.len() >= MAX_SEGMENTS {
                continue;
            }
            match entity {
                codec::EntityType::Line(line) => segments.push((
                    (line.start.x, line.start.y),
                    (line.end.x, line.end.y),
                )),
                codec::EntityType::Circle(circle) => {
                    let r = circle.radius.abs();
                    let steps = 16;
                    let mut previous = None;
                    for step in 0..=steps {
                        let angle = std::f64::consts::TAU * (step as f64) / (steps as f64);
                        let point = (
                            circle.center.x + r * angle.cos(),
                            circle.center.y + r * angle.sin(),
                        );
                        if let Some(from) = previous {
                            segments.push((from, point));
                        }
                        previous = Some(point);
                    }
                }
                codec::EntityType::Arc(arc) => {
                    let r = arc.radius.abs();
                    let steps = 8;
                    let sweep = arc.end_angle - arc.start_angle;
                    let mut previous = None;
                    for step in 0..=steps {
                        let angle = arc.start_angle + sweep * (step as f64) / (steps as f64);
                        let point = (
                            arc.center.x + r * angle.cos(),
                            arc.center.y + r * angle.sin(),
                        );
                        if let Some(from) = previous {
                            segments.push((from, point));
                        }
                        previous = Some(point);
                    }
                }
                codec::EntityType::LwPolyline(polyline) => {
                    let mut previous: Option<(f64, f64)> = None;
                    for vertex in &polyline.vertices {
                        let point = (vertex.location.x, vertex.location.y);
                        if let Some(from) = previous {
                            segments.push((from, point));
                        }
                        previous = Some(point);
                    }
                }
                _ => {}
            }
        }
        segments
    }



    /// The loaded symbols as `(name, class)`, grouped by class then name, for the palette.

    /// The loaded symbols with the segments the palette draws, grouped by class then name.
    pub(crate) fn elec_library_symbols(
        &self,
    ) -> Vec<(String, String, Vec<((f64, f64), (f64, f64))>)> {
        let Ok(index) = library_index().read() else {
            return Vec::new();
        };
        let i = self.active_tab;
        let mut rows: Vec<(String, String, Vec<((f64, f64), (f64, f64))>)> = index
            .values()
            .map(|entry| {
                (
                    entry.name.clone(),
                    entry.class.clone(),
                    self.elec_symbol_segments(i, &entry.name),
                )
            })
            .collect();
        rows.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        rows
    }


    /// `ELECNEW` prepares the drawing for electrical drafting.
    ///
    /// A new drawing arrives as bare model space: no wire layers, no wire-number layer, no
    /// symbol library. Doing that by hand is four commands and a path, and forgetting one
    /// leaves wires on layer 0 where nothing downstream looks for them.
    /// `ELECNEW [template.dxf]` prepares the drawing, optionally from a sheet template.
    ///
    /// The project's own sheets are the authority on frames, title blocks and text sizes -
    /// their content sits at a different scale from their border, so an invented frame
    /// would fight the drawings. Given a template, its layers and model-space geometry are
    /// copied in first, then the electrical setup runs as usual.
    pub(super) fn elec_new(&mut self, i: usize, template: Option<&str>) -> Task<Message> {
        let mut notes: Vec<String> = Vec::new();

        if let Some(path) = template {
            match crate::io::load_file(std::path::Path::new(path)) {
                Ok(document) => {
                    // Bring the template's layers across first: its geometry references them.
                    let mut layer_names: Vec<String> = Vec::new();
                    for record in document.layers.iter() {
                        let name = record.name.clone();
                        if self.tabs[i].scene.document.layers.get(&name).is_none() {
                            let mut layer = record.clone();
                            layer.name = name.clone();
                            let _ = self.tabs[i].scene.document.layers.add(layer);
                        }
                        layer_names.push(name);
                    }

                    let template_model_space = document.header.model_space_block_handle;
                    let mut copied = 0usize;
                    for entity in document.entities() {
                        if entity.common().owner_handle != template_model_space {
                            continue;
                        }
                        let mut copy = entity.clone();
                        copy.common_mut().handle = codec::Handle::NULL;
                        if self.tabs[i]
                            .scene
                            .document
                            .add_entity(copy)
                            .is_ok()
                        {
                            copied += 1;
                        }
                    }
                    notes.push(format!(
                        "{} object(s) and {} layer(s) from {path}",
                        copied,
                        layer_names.len()
                    ));
                }
                Err(error) => {
                    let message = format!("ELECNEW: cannot read template {path}: {error}");
                    self.command_line.push_output(&message);
                    return Task::none();
                }
            }
        }

        // Layers the electrical commands write to. An existing layer keeps the colour the
        // project gave it; only missing ones are created.
        let wanted: [(&str, i16); 5] = [
            ("WIRES", 3),
            ("JUNCTIONS", 3),
            ("WIRENO", 2),
            ("XREF", 6),
            ("ELEC-SYMBOLS", 7),
        ];
        let mut created: Vec<&str> = Vec::new();
        for (name, color) in wanted {
            let existed = self.tabs[i].scene.document.layers.get(name).is_some();
            self.tabs[i].scene.ensure_layer(name);
            if !existed {
                if let Some(layer) = self.tabs[i].scene.document.layers.get_mut(name) {
                    layer.color = codec::types::Color::from_index(color);
                }
                created.push(name);
            }
        }
        notes.push(format!(
            "{} electrical layer(s) created ({})",
            created.len(),
            if created.is_empty() {
                "none needed".to_string()
            } else {
                created.join(", ")
            }
        ));

        // The library is what makes the sheet draftable.
        let loaded = library_index()
            .read()
            .map(|index| !index.is_empty())
            .unwrap_or(false);
        if !loaded {
            if let Some(directory) = remembered_library() {
                let path = directory.display().to_string();
                let _ = self.elec_lib(i, &path);
                notes.push(format!("library loaded from {path}"));
            } else {
                notes.push("no library remembered - run ELECLIB <folder> once".to_string());
            }
        } else {
            notes.push("library already loaded".to_string());
        }

        self.tabs[i].dirty = true;
        let summary = format!("ELECNEW: {}.", notes.join("; "));
        self.command_line.push_output(&summary);
        self.command_line.push_output(
            "  Next: ELECPALETTE to pick a symbol, ELECWIRE to draw conductors, then ELECNUMBER / ELECBOM.",
        );
        Task::none()
    }





    /// `ELECPROJECT <folder>` reads every drawing in a folder and reports the project,
    /// not just the open sheet.
    ///
    /// Both products people compare this to are project-based: a device appears on several
    /// sheets, the bill of material sums the whole set, and a cross reference is only useful
    /// if it knows which sheet the other part is on. This is the smallest version of that -
    /// scan, aggregate, report - and it runs on the real project exports rather than a
    /// fixture.
    pub(super) fn elec_project(&mut self, folder: Option<&str>, out_path: Option<&str>) -> Task<Message> {
        // With no folder argument the open project file decides which sheets are read.
        let mut sheets: Vec<ProjectSheet> = Vec::new();
        if let Some(folder) = folder {
            let directory = std::path::Path::new(folder);
            if !directory.is_dir() {
                let message = format!("ELECPROJECT: {folder} is not a folder.");
                self.command_line.push_output(&message);
                return Task::none();
            }
            let mut paths: Vec<std::path::PathBuf> = Vec::new();
            if let Ok(entries) = std::fs::read_dir(directory) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("dxf"))
                    {
                        paths.push(path);
                    }
                }
            }
            paths.sort();
            sheets = paths
                .into_iter()
                .map(|path| ProjectSheet {
                    path,
                    number: String::new(),
                    title: String::new(),
                })
                .collect();
            if let Ok(mut guard) = project_dir().write() {
                *guard = Some(directory.to_path_buf());
            }
        } else if let Some(file) = project_file().read().ok().and_then(|guard| guard.clone()) {
            match Self::read_project(&file.display().to_string()) {
                Ok(list) => sheets = list,
                Err(error) => {
                    let message = format!("ELECPROJECT: {error}");
                    self.command_line.push_output(&message);
                    return Task::none();
                }
            }
        } else {
            self.command_line.push_output(
                "ELECPROJECT: no project open (ELECPROJNEW/ELECPROJOPEN) and no folder given.",
            );
            return Task::none();
        }

        if sheets.is_empty() {
            let message = format!("ELECPROJECT: no .dxf drawings in {}." , folder.unwrap_or("the project"));
            self.command_line.push_output(&message);
            return Task::none();
        }

        // tag -> sheets it appears on; part -> (quantity, sheets)
        let mut device_sheets: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut classes: BTreeMap<String, usize> = BTreeMap::new();
        let mut parts: BTreeMap<String, (usize, BTreeSet<String>)> = BTreeMap::new();
        let mut scanned = 0usize;
        let mut failed = 0usize;
        let mut components_total = 0usize;
        // Sheet -> how many components it contributed, for the workbook's sheet summary.
        let mut per_sheet: BTreeMap<String, usize> = BTreeMap::new();

        for entry_sheet in &sheets {
            let Ok(document) = crate::io::load_file(&entry_sheet.path) else {
                failed += 1;
                continue;
            };
            scanned += 1;
            // The project's own number and title identify the sheet; the file name is the
            // fallback when the project file does not supply them.
            let sheet = if entry_sheet.number.is_empty() {
                entry_sheet
                    .path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or("?")
                    .to_string()
            } else if entry_sheet.title.is_empty() {
                entry_sheet.number.clone()
            } else {
                format!("{} {}", entry_sheet.number, entry_sheet.title)
            };
            let mut on_this_sheet = 0usize;
            for component in Self::components_of(&document) {
                components_total += 1;
                on_this_sheet += 1;
                *classes.entry(component.class.clone()).or_default() += 1;
                let tag = component.tag.trim().to_string();
                if !tag.is_empty() {
                    device_sheets
                        .entry(tag.to_ascii_uppercase())
                        .or_default()
                        .push(sheet.clone());
                }
                if let Some(part) = component.part.clone() {
                    let entry = parts.entry(part).or_default();
                    entry.0 += 1;
                    entry.1.insert(sheet.clone());
                }
            }
            per_sheet.insert(sheet, on_this_sheet);
        }

        let multi_sheet: Vec<(&String, &Vec<String>)> = device_sheets
            .iter()
            .filter(|(_, list)| {
                let unique: BTreeSet<&String> = list.iter().collect();
                unique.len() > 1
            })
            .collect();

        let summary = format!(
            "ELECPROJECT: {scanned} sheet(s) read ({failed} failed); {components_total} component(s) in {} class(es); {} device tag(s), {} used on more than one sheet; {} distinct part(s).",
            classes.len(),
            device_sheets.len(),
            multi_sheet.len(),
            parts.len()
        );
        self.command_line.push_output(&summary);

        if !multi_sheet.is_empty() {
            self.command_line
                .push_output("  Devices across sheets:");
            for (tag, sheets) in multi_sheet.iter().take(15) {
                let unique: BTreeSet<&String> = sheets.iter().collect();
                let listing: Vec<String> = unique.iter().map(|name| (*name).clone()).collect();
                let line = format!("    {tag}: {}", listing.join(", "));
                self.command_line.push_output(&line);
            }
            if multi_sheet.len() > 15 {
                let line = format!("    ... and {} more", multi_sheet.len() - 15);
                self.command_line.push_output(&line);
            }
        }

        if !parts.is_empty() {
            self.command_line.push_output("  Parts across the project:");
            let mut rows: Vec<(&String, &(usize, BTreeSet<String>))> = parts.iter().collect();
            rows.sort_by_key(|(_, (quantity, _))| std::cmp::Reverse(*quantity));
            for (part, (quantity, sheets)) in rows.iter().take(15) {
                let line = format!("    {part}: {quantity} on {} sheet(s)", sheets.len());
                self.command_line.push_output(&line);
            }
        }

        if let Some(path) = out_path {
            match Self::write_project_workbook(path, &parts, &classes, &per_sheet, scanned) {
                Ok(()) => {
                    let message = format!(
                        "  wrote {path}: {} part row(s), {} sheet row(s).",
                        parts.len(),
                        per_sheet.len()
                    );
                    self.command_line.push_output(&message);
                }
                Err(error) => {
                    let message = format!("  cannot write {path}: {error}");
                    self.command_line.push_output(&message);
                }
            }
        }

        Task::none()
    }

    /// Writes the project-wide bill of material and per-sheet summary as a workbook.
    fn write_project_workbook(
        path: &str,
        parts: &BTreeMap<String, (usize, BTreeSet<String>)>,
        classes: &BTreeMap<String, usize>,
        per_sheet: &BTreeMap<String, usize>,
        sheets_read: usize,
    ) -> Result<(), String> {
        use ::elec::{Cell, Sheet};

        let mut bom = Sheet::new(
            "Project BOM",
            &["Part number", "Qty", "Sheets", "Where used"],
        );
        let mut rows: Vec<(&String, &(usize, BTreeSet<String>))> = parts.iter().collect();
        rows.sort_by_key(|(_, (quantity, _))| std::cmp::Reverse(*quantity));
        for (part, (quantity, sheets)) in rows {
            bom.push(vec![
                Cell::text(part.clone()),
                Cell::number(*quantity as f64),
                Cell::number(sheets.len() as f64),
                Cell::text(sheets.iter().cloned().collect::<Vec<_>>().join(", ")),
            ]);
        }

        let mut summary = Sheet::new("Sheets", &["Sheet", "Components"]);
        for (sheet, count) in per_sheet {
            summary.push(vec![Cell::text(sheet.clone()), Cell::number(*count as f64)]);
        }
        summary.push(vec![
            Cell::text("(sheets read)"),
            Cell::number(sheets_read as f64),
        ]);

        let mut by_class = Sheet::new("Classes", &["Class", "Components"]);
        let mut class_rows: Vec<(&String, &usize)> = classes.iter().collect();
        class_rows.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
        for (class, count) in class_rows {
            by_class.push(vec![Cell::text(class.clone()), Cell::number(*count as f64)]);
        }

        write_xlsx(path, &[bom, summary, by_class])
    }


    /// `ELECPLC <tag> <points> <base-address> [x,y]` places a PLC I/O module.
    ///
    /// Both products this is measured against treat a PLC module as one symbol carrying an
    /// address per point, so the module can be referenced as a unit while each I/O point
    /// still has its own address. This builds the module geometry on demand (a body with a
    /// pin per point), tags it, and writes `ADDR01..ADDRnn` from the base address.
    pub(super) fn elec_plc(
        &mut self,
        i: usize,
        tag: &str,
        points: usize,
        base: &str,
        at: Option<(f64, f64)>,
    ) -> Task<Message> {
        let tag = tag.trim();
        if tag.is_empty() || points == 0 {
            self.command_line.push_output(
                "ELECPLC: use ELECPLC <tag> <points> <base-address> [x,y], e.g. ELECPLC -P1 8 I0.0",
            );
            return Task::none();
        }
        if points > 128 {
            self.command_line
                .push_output("ELECPLC: at most 128 points per module.");
            return Task::none();
        }
        let Some(addresses) = Self::plc_addresses(base, points) else {
            let message = format!(
                "ELECPLC: \"{base}\" is not an address like I0.0 or O1.3."
            );
            self.command_line.push_output(&message);
            return Task::none();
        };

        let block_name = format!("PLC-{points}");
        let (x, y) = at.unwrap_or((0.0, 0.0));

        // One body with a pin per point, built once and reused for the same point count.
        if self.tabs[i].scene.document.block_records.get(&block_name).is_none() {
            let spacing = 0.25_f64;
            let width = 0.8_f64;
            let mut entities: Vec<codec::EntityType> = Vec::new();
            let mut line = |ax: f64, ay: f64, bx: f64, by: f64| {
                let mut entity = codec::entities::Line::from_points(
                    codec::types::Vector3::new(ax, ay, 0.0),
                    codec::types::Vector3::new(bx, by, 0.0),
                );
                entity.common.layer = "ELEC-SYMBOLS".to_string();
                entities.push(codec::EntityType::Line(entity));
            };
            let top = points as f64 * spacing;
            line(0.0, 0.0, width, 0.0);
            line(width, 0.0, width, top);
            line(width, top, 0.0, top);
            line(0.0, top, 0.0, 0.0);
            for point in 0..points {
                let y = (point as f64 + 0.5) * spacing;
                line(-0.15, y, 0.0, y);
                line(width, y, width + 0.15, y);
            }
            if let Err(error) = self.tabs[i].scene.define_block_from_owned_entities(
                entities,
                &block_name,
                glam::DVec3::ZERO,
            ) {
                let message = format!("ELECPLC: cannot build {block_name}: {error}");
                self.command_line.push_output(&message);
                return Task::none();
            }
        }

        let spacing = 0.25_f64;
        let mut insert = codec::entities::Insert::new(
            block_name.clone(),
            codec::types::Vector3::new(x, y, 0.0),
        );
        let mut push = |insert: &mut codec::entities::Insert, name: &str, value: &str, row: usize| {
            let mut attribute =
                codec::entities::AttributeEntity::new(name.to_string(), value.to_string());
            attribute.height = 0.1;
            attribute.insertion_point = codec::types::Vector3::new(
                x + 0.9,
                y + (row as f64 + 1.0) * spacing,
                0.0,
            );
            attribute.alignment_point = attribute.insertion_point;
            insert.attributes.push(attribute);
        };
        push(&mut insert, "TAG1", tag, 0);
        push(&mut insert, "DESC1", "PLC module", 1);
        for (point, address) in addresses.iter().enumerate() {
            push(&mut insert, &format!("ADDR{:02}", point + 1), address, point + 2);
            // The pin's own coordinates, so a later pass can follow the wire from the point
            // instead of guessing where the generated geometry put it.
            let pin_y = y + (point as f64 + 0.5) * spacing;
            push(
                &mut insert,
                &format!("X1TERM{:02}", point + 1),
                &format!("{:.4},{:.4}", x - 0.15, pin_y),
                point + 2,
            );
        }

        self.push_undo_snapshot(i, "ELECPLC");
        self.tabs[i]
            .scene
            .add_entity(codec::EntityType::Insert(insert));
        self.tabs[i].dirty = true;

        let summary = format!(
            "ELECPLC: {tag} with {points} point(s), {}..{} at {x:.2},{y:.2}.",
            addresses.first().cloned().unwrap_or_default(),
            addresses.last().cloned().unwrap_or_default()
        );
        self.command_line.push_output(&summary);
        Task::none()
    }

    /// Addresses for `points` consecutive I/O points from a base like `I0.0`.
    ///
    /// The convention both products use groups eight points per byte: `I0.0`-`I0.7`, then
    /// `I1.0`. Returns `None` when the base does not look like an address.
    fn plc_addresses(base: &str, points: usize) -> Option<Vec<String>> {
        let base = base.trim();
        let split = base.rfind('.')?;
        let (head, last) = base.split_at(split);
        let last = &last[1..];
        if last.is_empty() || !last.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let bit_start: usize = last.parse().ok()?;
        if bit_start > 7 {
            return None;
        }
        // `I0` or `I0.` prefix stays; only the byte number changes on overflow.
        let byte_start: usize = head
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>()
            .parse()
            .ok()?;
        let byte_prefix_len = head.len()
            - head
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .count();
        let prefix = &head[..byte_prefix_len];

        let mut out = Vec::with_capacity(points);
        let mut bit = bit_start;
        let mut byte = byte_start;
        for _ in 0..points {
            out.push(format!("{prefix}{byte}.{bit}"));
            bit += 1;
            if bit > 7 {
                bit = 0;
                byte += 1;
            }
        }
        Some(out)
    }

    /// `ELECIO` lists each PLC point with its address and what it is wired to.
    ///
    /// An address on its own is a label; what an electrician needs is "I0.3 goes to the
    /// terminal strip on this sheet". `ELECPLC` records each pin's coordinates in
    /// `X1TERMnn`/`X2TERMnn` (the same idea as the reference products' terminal-position
    /// attributes), so this can follow the wire from the pin and name the devices on it.
    pub(super) fn elec_io(&mut self, i: usize) -> Task<Message> {
        use ::elec::geometry::{netlist_from_geometry, wire_rows, Point};

        const TOLERANCE: f64 = 0.05;

        let (wires, anchors) = self.elec_collect(i);
        if wires.is_empty() {
            self.command_line
                .push_output("ELECIO: no wires on this drawing, so no point can be wired.");
            return Task::none();
        }

        // Cluster wires by shared endpoints. A PLC pin is a connection point inside the
        // symbol, not the symbol's insertion point, so the anchor-based netlist cannot see
        // it; following the wire geometry finds the conductor the pin actually lands on.
        let near = |a: (f64, f64), b: (f64, f64)| {
            (a.0 - b.0).abs() <= TOLERANCE && (a.1 - b.1).abs() <= TOLERANCE
        };
        let mut cluster: Vec<usize> = (0..wires.len()).collect();
        fn find(parent: &mut Vec<usize>, mut index: usize) -> usize {
            while parent[index] != index {
                parent[index] = parent[parent[index]];
                index = parent[index];
            }
            index
        }
        for a in 0..wires.len() {
            for b in (a + 1)..wires.len() {
                let aw = &wires[a];
                let bw = &wires[b];
                let touches = near((aw.from.x, aw.from.y), (bw.from.x, bw.from.y))
                    || near((aw.from.x, aw.from.y), (bw.to.x, bw.to.y))
                    || near((aw.to.x, aw.to.y), (bw.from.x, bw.from.y))
                    || near((aw.to.x, aw.to.y), (bw.to.x, bw.to.y));
                if touches {
                    let ra = find(&mut cluster, a);
                    let rb = find(&mut cluster, b);
                    if ra != rb {
                        cluster[ra] = rb;
                    }
                }
            }
        }

        // Resolve roots once, so the lookups below only read them.
        let roots: Vec<usize> = (0..wires.len())
            .map(|index| find(&mut cluster, index))
            .collect();

        // Devices sitting on a cluster: an anchor whose position matches a wire endpoint.
        let devices_on = |root: usize| -> Vec<String> {
            let mut names: Vec<String> = Vec::new();
            for (index, wire) in wires.iter().enumerate() {
                if roots[index] != root {
                    continue;
                }
                for anchor in &anchors {
                    let point = (anchor.at.x, anchor.at.y);
                    if near(point, (wire.from.x, wire.from.y)) || near(point, (wire.to.x, wire.to.y)) {
                        if !names.contains(&anchor.label) {
                            names.push(anchor.label.clone());
                        }
                    }
                }
            }
            names
        };

        let net_at = |x: f64, y: f64| -> Option<usize> {
            (0..wires.len()).find(|index| {
                let wire = &wires[*index];
                near((x, y), (wire.from.x, wire.from.y)) || near((x, y), (wire.to.x, wire.to.y))
            })
        };

        let modules: Vec<(String, Vec<(String, Option<(f64, f64)>)>)> = {
            let doc = &self.tabs[i].scene.document;
            let model_space = doc.header.model_space_block_handle;
            doc.entities()
                .filter(|entity| entity.common().owner_handle == model_space)
                .filter_map(|entity| match entity {
                    codec::EntityType::Insert(insert)
                        if insert.block_name.to_ascii_uppercase().starts_with("PLC-") =>
                    {
                        let mut tag = String::new();
                        for attribute in &insert.attributes {
                            if is_tag_attribute(&attribute.tag) && tag.is_empty() {
                                tag = attribute.value.trim().to_string();
                            }
                        }
                        let mut points: Vec<(String, Option<(f64, f64)>)> = Vec::new();
                        for attribute in &insert.attributes {
                            let name = attribute.tag.trim().to_ascii_uppercase();
                            if !name.starts_with("ADDR") {
                                continue;
                            }
                            let suffix = &name[4..];
                            let address = attribute.value.trim().to_string();
                            let pin = insert
                                .attributes
                                .iter()
                                .find(|other| {
                                    other
                                        .tag
                                        .trim()
                                        .eq_ignore_ascii_case(&format!("X1TERM{suffix}"))
                                })
                                .and_then(|other| {
                                    let (x, y) = other.value.split_once(',')?;
                                    Some((
                                        x.trim().parse::<f64>().ok()?,
                                        y.trim().parse::<f64>().ok()?,
                                    ))
                                });
                            points.push((address, pin));
                        }
                        Some((tag, points))
                    }
                    _ => None,
                })
                .collect()
        };

        if modules.is_empty() {
            self.command_line
                .push_output("ELECIO: no PLC modules on this drawing (place one with ELECPLC).");
            return Task::none();
        }

        let mut total = 0usize;
        let mut wired = 0usize;
        for (tag, points) in &modules {
            let header = format!("  {tag} ({} point(s)):", points.len());
            self.command_line.push_output(&header);
            for (address, pin) in points {
                total += 1;
                let target = pin.and_then(|(x, y)| net_at(x, y));
                match target {
                    Some(index) => {
                        wired += 1;
                        let root = find(&mut cluster, index);
                        let peers = devices_on(root);
                        let line = if peers.is_empty() {
                            format!("    {address} -> wired, no device on that conductor")
                        } else {
                            format!("    {address} -> {}", peers.join(", "))
                        };
                        self.command_line.push_output(&line);
                    }
                    None => {
                        let line = format!("    {address} -> not wired");
                        self.command_line.push_output(&line);
                    }
                }
            }
        }

        let summary = format!(
            "ELECIO: {} module(s), {total} point(s), {wired} wired.",
            modules.len()
        );
        self.command_line.push_output(&summary);
        Task::none()
    }

    /// `ELECPLCNEXT <tag> <points> [prefix] [x,y]` places a module continuing the project's
    /// I/O sequence.
    ///
    /// On a real drawing set the second sheet continues where the first stopped - `I1.0`
    /// after `I0.7`, not another `I0.0`. That needs the other sheets, so this scans the
    /// project folder `ELECPROJECT` remembered and takes the highest address already used.
    pub(super) fn elec_plc_next(
        &mut self,
        i: usize,
        tag: &str,
        points: usize,
        prefix: &str,
        at: Option<(f64, f64)>,
    ) -> Task<Message> {
        let prefix = if prefix.trim().is_empty() {
            "I"
        } else {
            prefix.trim()
        };

        let mut highest: Option<(usize, usize)> = None;
        let mut note: Option<String> = None;

        // Addresses already used by the project: the open project file if there is one,
        // otherwise the folder the last ELECPROJECT scanned. One source of truth, so the
        // sequence cannot disagree with the reports.
        let project_sheets = Self::project_sheets();
        if !project_sheets.is_empty() {
            let mut scanned = 0usize;
            for path in &project_sheets {
                let Ok(document) = crate::io::load_file(path) else {
                    continue;
                };
                scanned += 1;
                for component in Self::components_of(&document) {
                    for (name, value) in component.attributes.iter() {
                        if !name.trim().to_ascii_uppercase().starts_with("ADDR") {
                            continue;
                        }
                        if let Some(position) = Self::plc_parse_address(value, prefix) {
                            highest = Some(match highest {
                                Some(current) if current >= position => current,
                                _ => position,
                            });
                        }
                    }
                }
            }
            if scanned > 0 {
                note = Some(format!("{scanned} sheet(s) in the project"));
            }
        }

        // Addresses already in this drawing.
        for component in Self::components_of(&self.tabs[i].scene.document) {
            for (name, value) in component.attributes.iter() {
                if !name.trim().to_ascii_uppercase().starts_with("ADDR") {
                    continue;
                }
                if let Some(position) = Self::plc_parse_address(value, prefix) {
                    highest = Some(match highest {
                        Some(current) if current >= position => current,
                        _ => position,
                    });
                }
            }
        }

        let base = match highest {
            Some((byte, bit)) => {
                let (next_byte, next_bit) = if bit >= 7 { (byte + 1, 0) } else { (byte, bit + 1) };
                format!("{prefix}{next_byte}.{next_bit}")
            }
            None => format!("{prefix}0.0"),
        };

        let detail = match note {
            Some(text) => format!("continuing after the highest address in the {text}"),
            None => "starting the sequence (no project folder remembered)".to_string(),
        };
        let message = format!("ELECPLCNEXT: {detail}; base {base}.");
        self.command_line.push_output(&message);
        self.elec_plc(i, tag, points, &base, at)
    }

    /// Parses `I1.3` into `(1, 3)` when the prefix matches.
    fn plc_parse_address(value: &str, prefix: &str) -> Option<(usize, usize)> {
        let value = value.trim();
        let rest = value.strip_prefix(prefix)?;
        let (byte, bit) = rest.split_once('.')?;
        let byte = byte.parse::<usize>().ok()?;
        let bit = bit.parse::<usize>().ok()?;
        if bit > 7 {
            return None;
        }
        Some((byte, bit))
    }

    /// `ELECPANEL <out.dxf>` lays the drawing's devices out as a panel.
    ///
    /// The schematic says what is connected; a panel drawing says what is bolted to the
    /// backplate and in what order. This is the smallest honest version: one box per device
    /// in a grid inside an enclosure, labelled with its tag and class, written to its own
    /// drawing so the schematic is not touched. Real footprints and dimensions come later.
    pub(super) fn elec_panel(&mut self, i: usize, path: &str) -> Task<Message> {
        const CELL: f64 = 20.0;
        const BOX_W: f64 = 14.0;
        const BOX_H: f64 = 10.0;
        const MARGIN: f64 = 10.0;

        let mut devices: Vec<(String, String)> = Vec::new();
        for component in Self::components_of(&self.tabs[i].scene.document) {
            let tag = component.tag.trim().to_string();
            if tag.is_empty() {
                continue;
            }
            if devices.iter().any(|(existing, _)| existing == &tag) {
                continue;
            }
            devices.push((tag, component.class.clone()));
        }
        if devices.is_empty() {
            self.command_line
                .push_output("ELECPANEL: no tagged devices on this drawing.");
            return Task::none();
        }
        devices.sort();

        let count = devices.len();
        let columns = (count as f64).sqrt().ceil().max(1.0) as usize;
        let rows = count.div_ceil(columns);
        let width = columns as f64 * CELL + MARGIN * 2.0;
        let height = rows as f64 * CELL + MARGIN * 2.0;

        let mut document = codec::Document::new();
        let _ = document.layers.add(codec::tables::Layer::new("PANEL"));
        let _ = document.layers.add(codec::tables::Layer::new("PANEL-TEXT"));

        let mut add_line = |document: &mut codec::Document, ax: f64, ay: f64, bx: f64, by: f64| {
            let mut line = codec::entities::Line::from_points(
                codec::types::Vector3::new(ax, ay, 0.0),
                codec::types::Vector3::new(bx, by, 0.0),
            );
            line.common.layer = "PANEL".to_string();
            let _ = document.add_entity(codec::EntityType::Line(line));
        };
        let mut add_text = |document: &mut codec::Document, text: &str, x: f64, y: f64, height: f64| {
            let mut entity = codec::entities::Text::new();
            entity.value = text.to_string();
            entity.height = height;
            entity.insertion_point = codec::types::Vector3::new(x, y, 0.0);
            entity.common.layer = "PANEL-TEXT".to_string();
            let _ = document.add_entity(codec::EntityType::Text(entity));
        };

        // Enclosure
        add_line(&mut document, 0.0, 0.0, width, 0.0);
        add_line(&mut document, width, 0.0, width, height);
        add_line(&mut document, width, height, 0.0, height);
        add_line(&mut document, 0.0, height, 0.0, 0.0);

        for (index, (tag, class)) in devices.iter().enumerate() {
            let column = index % columns;
            let row = index / columns;
            let x = MARGIN + column as f64 * CELL;
            let y = MARGIN + (rows - 1 - row) as f64 * CELL;
            // Box, drawn from its lower-left corner.
            add_line(&mut document, x, y, x + BOX_W, y);
            add_line(&mut document, x + BOX_W, y, x + BOX_W, y + BOX_H);
            add_line(&mut document, x + BOX_W, y + BOX_H, x, y + BOX_H);
            add_line(&mut document, x, y + BOX_H, x, y);
            add_text(&mut document, tag, x + 1.0, y + BOX_H * 0.6, 2.0);
            add_text(&mut document, class, x + 1.0, y + BOX_H * 0.25, 1.2);
        }

        add_text(
            &mut document,
            &format!("PANEL LAYOUT - {count} device(s)"),
            MARGIN,
            height - MARGIN * 0.6,
            2.5,
        );

        match crate::io::save(&document, std::path::Path::new(path)) {
            Ok(()) => {
                let summary = format!(
                    "ELECPANEL: {count} device(s) in {columns}x{rows} cells ({width:.0}x{height:.0}); wrote {path}."
                );
                self.command_line.push_output(&summary);
            }
            Err(error) => {
                let message = format!("ELECPANEL: cannot write {path}: {error}");
                self.command_line.push_output(&message);
            }
        }
        Task::none()
    }

    /// `ELECPROJNEW <folder> [file.eprj]` writes a project file listing the drawings.
    ///
    /// A folder of DXFs is not a project: the order matters (sheet 1 before sheet 2), and
    /// numbering, cross references and reports all need to agree on which sheets belong
    /// together. The file is plain text so it can be read, edited and diffed.
    pub(super) fn elec_proj_new(&mut self, folder: &str, file: Option<&str>) -> Task<Message> {
        let directory = std::path::Path::new(folder);
        if !directory.is_dir() {
            let message = format!("ELECPROJNEW: {folder} is not a folder.");
            self.command_line.push_output(&message);
            return Task::none();
        }
        let mut sheets: Vec<String> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("dxf"))
                {
                    continue;
                }
                if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                    sheets.push(name.to_string());
                }
            }
        }
        sheets.sort();
        if sheets.is_empty() {
            let message = format!("ELECPROJNEW: no .dxf drawings in {folder}.");
            self.command_line.push_output(&message);
            return Task::none();
        }

        let path = match file {
            Some(path) => std::path::PathBuf::from(path),
            None => directory.join("project.eprj"),
        };
        let mut text = String::from(
            "# AryaCAD electrical project\n# one sheet per line, in drawing order\n",
        );
        for sheet in &sheets {
            text.push_str(&format!("sheet {sheet}\n"));
        }
        match std::fs::write(&path, text) {
            Ok(()) => {
                if let Ok(mut guard) = project_file().write() {
                    *guard = Some(path.clone());
                }
                let message = format!(
                    "ELECPROJNEW: {} sheet(s) written to {}.",
                    sheets.len(),
                    path.display()
                );
                self.command_line.push_output(&message);
            }
            Err(error) => {
                let message = format!("ELECPROJNEW: cannot write {}: {error}", path.display());
                self.command_line.push_output(&message);
            }
        }
        Task::none()
    }

    /// `ELECPROJOPEN <file.eprj>` opens a project file.
    pub(super) fn elec_proj_open(&mut self, path: &str) -> Task<Message> {
        match Self::read_project(path) {
            Ok(sheets) => {
                if let Ok(mut guard) = project_file().write() {
                    *guard = Some(std::path::PathBuf::from(path));
                }
                let message = format!(
                    "ELECPROJOPEN: {path} - {} sheet(s).",
                    sheets.len()
                );
                self.command_line.push_output(&message);
                for (index, sheet) in sheets.iter().enumerate().take(10) {
                    let label = if sheet.number.is_empty() {
                        format!("{}", index + 1)
                    } else {
                        sheet.number.clone()
                    };
                    let title = if sheet.title.is_empty() { String::new() } else { format!("  {}", sheet.title) };
                    let line = format!("  {label:>3}  {}{title}", sheet.path.display());
                    self.command_line.push_output(&line);
                }
                if sheets.len() > 10 {
                    let line = format!("  ... and {} more", sheets.len() - 10);
                    self.command_line.push_output(&line);
                }
            }
            Err(error) => {
                let message = format!("ELECPROJOPEN: {error}");
                self.command_line.push_output(&message);
            }
        }
        Task::none()
    }

    /// `ELECPROJLIST` shows the open project's sheets.
    pub(super) fn elec_proj_list(&mut self) -> Task<Message> {
        let Some(path) = project_file().read().ok().and_then(|guard| guard.clone()) else {
            self.command_line
                .push_output("ELECPROJLIST: no project open (ELECPROJNEW / ELECPROJOPEN).");
            return Task::none();
        };
        match Self::read_project(&path.display().to_string()) {
            Ok(sheets) => {
                let message = format!("ELECPROJLIST: {} - {} sheet(s).", path.display(), sheets.len());
                self.command_line.push_output(&message);
                for (index, sheet) in sheets.iter().enumerate().take(20) {
                    let label = if sheet.number.is_empty() {
                        format!("{}", index + 1)
                    } else {
                        sheet.number.clone()
                    };
                    let title = if sheet.title.is_empty() { String::new() } else { format!("  {}", sheet.title) };
                    let line = format!("  {label:>3}  {}{title}", sheet.path.display());
                    self.command_line.push_output(&line);
                }
            }
            Err(error) => {
                let message = format!("ELECPROJLIST: {error}");
                self.command_line.push_output(&message);
            }
        }
        Task::none()
    }

    /// The sheets of the current project.
    ///
    /// The open project file wins; falling back to the last scanned folder keeps the older
    /// `ELECPROJECT <folder>` workflow working. Numbering and reports both go through here,
    /// so they cannot disagree about which sheets belong to the project.
    fn project_sheets() -> Vec<std::path::PathBuf> {
        if let Some(file) = project_file().read().ok().and_then(|guard| guard.clone()) {
            if let Ok(sheets) = Self::read_project(&file.display().to_string()) {
                return sheets.into_iter().map(|sheet| sheet.path).collect();
            }
        }
        let Some(folder) = project_dir().read().ok().and_then(|guard| guard.clone()) else {
            return Vec::new();
        };
        let mut sheets: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&folder) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("dxf"))
                {
                    sheets.push(path);
                }
            }
        }
        sheets.sort();
        sheets
    }

    /// Reads a project file and resolves its sheets to paths.
    fn read_project(path: &str) -> Result<Vec<ProjectSheet>, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {path}: {error}"))?;
        let base = std::path::Path::new(path)
            .parent()
            .map(|parent| parent.to_path_buf())
            .unwrap_or_default();
        let mut sheets: Vec<ProjectSheet> = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // `number` and `title` belong to the sheet above them.
            if let Some(value) = line.strip_prefix("number ") {
                if let Some(current) = sheets.last_mut() {
                    current.number = value.trim().to_string();
                }
                continue;
            }
            if let Some(value) = line.strip_prefix("title ") {
                if let Some(current) = sheets.last_mut() {
                    current.title = value.trim().to_string();
                }
                continue;
            }
            let Some(name) = line.strip_prefix("sheet ") else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let candidate = std::path::Path::new(name);
            let resolved = if candidate.is_absolute() {
                candidate.to_path_buf()
            } else {
                base.join(candidate)
            };
            if resolved.is_file() {
                sheets.push(ProjectSheet {
                    path: resolved,
                    number: String::new(),
                    title: String::new(),
                });
            }
        }
        if sheets.is_empty() {
            return Err(format!("{path} lists no sheets that exist on disk"));
        }
        Ok(sheets)
    }

    /// `ELECPROJNUM [apply]` numbers devices across the whole project, sheet by sheet.
    ///
    /// The per-drawing `ELECNUMBER` cannot do this: a device with parts on two sheets must
    /// get one number, and the sequence must stay continuous across the set. This walks the
    /// project in order, assigns one number per device, and (only with `apply`) writes the
    /// tags back, copying each sheet to `.bak` first.
    pub(super) fn elec_proj_number(&mut self, dry_run: bool) -> Task<Message> {
        use ::elec::{default_prefix, infer_tag_prefix, NumberingFormat};

        let sheets = Self::project_sheet_list();
        if sheets.is_empty() {
            self.command_line.push_output(
                "ELECPROJNUM: no project open (ELECPROJNEW/ELECPROJOPEN) and no folder scanned.",
            );
            return Task::none();
        }

        let class_of = |block: &str| -> String {
            library_index()
                .read()
                .ok()
                .and_then(|index| {
                    index
                        .get(&block.to_ascii_uppercase())
                        .map(|entry| entry.class.clone())
                })
                .unwrap_or_else(|| block.to_string())
        };

        // Pass 1: every device, in sheet order then reading order. A tag seen again on a
        // later sheet is the same device and keeps its identity.
        struct Device {
            prefix: String,
            tag: String,
            sheets: Vec<String>,
        }
        let mut devices: Vec<Device> = Vec::new();
        let mut seen: BTreeMap<(String, String), usize> = BTreeMap::new();
        let mut duplicates: Vec<String> = Vec::new();

        for sheet in &sheets {
            let Ok(document) = crate::io::load_file(&sheet.path) else {
                continue;
            };
            let label = if sheet.number.is_empty() {
                sheet
                    .path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or("?")
                    .to_string()
            } else {
                sheet.number.clone()
            };
            let model_space = document.header.model_space_block_handle;
            let mut on_sheet: Vec<(String, String, f64, f64)> = Vec::new();
            for entity in document.entities() {
                if entity.common().owner_handle != model_space {
                    continue;
                }
                let codec::EntityType::Insert(insert) = entity else {
                    continue;
                };
                if insert.attributes.is_empty() || is_setup_block(&insert.block_name) {
                    continue;
                }
                let tag = insert
                    .attributes
                    .iter()
                    .find(|attribute| is_tag_attribute(&attribute.tag))
                    .map(|attribute| attribute.value.trim().to_string())
                    .unwrap_or_default();
                if tag.is_empty() {
                    continue;
                }
                let class = class_of(&insert.block_name);
                let prefix = infer_tag_prefix(&[tag.as_str()]).unwrap_or_else(|| default_prefix(&class));
                on_sheet.push((prefix, tag, insert.insert_point.x, insert.insert_point.y));
            }
            on_sheet.sort_by_key(|(_, _, x, y)| ::elec::reading_order_key(*x, *y));
            for (prefix, tag, _, _) in on_sheet {
                let key = (prefix.clone(), tag.to_ascii_uppercase());
                match seen.get(&key) {
                    Some(index) => {
                        let device: &mut Device = &mut devices[*index];
                        if !device.sheets.contains(&label) {
                            device.sheets.push(label.clone());
                        }
                    }
                    None => {
                        seen.insert(key, devices.len());
                        devices.push(Device {
                            prefix,
                            tag,
                            sheets: vec![label.clone()],
                        });
                    }
                }
            }
        }

        if devices.is_empty() {
            self.command_line
                .push_output("ELECPROJNUM: no tagged devices across the project.");
            return Task::none();
        }

        // Pass 2: assign numbers per prefix, in encounter order.
        let mut counters: BTreeMap<String, usize> = BTreeMap::new();
        let mut mapping: BTreeMap<String, String> = BTreeMap::new();
        let mut plan: Vec<String> = Vec::new();
        for device in &devices {
            let next = counters.entry(device.prefix.clone()).or_insert(0);
            *next += 1;
            let format = NumberingFormat::tag(&device.prefix);
            // `value` is 0-based with a start of 1, so the first device is ordinal 0.
            let value = format.value((*next - 1) as u32);
            mapping.insert(device.tag.clone(), value.clone());
            if value != device.tag {
                plan.push(format!(
                    "    {} -> {value}  (on {})",
                    device.tag,
                    device.sheets.join(", ")
                ));
            }
            if device.sheets.len() > 1 {
                duplicates.push(device.tag.clone());
            }
        }

        let changed = plan.len();
        let verb = if dry_run { "would renumber" } else { "renumbered" };
        let summary = format!(
            "ELECPROJNUM: {} sheet(s), {} device(s); {verb} {changed}.",
            sheets.len(),
            devices.len()
        );
        self.command_line.push_output(&summary);
        for line in plan.iter().take(40) {
            self.command_line.push_output(line);
        }
        if plan.len() > 40 {
            let line = format!("    ... and {} more", plan.len() - 40);
            self.command_line.push_output(&line);
        }
        if !duplicates.is_empty() {
            let line = format!("  devices spanning sheets: {}", duplicates.join(", "));
            self.command_line.push_output(&line);
        }

        if dry_run || changed == 0 {
            if dry_run {
                self.command_line
                    .push_output("  (preview only; run ELECPROJNUM apply to write)");
            }
            return Task::none();
        }

        // Pass 3: write the tags back, one sheet at a time, keeping a .bak beside each.
        let mut written = 0usize;
        let mut failed = 0usize;
        for sheet in &sheets {
            let Ok(mut document) = crate::io::load_file(&sheet.path) else {
                failed += 1;
                continue;
            };
            let model_space = document.header.model_space_block_handle;
            let handles: Vec<codec::Handle> = document
                .entities()
                .filter(|entity| entity.common().owner_handle == model_space)
                .filter_map(|entity| match entity {
                    codec::EntityType::Insert(insert)
                        if !insert.attributes.is_empty()
                            && !is_setup_block(&insert.block_name) =>
                    {
                        Some(insert.common.handle)
                    }
                    _ => None,
                })
                .collect();
            let mut touched = 0usize;
            for handle in handles {
                let Some(entity) = document.get_entity_mut(handle) else {
                    continue;
                };
                let codec::EntityType::Insert(insert) = entity else {
                    continue;
                };
                let current = insert
                    .attributes
                    .iter()
                    .find(|attribute| is_tag_attribute(&attribute.tag))
                    .map(|attribute| attribute.value.trim().to_string())
                    .unwrap_or_default();
                let Some(next) = mapping.get(&current) else {
                    continue;
                };
                if next == &current {
                    continue;
                }
                let next = next.clone();
                for attribute in insert.attributes.iter_mut() {
                    if is_tag_attribute(&attribute.tag) {
                        attribute.value = next.clone();
                    }
                }
                touched += 1;
            }
            if touched == 0 {
                continue;
            }
            let backup = sheet.path.with_extension("dxf.bak");
            let _ = std::fs::copy(&sheet.path, &backup);
            match crate::io::save(&document, &sheet.path) {
                Ok(()) => written += touched,
                Err(_) => failed += 1,
            }
        }

        let message = format!(
            "  wrote {written} tag(s) across {} sheet(s); {failed} failed. Sheets are backed up as .dxf.bak; reopen any drawing you have open.",
            sheets.len()
        );
        self.command_line.push_output(&message);
        Task::none()
    }

    /// The project's sheets with their metadata: the open project file, else the folder.
    fn project_sheet_list() -> Vec<ProjectSheet> {
        if let Some(file) = project_file().read().ok().and_then(|guard| guard.clone()) {
            if let Ok(sheets) = Self::read_project(&file.display().to_string()) {
                return sheets;
            }
        }
        Self::project_sheets()
            .into_iter()
            .map(|path| ProjectSheet {
                path,
                number: String::new(),
                title: String::new(),
            })
            .collect()
    }

    pub(super) fn elec_sheet(&mut self, i: usize, size: &str, title: &str) -> Task<Message> {
        let (paper_w, paper_h) = match size.trim().to_ascii_uppercase().as_str() {
            "A3" => (420.0_f64, 297.0_f64),
            "A2" => (594.0, 420.0),
            "A1" => (841.0, 594.0),
            "A0" => (1189.0, 841.0),
            _ => (297.0, 210.0),
        };

        // The frame has to fit the drawing, not the other way round: the imported sheets hold
        // roughly 35x25 units of content, and a frame drawn at 297x210 mm in those units is
        // eight times too big. Scale the paper to the content's own extent.
        let mut min_x = f64::MAX;
        let mut min_y = f64::MAX;
        let mut max_x = f64::MIN;
        let mut max_y = f64::MIN;
        {
            let doc = &self.tabs[i].scene.document;
            let model_space = doc.header.model_space_block_handle;
            let mut take = |x: f64, y: f64| {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            };
            for entity in doc.entities() {
                if entity.common().owner_handle != model_space {
                    continue;
                }
                // Ignore an existing frame, so running ELECSHEET twice refits rather than
                // measuring its own borders.
                if entity.common().layer.as_str() == "ELEC-FRAME" {
                    continue;
                }
                match entity {
                    codec::EntityType::Line(line) => {
                        take(line.start.x, line.start.y);
                        take(line.end.x, line.end.y);
                    }
                    codec::EntityType::Circle(circle) => {
                        let r = circle.radius.abs();
                        take(circle.center.x - r, circle.center.y - r);
                        take(circle.center.x + r, circle.center.y + r);
                    }
                    codec::EntityType::Arc(arc) => {
                        let r = arc.radius.abs();
                        take(arc.center.x - r, arc.center.y - r);
                        take(arc.center.x + r, arc.center.y + r);
                    }
                    codec::EntityType::Text(text) => {
                        take(text.insertion_point.x, text.insertion_point.y);
                    }
                    codec::EntityType::Insert(insert) => {
                        take(insert.insert_point.x, insert.insert_point.y);
                    }
                    codec::EntityType::LwPolyline(polyline) => {
                        for vertex in &polyline.vertices {
                            take(vertex.location.x, vertex.location.y);
                        }
                    }
                    _ => {}
                }
            }
        }

        let (centre_x, centre_y, scale) = if min_x < max_x && min_y < max_y {
            let width = (max_x - min_x).max(1e-9);
            let height = (max_y - min_y).max(1e-9);
            // 15% breathing room around the content.
            let scale = ((width * 1.15) / paper_w).max((height * 1.15) / paper_h);
            (
                (min_x + max_x) * 0.5,
                (min_y + max_y) * 0.5,
                scale,
            )
        } else {
            // Empty drawing: draw a real sheet at 1:1 around the origin.
            (paper_w * 0.5, paper_h * 0.5, 1.0)
        };

        let frame_w = paper_w * scale;
        let frame_h = paper_h * scale;
        let origin_x = centre_x - frame_w * 0.5;
        let origin_y = centre_y - frame_h * 0.5;
        let mm = |value: f64| value * scale;

        // What the title block says about this sheet: project metadata if this file is in
        // the open project, otherwise whatever the caller supplied.
        let current = self.tabs[i]
            .current_path
            .clone()
            .map(|path| std::path::PathBuf::from(path));
        let mut sheet_number = String::new();
        let mut sheet_title = title.trim().to_string();
        let mut project_name = String::new();
        if let Some(file) = project_file().read().ok().and_then(|guard| guard.clone()) {
            project_name = file
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("project")
                .to_string();
            if let Ok(sheets) = Self::read_project(&file.display().to_string()) {
                if let Some(current) = &current {
                    for (index, sheet) in sheets.iter().enumerate() {
                        let same = sheet.path.file_name() == current.file_name();
                        if !same {
                            continue;
                        }
                        sheet_number = if sheet.number.is_empty() {
                            (index + 1).to_string()
                        } else {
                            sheet.number.clone()
                        };
                        if sheet_title.is_empty() {
                            sheet_title = sheet.title.clone();
                        }
                        break;
                    }
                }
            }
        }

        let mut lines: Vec<(f64, f64, f64, f64)> = Vec::new();
        let mut texts: Vec<(String, f64, f64, f64)> = Vec::new();

        // Paper edge and inner frame, in drawing units.
        let (left_edge, bottom_edge) = (origin_x, origin_y);
        let (right_edge, top_edge) = (origin_x + frame_w, origin_y + frame_h);
        lines.push((left_edge, bottom_edge, right_edge, bottom_edge));
        lines.push((right_edge, bottom_edge, right_edge, top_edge));
        lines.push((right_edge, top_edge, left_edge, top_edge));
        lines.push((left_edge, top_edge, left_edge, bottom_edge));
        let inset = mm(10.0);
        lines.push((left_edge + inset, bottom_edge + inset, right_edge - inset, bottom_edge + inset));
        lines.push((right_edge - inset, bottom_edge + inset, right_edge - inset, top_edge - inset));
        lines.push((right_edge - inset, top_edge - inset, left_edge + inset, top_edge - inset));
        lines.push((left_edge + inset, top_edge - inset, left_edge + inset, bottom_edge + inset));

        // Title block, bottom right inside the frame.
        let block_w = mm(120.0);
        let block_h = mm(32.0);
        let block_left = right_edge - inset - block_w;
        let block_bottom = bottom_edge + inset;
        lines.push((block_left, block_bottom, block_left + block_w, block_bottom));
        lines.push((block_left + block_w, block_bottom, block_left + block_w, block_bottom + block_h));
        lines.push((block_left + block_w, block_bottom + block_h, block_left, block_bottom + block_h));
        lines.push((block_left, block_bottom + block_h, block_left, block_bottom));
        let row1 = block_bottom + mm(20.0);
        let row2 = block_bottom + mm(12.0);
        let row3 = block_bottom + mm(6.0);
        lines.push((block_left, row1, block_left + block_w, row1));
        lines.push((block_left, row2, block_left + block_w, row2));
        lines.push((block_left, row3, block_left + block_w, row3));
        let split = block_left + block_w * 0.65;
        lines.push((split, block_bottom, split, row3));

        let number_text = if sheet_number.is_empty() {
            "SHEET".to_string()
        } else {
            format!("SHEET {sheet_number}")
        };
        texts.push((number_text, block_left + mm(2.0), block_bottom + mm(23.0), mm(6.0)));
        texts.push((
            if sheet_title.is_empty() {
                "UNTITLED".to_string()
            } else {
                sheet_title.to_uppercase()
            },
            block_left + mm(2.0),
            block_bottom + mm(14.0),
            mm(4.0),
        ));
        texts.push((
            if project_name.is_empty() {
                "ARYACAD ELECTRICAL".to_string()
            } else {
                project_name.to_uppercase()
            },
            block_left + mm(2.0),
            block_bottom + mm(8.0),
            mm(3.0),
        ));
        let size_label = format!(
            "{}  {} x {}",
            size.trim().to_ascii_uppercase(),
            paper_w as i64,
            paper_h as i64
        );
        texts.push((size_label, split + mm(2.0), block_bottom + mm(2.0), mm(2.5)));
        texts.push((
            format!("1:{}", if scale > 0.0 { (1.0 / scale).round() as i64 } else { 1 }),
            block_left + mm(2.0),
            block_bottom + mm(2.0),
            mm(2.5),
        ));

        self.push_undo_snapshot(i, "ELECSHEET");
        for (ax, ay, bx, by) in lines {
            let mut line = codec::entities::Line::from_points(
                codec::types::Vector3::new(ax, ay, 0.0),
                codec::types::Vector3::new(bx, by, 0.0),
            );
            line.common.layer = "ELEC-FRAME".to_string();
            self.tabs[i].scene.add_entity(codec::EntityType::Line(line));
        }
        for (value, x, y, height) in texts {
            let mut text = codec::entities::Text::new();
            text.value = value;
            text.height = height;
            text.insertion_point = codec::types::Vector3::new(x, y, 0.0);
            text.common.layer = "ELEC-FRAME".to_string();
            self.tabs[i].scene.add_entity(codec::EntityType::Text(text));
        }
        self.tabs[i].dirty = true;

        let summary = format!(
            "ELECSHEET: {} fitted to the drawing at 1:{} ({frame_w:.2}x{frame_h:.2} units); title block shows sheet {} / {}.",
            size.trim().to_ascii_uppercase(),
            if scale > 0.0 { (1.0 / scale).round() as i64 } else { 1 },
            if sheet_number.is_empty() { "-" } else { &sheet_number },
            if sheet_title.is_empty() { "-" } else { &sheet_title }
        );
        self.command_line.push_output(&summary);
        Task::none()
    }


    /// Command entry point. Returns `None` when the command is not ours so the shared
    /// dispatcher can try the next family.
    pub(super) fn dispatch_elec(&mut self, cmd: &str, i: usize) -> Option<Task<Message>> {
        match cmd {
            "ELECBOM" => Some(self.elec_bom(i, None)),
            "ELECLIST" => Some(self.elec_list(i)),
            "ELECWIRELIST" => Some(self.elec_wirelist(i)),
            "ELECWIRENUMBER" => Some(self.elec_wirenumber(i)),
            "ELECTERMINALS" => Some(self.elec_terminals(i)),
            "ELECTSIDES" => Some(self.elec_sides(i)),
            "ELECIO" => Some(self.elec_io(i)),
            // The palette is UI, not a drawing edit: it opens the modal that lists the
            // loaded library, and a click in it runs this same ELECINSERT command.
            "ELECMAKESYMBOL" => {
                self.command_line.push_output("ELECMAKESYMBOL: select geometry, then ELECMAKESYMBOL <name>");
                Some(Task::none())
            }
            "ELECNEW" => Some(self.elec_new(i, None)),
            "ELECWIRE" => Some(self.elec_wire(i)),
            "ELECJUNCTIONS" => Some(self.elec_junctions(i)),
            "ELECATTR" => {
                self.command_line
                    .push_output("ELECATTR: use ELECSETATTR <tag> NAME=value");
                Some(Task::none())
            }
            "ELECPLACE" => {
                // Click-to-place: the palette routes here so the symbol follows the cursor.
                let loaded = library_index().read().map(|i| !i.is_empty()).unwrap_or(false);
                if !loaded {
                    if let Some(directory) = remembered_library() {
                        let path = directory.display().to_string();
                        let _ = self.elec_lib(i, &path);
                    }
                }
                None
            }
            "ELECPALETTE" => {
                // The palette is useless without symbols, and the folder is remembered, so
                // open it with the library already loaded when this session has not loaded
                // one yet.
                let loaded = library_index()
                    .read()
                    .map(|index| !index.is_empty())
                    .unwrap_or(false);
                if !loaded {
                    if let Some(directory) = remembered_library() {
                        let path = directory.display().to_string();
                        let _ = self.elec_lib(i, &path);
                    }
                }
                self.active_modal = Some(crate::app::ModalKind::ElectricalLibrary);
                self.reset_modal_geometry();
                Some(Task::none())
            }
            "ELECXREF" => Some(self.elec_xref(i)),
            "ELECXREFWRITE" => Some(self.elec_xref_write(i)),
            "ELECNUMBER" => Some(self.elec_number(i, false)),
            // A separate command, not an argument form: the command dispatcher can run a
            // line twice when it carries a space-separated argument, and a preview that
            // mutates the drawing is worse than no preview at all.
            "ELECNUMBERPREVIEW" => Some(self.elec_number(i, true)),
            _ => {
                if let Some(argument) = cmd.strip_prefix("ELECBOM ") {
                    let path = argument.trim();
                    if !path.is_empty() {
                        return Some(self.elec_bom(i, Some(path)));
                    }
                }
                if let Some(argument) = cmd.strip_prefix("ELECPART ") {
                    let path = argument.trim();
                    if !path.is_empty() {
                        return Some(self.elec_parts(path));
                    }
                }
                if let Some(argument) = cmd.strip_prefix("ELECREPORT ") {
                    let path = argument.trim();
                    if !path.is_empty() {
                        return Some(self.elec_report(i, path));
                    }
                }
                if let Some(argument) = cmd.strip_prefix("ELECMAKESYMBOL ") {
                    let name = argument.trim();
                    if !name.is_empty() {
                        return Some(self.elec_make_symbol(i, name));
                    }
                }
                if let Some(argument) = cmd.strip_prefix("ELECSETATTR ") {
                    let mut words = argument.split_whitespace();
                    if let Some(tag) = words.next() {
                        let rest = words.collect::<Vec<_>>().join(" ");
                        return Some(self.elec_set_attr(i, tag, &rest));
                    }
                }
                if let Some(argument) = cmd.strip_prefix("ELECPLACE ") {
                    let name = argument.trim();
                    if !name.is_empty() {
                        return Some(self.elec_place(i, name));
                    }
                }
                if cmd == "ELECNEW" {
                    return Some(self.elec_new(i, None));
                }
                if cmd == "ELECPROJLIST" {
                    return Some(self.elec_proj_list());
                }
                if let Some(argument) = cmd.strip_prefix("ELECPROJOPEN ") {
                    let path = argument.trim();
                    if !path.is_empty() {
                        return Some(self.elec_proj_open(path));
                    }
                }
                if let Some(argument) = cmd.strip_prefix("ELECPROJNEW ") {
                    let mut words = argument.split_whitespace();
                    if let Some(folder) = words.next() {
                        let file = words.next();
                        return Some(self.elec_proj_new(folder, file));
                    }
                }
                if let Some(argument) = cmd.strip_prefix("ELECPANEL ") {
                    let path = argument.trim();
                    if !path.is_empty() {
                        return Some(self.elec_panel(i, path));
                    }
                }
                if let Some(argument) = cmd.strip_prefix("ELECPLCNEXT ") {
                    // ELECPLCNEXT <tag> <points> [prefix] [x,y]
                    let mut words = argument.split_whitespace();
                    let tag = words.next().unwrap_or("");
                    let points = words
                        .next()
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(0);
                    let mut prefix = "I";
                    let mut at = None;
                    for word in words {
                        if let Some((x, y)) = word.split_once(',') {
                            at = Some((
                                x.trim().parse::<f64>().unwrap_or(0.0),
                                y.trim().parse::<f64>().unwrap_or(0.0),
                            ));
                        } else if word.len() == 1 {
                            prefix = word;
                        }
                    }
                    return Some(self.elec_plc_next(i, tag, points, prefix, at));
                }
                if let Some(argument) = cmd.strip_prefix("ELECPLC ") {
                    // ELECPLC <tag> <points> <base-address> [x,y]
                    let mut words = argument.split_whitespace();
                    let tag = words.next().unwrap_or("");
                    let points = words
                        .next()
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(0);
                    let base = words.next().unwrap_or("");
                    let at = words.next().and_then(|text| {
                        let (x, y) = text.split_once(',')?;
                        Some((
                            x.trim().parse::<f64>().ok()?,
                            y.trim().parse::<f64>().ok()?,
                        ))
                    });
                    return Some(self.elec_plc(i, tag, points, base, at));
                }
                if let Some(argument) = cmd.strip_prefix("ELECSHEET ") {
                    let mut words = argument.split_whitespace();
                    let size = words.next().unwrap_or("A4");
                    let title = argument.trim().trim_start_matches(size).trim();
                    return Some(self.elec_sheet(i, size, title));
                }
                if cmd == "ELECSHEET" {
                    return Some(self.elec_sheet(i, "A4", ""));
                }
                if cmd == "ELECPROJNUM" {
                    return Some(self.elec_proj_number(true));
                }
                if let Some(argument) = cmd.strip_prefix("ELECPROJNUM ") {
                    let apply = argument.trim().eq_ignore_ascii_case("apply");
                    return Some(self.elec_proj_number(!apply));
                }
                if cmd == "ELECPROJECT" {
                    // No folder: read the open project file.
                    return Some(self.elec_project(None, None));
                }
                if let Some(argument) = cmd.strip_prefix("ELECPROJECT ") {
                    // `ELECPROJECT <folder> [out.xlsx]`: the optional path writes the
                    // project BOM, the per-sheet summary and the class breakdown.
                    let mut words = argument.split_whitespace();
                    if let Some(first) = words.next() {
                        let out = words.next();
                        // A single .xlsx argument means "the open project, written here".
                        if out.is_none()
                            && first.to_ascii_lowercase().ends_with(".xlsx")
                            && project_file().read().map(|guard| guard.is_some()).unwrap_or(false)
                        {
                            return Some(self.elec_project(None, Some(first)));
                        }
                        return Some(self.elec_project(Some(first), out));
                    }
                }
                if let Some(argument) = cmd.strip_prefix("ELECNEW ") {
                    let path = argument.trim();
                    if !path.is_empty() {
                        return Some(self.elec_new(i, Some(path)));
                    }
                }
                if let Some(argument) = cmd.strip_prefix("ELECLIB ") {
                    let path = argument.trim();
                    if !path.is_empty() {
                        return Some(self.elec_lib(i, path));
                    }
                }
                if let Some(argument) = cmd.strip_prefix("ELECINSERT ") {
                    let mut words = argument.split_whitespace();
                    if let Some(block) = words.next() {
                        let part = words.next();
                        return Some(self.elec_insert(i, block, part));
                    }
                }
                None
            }
        }
    }
}

/// True when an attribute carries the device tag.
fn is_tag_attribute(tag: &str) -> bool {
    matches!(
        tag.trim().to_ascii_uppercase().as_str(),
        "TAG" | "TAG1" | "TAG2" | "TAGSTR" | "WORKID"
    )
}

/// Write bill-of-material rows as CSV, using the column names the report templates use.
fn write_bom_csv(path: &str, rows: &[::elec::BomRow]) -> std::io::Result<()> {
    let mut out = String::from("Class name,Part number,Manufacturer,Description,Qty,Where used\n");
    for row in rows {
        out.push_str(&csv_field(&row.class));
        out.push(',');
        out.push_str(&csv_field(row.part.as_deref().unwrap_or("")));
        out.push(',');
        out.push_str(&csv_field(row.manufacturer.as_deref().unwrap_or("")));
        out.push(',');
        out.push_str(&csv_field(row.description.as_deref().unwrap_or("")));
        out.push(',');
        out.push_str(&row.quantity.to_string());
        out.push(',');
        out.push_str(&csv_field(&row.where_used.join(", ")));
        out.push('\n');
    }
    std::fs::write(path, out)
}

/// Quote a CSV field when it contains a delimiter, quote or newline.
fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// Shorten a string for fixed-width console output.
fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut out: String = text.chars().take(width.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

/// True for a drawing-setup block rather than a device.
///
/// AutoCAD Electrical stores the sheet's electrical settings in the `WD_M` block, which
/// carries dozens of attributes. Treating it as a component put a fake `WD_M` line in the
/// bill of material and numbered it as a device on the production sheets before this filter.
fn is_setup_block(block_name: &str) -> bool {
    let name = block_name.trim().to_ascii_uppercase();
    name == "WD_M" || name.starts_with("WD_")
}


/// Write sheets as a minimal xlsx workbook.
///
/// The workbook is built directly rather than through a spreadsheet crate: three parts are
/// needed (content types, workbook, worksheets), strings are written inline so there is no
/// shared-string table to maintain, and the `zip` crate already in the tree packages it.
/// Cells are values, not formulas, which is what a report wants.
fn write_xlsx(path: &str, sheets: &[::elec::Sheet]) -> Result<(), String> {
    use std::io::Write;

    let file = std::fs::File::create(path).map_err(|error| error.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let put = |zip: &mut zip::ZipWriter<std::fs::File>, name: &str, data: &str| -> Result<(), String> {
        zip.start_file(name, options)
            .map_err(|error| error.to_string())?;
        zip.write_all(data.as_bytes())
            .map_err(|error| error.to_string())
    };

    let mut content_types = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>",
    );
    for index in 1..=sheets.len() {
        content_types.push_str(&format!(
            "<Override PartName=\"/xl/worksheets/sheet{index}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"
        ));
    }
    content_types.push_str("</Types>");
    put(&mut zip, "[Content_Types].xml", &content_types)?;

    put(
        &mut zip,
        "_rels/.rels",
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
         </Relationships>",
    )?;

    let mut workbook = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets>",
    );
    for (index, sheet) in sheets.iter().enumerate() {
        workbook.push_str(&format!(
            "<sheet name=\"{}\" sheetId=\"{}\" r:id=\"rId{}\"/>",
            xml_escape(&sheet.name),
            index + 1,
            index + 1
        ));
    }
    workbook.push_str("</sheets></workbook>");
    put(&mut zip, "xl/workbook.xml", &workbook)?;

    let mut workbook_rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    );
    for index in 1..=sheets.len() {
        workbook_rels.push_str(&format!(
            "<Relationship Id=\"rId{index}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{index}.xml\"/>"
        ));
    }
    workbook_rels.push_str("</Relationships>");
    put(&mut zip, "xl/_rels/workbook.xml.rels", &workbook_rels)?;

    for (index, sheet) in sheets.iter().enumerate() {
        let mut xml = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>",
        );
        for (row_index, row) in sheet.rows.iter().enumerate() {
            let number = row_index + 1;
            xml.push_str(&format!("<row r=\"{number}\">"));
            for (column_index, cell) in row.iter().enumerate() {
                let reference = format!("{}{number}", column_name(column_index));
                match cell {
                    ::elec::Cell::Text(text) => xml.push_str(&format!(
                        "<c r=\"{reference}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                        xml_escape(text)
                    )),
                    ::elec::Cell::Number(value) => {
                        if value.fract() == 0.0 && value.abs() < 1e15 {
                            xml.push_str(&format!("<c r=\"{reference}\"><v>{}</v></c>", *value as i64));
                        } else {
                            xml.push_str(&format!("<c r=\"{reference}\"><v>{value}</v></c>"));
                        }
                    }
                }
            }
            xml.push_str("</row>");
        }
        xml.push_str("</sheetData></worksheet>");
        put(&mut zip, &format!("xl/worksheets/sheet{}.xml", index + 1), &xml)?;
    }

    zip.finish().map_err(|error| error.to_string())?;
    Ok(())
}

/// Spreadsheet column name for a zero-based index: 0 -> A, 25 -> Z, 26 -> AA.
fn column_name(index: usize) -> String {
    let mut name = String::new();
    let mut n = index + 1;
    while n > 0 {
        let remainder = (n - 1) % 26;
        name.insert(0, (b'A' + remainder as u8) as char);
        n = (n - 1) / 26;
    }
    name
}

/// Escape text for XML content and attributes.
fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Control characters are illegal in XML; drop them rather than emit a broken file.
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            other => out.push(other),
        }
    }
    out
}




























