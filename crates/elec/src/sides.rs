//! Terminal sides: what a terminal connects to on each of its two sides.
//!
//! A terminal is not a wire node. It is a *bridge*: the conductor arrives on one side and
//! leaves on the other, and the two sides are different nets. Treating it as a node - which
//! the geometric netlist does, because it only knows points - merges both circuits into one
//! net and hides the very thing a terminal strip report exists to say.
//!
//! This pass therefore treats the terminal as a barrier: the wires meeting at its point are
//! traversed *without* passing through that point, so each side of the terminal becomes its
//! own group of wires and devices.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::geometry::{Anchor, Point, WireSeg};

type Cell = (i64, i64);

fn cell(point: Point, tolerance: f64) -> Cell {
    let tol = if tolerance > 0.0 { tolerance } else { 1e-6 };
    (
        (point.x / tol).round() as i64,
        (point.y / tol).round() as i64,
    )
}

/// One side of a terminal: the wires and devices reachable from it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SideNet {
    pub wires: Vec<u64>,
    pub anchors: Vec<u64>,
    /// Grid cells in this side, used to decide whether two incident wires share a side.
    cells: BTreeSet<Cell>,
}

/// Both sides of one terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSides {
    pub terminal: u64,
    pub label: String,
    /// Up to two sides; empty when no wire reaches the terminal. A terminal with more than
    /// two groups reports every group rather than inventing a pairing.
    pub sides: Vec<SideNet>,
}

impl TerminalSides {
    /// True when the terminal has a conductor on both sides.
    pub fn is_through(&self) -> bool {
        self.sides.len() == 2
    }

    /// True when nothing reaches the terminal at all.
    pub fn is_unconnected(&self) -> bool {
        self.sides.is_empty()
    }
}

/// Compute the sides of each terminal.
///
/// `terminals` selects which anchors to analyse (usually the ones whose tag is
/// `strip:index`). The tolerance is the same one the geometric netlist uses.
pub fn terminal_sides(
    wires: &[WireSeg],
    anchors: &[Anchor],
    terminals: &[u64],
    tolerance: f64,
) -> Vec<TerminalSides> {
    // cell -> (wire id, the wire's other endpoint)
    let mut adjacency: BTreeMap<Cell, Vec<(u64, Cell)>> = BTreeMap::new();
    for wire in wires {
        let a = cell(wire.from, tolerance);
        let b = cell(wire.to, tolerance);
        adjacency.entry(a).or_default().push((wire.id, b));
        adjacency.entry(b).or_default().push((wire.id, a));
    }

    let mut out = Vec::new();
    for terminal_id in terminals {
        let Some(anchor) = anchors.iter().find(|anchor| anchor.id == *terminal_id) else {
            continue;
        };
        let centre = cell(anchor.at, tolerance);
        let incident: Vec<(u64, Cell)> = adjacency.get(&centre).cloned().unwrap_or_default();

        let mut sides: Vec<SideNet> = Vec::new();
        // Which side each cell belongs to, so two incident wires that reach the same
        // conductor end up on the same side instead of being split arbitrarily.
        let mut side_of: BTreeMap<Cell, usize> = BTreeMap::new();

        for (wire_id, far) in incident {
            let existing = side_of.get(&far).copied();
            let index = match existing {
                Some(index) => {
                    if !sides[index].wires.contains(&wire_id) {
                        sides[index].wires.push(wire_id);
                    }
                    index
                }
                None => {
                    let index = sides.len();
                    sides.push(SideNet::default());
                    // Walk from the far endpoint without crossing the terminal.
                    let mut queue: VecDeque<Cell> = VecDeque::new();
                    queue.push_back(far);
                    while let Some(current) = queue.pop_front() {
                        if current == centre || side_of.contains_key(&current) {
                            continue;
                        }
                        side_of.insert(current, index);
                        sides[index].cells.insert(current);
                        for (neighbour_wire, neighbour) in
                            adjacency.get(&current).cloned().unwrap_or_default()
                        {
                            if !sides[index].wires.contains(&neighbour_wire) {
                                sides[index].wires.push(neighbour_wire);
                            }
                            if neighbour != centre && !side_of.contains_key(&neighbour) {
                                queue.push_back(neighbour);
                            }
                        }
                    }
                    index
                }
            };
            if !sides[index].wires.contains(&wire_id) {
                sides[index].wires.push(wire_id);
            }
        }

        for side in &mut sides {
            side.wires.sort_unstable();
            side.wires.dedup();
            side.anchors = anchors
                .iter()
                .filter(|anchor| anchor.id != *terminal_id)
                .filter(|anchor| side.cells.contains(&cell(anchor.at, tolerance)))
                .map(|anchor| anchor.id)
                .collect();
            side.anchors.sort_unstable();
            side.anchors.dedup();
        }
        sides.sort_by_key(|side| side.wires.first().copied().unwrap_or(u64::MAX));

        out.push(TerminalSides {
            terminal: *terminal_id,
            label: anchor.label.clone(),
            sides,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    fn anchor(id: u64, label: &str, x: f64, y: f64) -> Anchor {
        Anchor::new(id, label, p(x, y))
    }

    #[test]
    fn a_straight_through_terminal_has_two_sides() {
        // -K1 --- X1:1 --- -K2   (terminal in the middle)
        let wires = vec![
            WireSeg::new(1, p(0.0, 0.0), p(50.0, 0.0)),
            WireSeg::new(2, p(50.0, 0.0), p(100.0, 0.0)),
        ];
        let anchors = vec![
            anchor(10, "-K1", 0.0, 0.0),
            anchor(11, "X1:1", 50.0, 0.0),
            anchor(12, "-K2", 100.0, 0.0),
        ];
        let result = terminal_sides(&wires, &anchors, &[11], 0.01);
        assert_eq!(result.len(), 1);
        assert!(result[0].is_through());
        assert_eq!(result[0].sides.len(), 2);
        assert_eq!(result[0].sides[0].anchors, vec![10]);
        assert_eq!(result[0].sides[1].anchors, vec![12]);
        assert_eq!(result[0].sides[0].wires, vec![1]);
        assert_eq!(result[0].sides[1].wires, vec![2]);
    }

    #[test]
    fn a_terminal_with_one_wire_has_one_side() {
        let wires = vec![WireSeg::new(1, p(0.0, 0.0), p(50.0, 0.0))];
        let anchors = vec![anchor(10, "-K1", 0.0, 0.0), anchor(11, "X1:1", 50.0, 0.0)];
        let result = terminal_sides(&wires, &anchors, &[11], 0.01);
        assert_eq!(result[0].sides.len(), 1);
        assert!(!result[0].is_through());
        assert_eq!(result[0].sides[0].anchors, vec![10]);
    }

    #[test]
    fn a_terminal_with_no_wires_is_unconnected() {
        let anchors = vec![anchor(11, "X1:1", 50.0, 0.0)];
        let result = terminal_sides(&[], &anchors, &[11], 0.01);
        assert!(result[0].is_unconnected());
    }

    #[test]
    fn both_wires_on_one_side_form_a_single_group() {
        // Two devices wired to the same side of the terminal: -K1 and -K2 both reach the
        // terminal from the left, through a shared point.
        let wires = vec![
            WireSeg::new(1, p(0.0, 0.0), p(50.0, 0.0)),
            WireSeg::new(2, p(0.0, 0.0), p(0.0, 20.0)),
            WireSeg::new(3, p(50.0, 0.0), p(100.0, 0.0)),
        ];
        let anchors = vec![
            anchor(10, "-K1", 0.0, 20.0),
            anchor(11, "X1:1", 50.0, 0.0),
            anchor(12, "-K2", 100.0, 0.0),
        ];
        let result = terminal_sides(&wires, &anchors, &[11], 0.01);
        assert_eq!(result[0].sides.len(), 2);
        // The side holding -K1 carries both of its wires.
        let left = result[0]
            .sides
            .iter()
            .find(|side| side.anchors.contains(&10))
            .expect("side with -K1");
        assert_eq!(left.wires, vec![1, 2]);
        let right = result[0]
            .sides
            .iter()
            .find(|side| side.anchors.contains(&12))
            .expect("side with -K2");
        assert_eq!(right.wires, vec![3]);
    }

    #[test]
    fn a_chain_reaches_the_far_device() {
        // X1:1 --- a --- b --- -K9
        let wires = vec![
            WireSeg::new(1, p(50.0, 0.0), p(60.0, 0.0)),
            WireSeg::new(2, p(60.0, 0.0), p(70.0, 0.0)),
            WireSeg::new(3, p(70.0, 0.0), p(80.0, 0.0)),
        ];
        let anchors = vec![anchor(11, "X1:1", 50.0, 0.0), anchor(12, "-K9", 80.0, 0.0)];
        let result = terminal_sides(&wires, &anchors, &[11], 0.01);
        assert_eq!(result[0].sides.len(), 1);
        assert_eq!(result[0].sides[0].anchors, vec![12]);
        assert_eq!(result[0].sides[0].wires, vec![1, 2, 3]);
    }

    #[test]
    fn the_terminal_itself_is_not_one_of_its_peers() {
        let wires = vec![WireSeg::new(1, p(0.0, 0.0), p(50.0, 0.0))];
        let anchors = vec![anchor(10, "-K1", 0.0, 0.0), anchor(11, "X1:1", 50.0, 0.0)];
        let result = terminal_sides(&wires, &anchors, &[11], 0.01);
        for side in &result[0].sides {
            assert!(!side.anchors.contains(&11));
        }
    }

    #[test]
    fn several_terminals_are_reported_independently() {
        let wires = vec![
            WireSeg::new(1, p(0.0, 0.0), p(50.0, 0.0)),
            WireSeg::new(2, p(50.0, 0.0), p(100.0, 0.0)),
            WireSeg::new(3, p(200.0, 0.0), p(250.0, 0.0)),
        ];
        let anchors = vec![
            anchor(10, "-K1", 0.0, 0.0),
            anchor(11, "X1:1", 50.0, 0.0),
            anchor(12, "-K2", 100.0, 0.0),
            anchor(21, "X2:1", 200.0, 0.0),
        ];
        let result = terminal_sides(&wires, &anchors, &[11, 21], 0.01);
        assert_eq!(result.len(), 2);
        assert!(result[0].is_through());
        assert_eq!(result[1].sides.len(), 1);
    }
}
