//! Connectivity: ports, wires and the nets they form.
//!
//! This is the electrical heart of the application. On screen a schematic is lines and
//! symbols; electrically it is a set of terminals joined by wires, grouped into nets
//! (equipotentials). Everything else — wire numbering, cross references, terminal strips,
//! the wire list report — is derived from this graph.
//!
//! The model is deliberately small: ports belong to components, wires join two ports, and
//! nets are the connected components of that graph. Drawing geometry decides *which* ports
//! a wire joins; this module only needs the answer.

use std::collections::BTreeMap;

/// A terminal on a component. `name` is what the drawing prints (`1`, `A1`, `L+`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub id: u64,
    pub component: u64,
    pub name: String,
}

impl Port {
    pub fn new(id: u64, component: u64, name: &str) -> Self {
        Self {
            id,
            component,
            name: name.to_string(),
        }
    }
}

/// A wire joining two ports. A wire that is not yet connected to a second port keeps
/// `to: None` so the drawing can report it as dangling instead of silently dropping it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wire {
    pub id: u64,
    pub from: u64,
    pub to: Option<u64>,
    /// Wire number, assigned by the numbering pass.
    pub number: Option<String>,
}

impl Wire {
    pub fn new(id: u64, from: u64, to: u64) -> Self {
        Self {
            id,
            from,
            to: Some(to),
            number: None,
        }
    }

    pub fn dangling(id: u64, from: u64) -> Self {
        Self {
            id,
            from,
            to: None,
            number: None,
        }
    }
}

/// A net: every port reachable from one another through wires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Net {
    /// Lowest port id in the net, used as its stable identity.
    pub id: u64,
    /// Member port ids, ascending.
    pub ports: Vec<u64>,
    /// Wire ids in the net, ascending.
    pub wires: Vec<u64>,
}

/// Union-find over port ids.
struct DisjointSet {
    parent: BTreeMap<u64, u64>,
}

impl DisjointSet {
    fn new() -> Self {
        Self {
            parent: BTreeMap::new(),
        }
    }

    fn find(&mut self, x: u64) -> u64 {
        let parent = *self.parent.entry(x).or_insert(x);
        if parent == x {
            return x;
        }
        let root = self.find(parent);
        self.parent.insert(x, root);
        root
    }

    fn union(&mut self, a: u64, b: u64) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            // Keep the lower id as the root so net ids are deterministic.
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.parent.insert(hi, lo);
        }
    }
}

/// Build nets from wires.
///
/// Only ports that appear in at least one wire become part of a net: a symbol with an
/// unconnected terminal is not a net of one, it is an unconnected terminal, and conflating
/// the two would fill the wire list with noise.
pub fn build_nets(wires: &[Wire]) -> Vec<Net> {
    let mut dsu = DisjointSet::new();
    for wire in wires {
        if let Some(to) = wire.to {
            if wire.from == to {
                // A wire from a port to itself carries no connectivity.
                continue;
            }
            dsu.union(wire.from, to);
        }
    }

    let mut by_root: BTreeMap<u64, Net> = BTreeMap::new();
    for wire in wires {
        let Some(to) = wire.to else { continue };
        if wire.from == to {
            continue;
        }
        let root = dsu.find(wire.from);
        let net = by_root.entry(root).or_insert_with(|| Net {
            id: root,
            ports: Vec::new(),
            wires: Vec::new(),
        });
        net.ports.push(wire.from);
        net.ports.push(to);
        net.wires.push(wire.id);
    }

    let mut nets: Vec<Net> = by_root.into_values().collect();
    for net in &mut nets {
        net.ports.sort_unstable();
        net.ports.dedup();
        net.wires.sort_unstable();
        net.wires.dedup();
        // The lowest port id is the net identity; keep it consistent after sorting.
        net.id = net.ports.first().copied().unwrap_or(net.id);
    }
    nets.sort_by_key(|n| n.id);
    nets
}

/// One line of the wire list report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireRow {
    pub wire: u64,
    pub number: Option<String>,
    pub net: u64,
    pub from_port: u64,
    pub to_port: u64,
}

/// Project wires into wire-list rows, ordered by wire id, skipping dangling wires.
pub fn wire_list(wires: &[Wire], nets: &[Net]) -> Vec<WireRow> {
    let mut net_of_port: BTreeMap<u64, u64> = BTreeMap::new();
    for net in nets {
        for port in &net.ports {
            net_of_port.insert(*port, net.id);
        }
    }
    let mut rows = Vec::new();
    for wire in wires {
        let Some(to) = wire.to else { continue };
        if wire.from == to {
            continue;
        }
        let Some(net) = net_of_port.get(&wire.from).copied() else {
            continue;
        };
        rows.push(WireRow {
            wire: wire.id,
            number: wire.number.clone(),
            net,
            from_port: wire.from,
            to_port: to,
        });
    }
    rows.sort_by_key(|r| r.wire);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> Vec<Wire> {
        vec![
            Wire::new(1, 10, 11),
            Wire::new(2, 11, 12),
            Wire::new(3, 12, 13),
        ]
    }

    #[test]
    fn a_chain_is_one_net() {
        let nets = build_nets(&chain());
        assert_eq!(nets.len(), 1);
        assert_eq!(nets[0].ports, vec![10, 11, 12, 13]);
        assert_eq!(nets[0].wires, vec![1, 2, 3]);
        assert_eq!(nets[0].id, 10);
    }

    #[test]
    fn separate_wires_are_separate_nets() {
        let wires = vec![Wire::new(1, 10, 11), Wire::new(2, 20, 21)];
        let nets = build_nets(&wires);
        assert_eq!(nets.len(), 2);
        assert_eq!(nets[0].ports, vec![10, 11]);
        assert_eq!(nets[1].ports, vec![20, 21]);
    }

    #[test]
    fn branches_merge_into_the_same_net() {
        // 10-11, 11-12 and 11-13: a T junction.
        let wires = vec![
            Wire::new(1, 10, 11),
            Wire::new(2, 11, 12),
            Wire::new(3, 11, 13),
        ];
        let nets = build_nets(&wires);
        assert_eq!(nets.len(), 1);
        assert_eq!(nets[0].ports, vec![10, 11, 12, 13]);
    }

    #[test]
    fn a_cycle_does_not_duplicate_members() {
        let wires = vec![
            Wire::new(1, 10, 11),
            Wire::new(2, 11, 12),
            Wire::new(3, 12, 10),
        ];
        let nets = build_nets(&wires);
        assert_eq!(nets.len(), 1);
        assert_eq!(nets[0].ports, vec![10, 11, 12]);
    }

    #[test]
    fn dangling_wires_form_no_net() {
        let wires = vec![Wire::dangling(1, 10), Wire::new(2, 20, 21)];
        let nets = build_nets(&wires);
        assert_eq!(nets.len(), 1);
        assert_eq!(nets[0].ports, vec![20, 21]);
    }

    #[test]
    fn self_connection_is_ignored() {
        let wires = vec![Wire::new(1, 10, 10)];
        assert!(build_nets(&wires).is_empty());
    }

    #[test]
    fn duplicate_wires_are_collapsed_in_the_wire_list_of_a_net() {
        let wires = vec![Wire::new(1, 10, 11), Wire::new(1, 10, 11)];
        let nets = build_nets(&wires);
        assert_eq!(nets[0].wires, vec![1]);
        assert_eq!(nets[0].ports, vec![10, 11]);
    }

    #[test]
    fn net_identity_is_independent_of_wire_order() {
        let a = build_nets(&[Wire::new(1, 30, 31), Wire::new(2, 31, 32)]);
        let b = build_nets(&[Wire::new(2, 32, 31), Wire::new(1, 31, 30)]);
        assert_eq!(a, b);
        assert_eq!(a[0].id, 30);
    }

    #[test]
    fn wire_list_reports_net_membership() {
        let wires = vec![Wire::new(1, 10, 11), Wire::new(2, 20, 21)];
        let nets = build_nets(&wires);
        let rows = wire_list(&wires, &nets);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].net, 10);
        assert_eq!(rows[1].net, 20);
        assert_eq!(rows[0].from_port, 10);
        assert_eq!(rows[0].to_port, 11);
    }

    #[test]
    fn wire_list_skips_dangling_wires() {
        let wires = vec![Wire::dangling(1, 10), Wire::new(2, 20, 21)];
        let nets = build_nets(&wires);
        let rows = wire_list(&wires, &nets);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].wire, 2);
    }

    #[test]
    fn wire_list_is_ordered_by_wire_id() {
        let wires = vec![Wire::new(7, 10, 11), Wire::new(3, 20, 21)];
        let nets = build_nets(&wires);
        let rows = wire_list(&wires, &nets);
        assert_eq!(rows.iter().map(|r| r.wire).collect::<Vec<_>>(), vec![3, 7]);
    }
}
