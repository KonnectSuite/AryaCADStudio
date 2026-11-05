//! The wire tool: draw conductors, not decoration.
//!
//! A wire here is a line on the `WIRES` layer with a net number derived afterwards. The tool
//! exists separately from `LINE` because a schematic wire is drawn differently: it chains
//! from the last point, it lands on the wire layer whatever the current layer is, and its
//! endpoints are what junction detection and the netlist read.

use codec::types::Vector3;
use codec::{EntityType, Line};
use glam::DVec3;

use crate::command::{CadCommand, CmdResult};
use crate::t;

/// Layer conductors are drawn on.
pub const WIRE_LAYER: &str = "WIRES";

pub struct WireCommand {
    last: Option<DVec3>,
    /// Wires committed in this run, for the closing message.
    segments: usize,
}

impl WireCommand {
    pub fn new() -> Self {
        Self {
            last: None,
            segments: 0,
        }
    }

    fn segment(from: DVec3, to: DVec3) -> EntityType {
        let mut line = Line::from_points(
            Vector3::new(from.x, from.y, from.z),
            Vector3::new(to.x, to.y, to.z),
        );
        line.common.layer = WIRE_LAYER.to_string();
        EntityType::Line(line)
    }
}

impl Default for WireCommand {
    fn default() -> Self {
        Self::new()
    }
}

impl CadCommand for WireCommand {
    fn name(&self) -> &'static str {
        "ELECWIRE"
    }

    fn prompt(&self) -> String {
        match self.last {
            None => t!("ELECWIRE  Specify start point:").into_owned(),
            Some(_) => t!("ELECWIRE  Specify next point  [Enter to finish]:").into_owned(),
        }
    }

    fn on_point(&mut self, point: DVec3) -> CmdResult {
        match self.last {
            None => {
                self.last = Some(point);
                CmdResult::NeedPoint
            }
            Some(from) => {
                self.last = Some(point);
                self.segments += 1;
                CmdResult::CommitEntity(Self::segment(from, point))
            }
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_pick_only_starts_the_wire() {
        let mut command = WireCommand::new();
        assert_eq!(
            command.on_point(DVec3::new(0.0, 0.0, 0.0)),
            CmdResult::NeedPoint
        );
        assert!(command.prompt().contains("next point"));
    }

    #[test]
    fn each_further_pick_commits_a_wire_on_the_wire_layer() {
        let mut command = WireCommand::new();
        command.on_point(DVec3::new(0.0, 0.0, 0.0));
        match command.on_point(DVec3::new(10.0, 0.0, 0.0)) {
            CmdResult::CommitEntity(EntityType::Line(line)) => {
                assert_eq!(line.common.layer, WIRE_LAYER);
                assert_eq!(line.start.x, 0.0);
                assert_eq!(line.end.x, 10.0);
            }
            other => panic!("expected a line commit, got {other:?}"),
        }
        // The next segment starts where the last one ended.
        match command.on_point(DVec3::new(10.0, 10.0, 0.0)) {
            CmdResult::CommitEntity(EntityType::Line(line)) => {
                assert_eq!(line.start.x, 10.0);
                assert_eq!(line.start.y, 0.0);
                assert_eq!(line.end.y, 10.0);
            }
            other => panic!("expected a chained line, got {other:?}"),
        }
        assert_eq!(command.on_enter(), CmdResult::Done);
    }
}
