//! Geometric connectivity: turn drawn wires into nets.
//!
//! A schematic is drawn as lines and symbols, but it is *read* as a graph. This module does
//! the conversion: wire segments join at shared endpoints, and a symbol anchors to a net
//! when its connection point lands on a wire endpoint.
//!
//! The rule is deliberately conservative. Two things connect when their points coincide
//! within a tolerance; nothing is inferred from proximity alone, and a wire that crosses
//! another without sharing an endpoint does not merge them (that is what schematic dots
//! are for, and a crossing without a dot is not a connection).

use std::collections::BTreeMap;

/// A point in drawing units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// A drawn wire: one segment between two points.
#[derive(Debug, Clone, PartialEq)]
pub struct WireSeg {
    pub id: u64,
    pub from: Point,
    pub to: Point,
}

impl WireSeg {
    pub fn new(id: u64, from: Point, to: Point) -> Self {
        Self { id, from, to }
    }
}

/// Something that attaches to a net at a point: a component's connection point.
#[derive(Debug, Clone, PartialEq)]
pub struct Anchor {
    pub id: u64,
    /// Human-readable identity for reports, e.g. the device tag.
    pub label: String,
    pub at: Point,
}

impl Anchor {
    pub fn new(id: u64, label: &str, at: Point) -> Self {
        Self {
            id,
            label: label.to_string(),
            at,
        }
    }
}

/// One electrical net found in the geometry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeoNet {
    /// Stable identity: the lowest wire id in the net, or the lowest anchor id when the
    /// net is a single symbol with no wires.
    pub id: u64,
    /// Anchored component ids, ascending.
    pub anchors: Vec<u64>,
    /// Wire ids, ascending.
    pub wires: Vec<u64>,
}

/// Result of the geometric pass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GeoNetlist {
    pub nets: Vec<GeoNet>,
    /// Anchors that no wire reached.
    pub loose_anchors: Vec<u64>,
    /// Wires in no net (a wire with no anchor and no other wire).
    pub loose_wires: Vec<u64>,
}

/// Grid cell a point falls into. Coordinates are divided by the tolerance and rounded, so
/// two endpoints within half a cell of each other land on the same node.
type Cell = (i64, i64);

fn cell(point: Point, tolerance: f64) -> Cell {
    let tol = if tolerance > 0.0 { tolerance } else { 1e-6 };
    (
        (point.x / tol).round() as i64,
        (point.y / tol).round() as i64,
    )
}

/// Union-find over grid cells.
struct DisjointSet {
    parent: BTreeMap<Cell, Cell>,
}

impl DisjointSet {
    fn new() -> Self {
        Self {
            parent: BTreeMap::new(),
        }
    }

    fn find(&mut self, node: Cell) -> Cell {
        let parent = *self.parent.entry(node).or_insert(node);
        if parent == node {
            return node;
        }
        let root = self.find(parent);
        self.parent.insert(node, root);
        root
    }

    fn union(&mut self, a: Cell, b: Cell) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        let (lo, hi) = if (ra.0, ra.1) <= (rb.0, rb.1) { (ra, rb) } else { (rb, ra) };
        self.parent.insert(hi, lo);
    }
}

/// Build nets from drawn wires and component anchors.
///
/// `tolerance` is the distance below which two points are the same node. A typical drawing
/// uses something around the snap grid (0.1 mm at 1:1, or 1 unit at schematic scale).
pub fn netlist_from_geometry(wires: &[WireSeg], anchors: &[Anchor], tolerance: f64) -> GeoNetlist {
    let mut dsu = DisjointSet::new();
    for wire in wires {
        let a = cell(wire.from, tolerance);
        let b = cell(wire.to, tolerance);
        dsu.union(a, b);
    }
    for anchor in anchors {
        // Registering the anchor's node is what lets a symbol with no wires still be
        // reported as unconnected rather than silently vanishing.
        dsu.find(cell(anchor.at, tolerance));
    }

    // Group wires and anchors by their root cell.
    let mut by_root: BTreeMap<Cell, (Vec<u64>, Vec<u64>)> = BTreeMap::new();
    for wire in wires {
        let root = dsu.find(cell(wire.from, tolerance));
        by_root.entry(root).or_default().0.push(wire.id);
    }
    let mut anchor_root: BTreeMap<u64, Cell> = BTreeMap::new();
    for anchor in anchors {
        let root = dsu.find(cell(anchor.at, tolerance));
        by_root.entry(root).or_default().1.push(anchor.id);
        anchor_root.insert(anchor.id, root);
    }

    let mut nets = Vec::new();
    let mut loose_anchors = Vec::new();
    let mut loose_wires = Vec::new();

    for (_root, (mut wire_ids, mut anchor_ids)) in by_root {
        wire_ids.sort_unstable();
        anchor_ids.sort_unstable();
        if wire_ids.is_empty() && anchor_ids.len() == 1 {
            // A symbol alone is not a net; it is an unconnected terminal.
            loose_anchors.extend(anchor_ids);
            continue;
        }
        if anchor_ids.is_empty() {
            // Wires with no symbol on them still form a net electrically (a cable run
            // between two sheets), but with nothing attached they are reported as loose
            // so the drawing can show them as unconnected drafting.
            loose_wires.extend(wire_ids);
            continue;
        }
        let id = wire_ids
            .first()
            .copied()
            .or_else(|| anchor_ids.first().copied())
            .unwrap_or(0);
        nets.push(GeoNet {
            id,
            anchors: anchor_ids,
            wires: wire_ids,
        });
    }

    loose_anchors.sort_unstable();
    loose_wires.sort_unstable();
    nets.sort_by_key(|net| net.id);
    GeoNetlist {
        nets,
        loose_anchors,
        loose_wires,
    }
}

/// One row of the wire list report.
#[derive(Debug, Clone, PartialEq)]
pub struct GeoWireRow {
    pub wire: u64,
    pub net: u64,
    pub from: Point,
    pub to: Point,
}

/// Project the wires of a netlist into report rows, ordered by wire id.
pub fn wire_rows(wires: &[WireSeg], netlist: &GeoNetlist) -> Vec<GeoWireRow> {
    let mut net_of_wire: BTreeMap<u64, u64> = BTreeMap::new();
    for net in &netlist.nets {
        for wire in &net.wires {
            net_of_wire.insert(*wire, net.id);
        }
    }
    let mut rows: Vec<GeoWireRow> = wires
        .iter()
        .filter_map(|wire| {
            net_of_wire.get(&wire.id).map(|net| GeoWireRow {
                wire: wire.id,
                net: *net,
                from: wire.from,
                to: wire.to,
            })
        })
        .collect();
    rows.sort_by_key(|row| row.wire);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn two_symbols_joined_by_one_wire_form_a_net() {
        let wires = vec![WireSeg::new(1, p(0.0, 0.0), p(100.0, 0.0))];
        let anchors = vec![Anchor::new(10, "-K1", p(0.0, 0.0)), Anchor::new(11, "-K2", p(100.0, 0.0))];
        let netlist = netlist_from_geometry(&wires, &anchors, 0.01);
        assert_eq!(netlist.nets.len(), 1);
        assert_eq!(netlist.nets[0].anchors, vec![10, 11]);
        assert_eq!(netlist.nets[0].wires, vec![1]);
        assert_eq!(netlist.nets[0].id, 1);
    }

    #[test]
    fn chained_wires_merge_into_one_net() {
        let wires = vec![
            WireSeg::new(1, p(0.0, 0.0), p(50.0, 0.0)),
            WireSeg::new(2, p(50.0, 0.0), p(100.0, 0.0)),
            WireSeg::new(3, p(100.0, 0.0), p(150.0, 0.0)),
        ];
        let anchors = vec![Anchor::new(10, "-K1", p(0.0, 0.0)), Anchor::new(11, "-K2", p(150.0, 0.0))];
        let netlist = netlist_from_geometry(&wires, &anchors, 0.01);
        assert_eq!(netlist.nets.len(), 1);
        assert_eq!(netlist.nets[0].wires, vec![1, 2, 3]);
        assert_eq!(netlist.nets[0].anchors, vec![10, 11]);
    }

    #[test]
    fn crossing_wires_without_a_shared_point_stay_separate() {
        // A horizontal and a vertical wire crossing at (50,0) but neither ending there.
        let wires = vec![
            WireSeg::new(1, p(0.0, 0.0), p(100.0, 0.0)),
            WireSeg::new(2, p(50.0, -50.0), p(50.0, 50.0)),
        ];
        let anchors = vec![
            Anchor::new(10, "-K1", p(0.0, 0.0)),
            Anchor::new(11, "-K2", p(50.0, 50.0)),
        ];
        let netlist = netlist_from_geometry(&wires, &anchors, 0.01);
        assert_eq!(netlist.nets.len(), 2);
    }

    #[test]
    fn a_symbol_on_no_wire_is_reported_loose() {
        let wires = vec![WireSeg::new(1, p(0.0, 0.0), p(100.0, 0.0))];
        let anchors = vec![Anchor::new(10, "-K1", p(0.0, 0.0)), Anchor::new(11, "-K9", p(500.0, 500.0))];
        let netlist = netlist_from_geometry(&wires, &anchors, 0.01);
        assert_eq!(netlist.loose_anchors, vec![11]);
        assert_eq!(netlist.nets.len(), 1);
    }

    #[test]
    fn wires_with_no_symbol_are_reported_loose() {
        let wires = vec![WireSeg::new(7, p(0.0, 0.0), p(10.0, 0.0))];
        let netlist = netlist_from_geometry(&wires, &[], 0.01);
        assert!(netlist.nets.is_empty());
        assert_eq!(netlist.loose_wires, vec![7]);
    }

    #[test]
    fn tolerance_joins_near_miss_endpoints() {
        let wires = vec![
            WireSeg::new(1, p(0.0, 0.0), p(50.0, 0.0)),
            WireSeg::new(2, p(50.02, 0.0), p(100.0, 0.0)),
        ];
        let anchors = vec![Anchor::new(10, "-K1", p(0.0, 0.0)), Anchor::new(11, "-K2", p(100.0, 0.0))];
        let joined = netlist_from_geometry(&wires, &anchors, 0.5);
        assert_eq!(joined.nets.len(), 1);
        let apart = netlist_from_geometry(&wires, &anchors, 0.001);
        assert!(apart.nets.is_empty() || apart.nets.len() == 2);
    }

    #[test]
    fn net_identity_is_stable_under_input_order() {
        let wires = vec![
            WireSeg::new(2, p(50.0, 0.0), p(100.0, 0.0)),
            WireSeg::new(1, p(0.0, 0.0), p(50.0, 0.0)),
        ];
        let anchors = vec![Anchor::new(10, "-K1", p(0.0, 0.0)), Anchor::new(11, "-K2", p(100.0, 0.0))];
        let netlist = netlist_from_geometry(&wires, &anchors, 0.01);
        assert_eq!(netlist.nets.len(), 1);
        assert_eq!(netlist.nets[0].wires, vec![1, 2]);
        assert_eq!(netlist.nets[0].id, 1);
    }

    #[test]
    fn coincident_symbols_form_a_net_with_no_wires() {
        // Two symbols dropped on the same point are electrically joined by contact, but
        // there is no wire to hang a wire number on. Consumers must cope with an empty
        // wire list rather than indexing it.
        let anchors = vec![
            Anchor::new(10, "WD_M", p(50.0, 50.0)),
            Anchor::new(11, "WD_M", p(50.0, 50.0)),
        ];
        let netlist = netlist_from_geometry(&[], &anchors, 0.01);
        assert_eq!(netlist.nets.len(), 1);
        assert!(netlist.nets[0].wires.is_empty());
        assert_eq!(netlist.nets[0].anchors, vec![10, 11]);
    }

    #[test]
    fn wire_rows_carry_the_net() {
        let wires = vec![
            WireSeg::new(1, p(0.0, 0.0), p(50.0, 0.0)),
            WireSeg::new(2, p(50.0, 0.0), p(100.0, 0.0)),
            WireSeg::new(9, p(0.0, 90.0), p(10.0, 90.0)),
        ];
        let anchors = vec![Anchor::new(10, "-K1", p(0.0, 0.0)), Anchor::new(11, "-K2", p(100.0, 0.0))];
        let netlist = netlist_from_geometry(&wires, &anchors, 0.01);
        let rows = wire_rows(&wires, &netlist);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].wire, 1);
        assert_eq!(rows[1].wire, 2);
        assert_eq!(rows[0].net, 1);
        assert_eq!(rows[1].net, 1);
    }
}
