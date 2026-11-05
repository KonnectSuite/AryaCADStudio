//! Junctions: where wires actually meet.
//!
//! Two wires that touch at their ends are one conductor and need no dot. Three or more
//! meeting at a point, or a wire end landing in the middle of another wire, must be drawn
//! with a junction - on a schematic the dot is the difference between "connected" and
//! "crossing", and an electrician reads it that way.
//!
//! Crossing wires without a shared point are *not* connected, so they get no dot.

use std::collections::BTreeMap;

use crate::geometry::{Point, WireSeg};

type Cell = (i64, i64);

fn cell(point: Point, tolerance: f64) -> Cell {
    let tol = if tolerance > 0.0 { tolerance } else { 1e-6 };
    (
        (point.x / tol).round() as i64,
        (point.y / tol).round() as i64,
    )
}

/// Points that need a junction dot.
///
/// A point qualifies when three or more wire ends meet there, or when a wire end lies on
/// the interior of another wire. A plain corner or an end-to-end join does not.
pub fn junction_points(wires: &[WireSeg], tolerance: f64) -> Vec<Point> {
    // How many wire ends land on each point.
    let mut ends: BTreeMap<Cell, (Point, usize)> = BTreeMap::new();
    for wire in wires {
        for endpoint in [wire.from, wire.to] {
            let entry = ends.entry(cell(endpoint, tolerance)).or_insert((endpoint, 0));
            entry.1 += 1;
        }
    }

    let mut junctions: BTreeMap<Cell, Point> = BTreeMap::new();
    for (key, (point, count)) in &ends {
        if *count >= 3 {
            junctions.insert(*key, *point);
        }
    }

    // A wire ending in the middle of another wire is a T junction even though only two
    // ends meet there.
    let tol = if tolerance > 0.0 { tolerance } else { 1e-6 };
    for (key, (point, _)) in &ends {
        if junctions.contains_key(key) {
            continue;
        }
        for wire in wires {
            if !on_interior(*point, wire, tol) {
                continue;
            }
            junctions.insert(*key, *point);
            break;
        }
    }

    junctions.into_values().collect()
}

/// True when `point` lies on the segment but not at either end.
fn on_interior(point: Point, wire: &WireSeg, tolerance: f64) -> bool {
    let ax = wire.from.x;
    let ay = wire.from.y;
    let bx = wire.to.x;
    let by = wire.to.y;
    let length_squared = (bx - ax) * (bx - ax) + (by - ay) * (by - ay);
    if length_squared <= tolerance * tolerance {
        return false;
    }
    // Project onto the segment and measure the distance.
    let t = ((point.x - ax) * (bx - ax) + (point.y - ay) * (by - ay)) / length_squared;
    if !(0.0..=1.0).contains(&t) {
        return false;
    }
    let closest_x = ax + t * (bx - ax);
    let closest_y = ay + t * (by - ay);
    let distance = ((point.x - closest_x).powi(2) + (point.y - closest_y).powi(2)).sqrt();
    if distance > tolerance {
        return false;
    }
    // Exclude the endpoints: an end-to-end join is not a T junction.
    let at_a = (point.x - ax).abs() <= tolerance && (point.y - ay).abs() <= tolerance;
    let at_b = (point.x - bx).abs() <= tolerance && (point.y - by).abs() <= tolerance;
    !(at_a || at_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    fn wire(id: u64, ax: f64, ay: f64, bx: f64, by: f64) -> WireSeg {
        WireSeg::new(id, p(ax, ay), p(bx, by))
    }

    #[test]
    fn three_ends_at_a_point_need_a_junction() {
        let wires = vec![
            wire(1, 0.0, 0.0, 10.0, 0.0),
            wire(2, 10.0, 0.0, 20.0, 0.0),
            wire(3, 10.0, 0.0, 10.0, 10.0),
        ];
        let points = junction_points(&wires, 0.01);
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].x, 10.0);
        assert_eq!(points[0].y, 0.0);
    }

    #[test]
    fn two_wires_meeting_end_to_end_need_none() {
        let wires = vec![wire(1, 0.0, 0.0, 10.0, 0.0), wire(2, 10.0, 0.0, 20.0, 0.0)];
        assert!(junction_points(&wires, 0.01).is_empty());
    }

    #[test]
    fn a_corner_needs_none() {
        let wires = vec![wire(1, 0.0, 0.0, 10.0, 0.0), wire(2, 10.0, 0.0, 10.0, 10.0)];
        assert!(junction_points(&wires, 0.01).is_empty());
    }

    #[test]
    fn a_wire_ending_on_another_wire_is_a_t_junction() {
        let wires = vec![
            wire(1, 0.0, 0.0, 20.0, 0.0),
            wire(2, 10.0, 0.0, 10.0, 10.0),
        ];
        let points = junction_points(&wires, 0.01);
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].x, 10.0);
    }

    #[test]
    fn a_crossing_without_a_shared_point_is_not_connected() {
        // A vertical wire passing through the middle of a horizontal one, with no endpoint
        // there: the wires cross but do not meet, so no dot.
        let wires = vec![
            wire(1, 0.0, 0.0, 20.0, 0.0),
            wire(2, 10.0, -10.0, 10.0, 10.0),
        ];
        assert!(junction_points(&wires, 0.01).is_empty());
    }

    #[test]
    fn four_ends_at_a_point_are_one_junction() {
        let wires = vec![
            wire(1, 0.0, 0.0, 10.0, 0.0),
            wire(2, 20.0, 0.0, 10.0, 0.0),
            wire(3, 10.0, 10.0, 10.0, 0.0),
            wire(4, 10.0, -10.0, 10.0, 0.0),
        ];
        assert_eq!(junction_points(&wires, 0.01).len(), 1);
    }

    #[test]
    fn separate_junctions_are_reported_separately() {
        let wires = vec![
            wire(1, 0.0, 0.0, 10.0, 0.0),
            wire(2, 10.0, 0.0, 20.0, 0.0),
            wire(3, 10.0, 0.0, 10.0, 10.0),
            wire(4, 100.0, 0.0, 110.0, 0.0),
            wire(5, 110.0, 0.0, 120.0, 0.0),
            wire(6, 110.0, 0.0, 110.0, 10.0),
        ];
        assert_eq!(junction_points(&wires, 0.01).len(), 2);
    }

    #[test]
    fn no_wires_no_junctions() {
        assert!(junction_points(&[], 0.01).is_empty());
    }
}
