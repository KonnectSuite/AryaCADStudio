//! Click-to-place for library symbols.
//!
//! The palette can only say *what* to insert; this command asks *where*. Everything that
//! needs the app - the library's attribute template, the next free tag, the catalogue -
//! is resolved before the command starts, so the command itself stays a plain value that
//! commits one entity when a point is picked.

use codec::types::Vector3;
use codec::EntityType;
use glam::DVec3;

use crate::command::{CadCommand, CmdResult};
use crate::t;

/// One attribute to write on the new instance: tag and value.
#[derive(Debug, Clone)]
pub struct PlacedAttribute {
    pub tag: String,
    pub value: String,
}

pub struct PlaceSymbolCommand {
    block: String,
    attributes: Vec<PlacedAttribute>,
    /// Text height for the attributes, taken from the symbol size.
    height: f64,
    /// Uniform scale the symbol is drawn at on the sheet.
    scale: f64,
    /// Set once a point has been picked, so a second click places another instance - the
    /// same behaviour as INSERT, and what a user expects when placing a row of contacts.
    placed: usize,
}

impl PlaceSymbolCommand {
    pub fn new(
        block: String,
        attributes: Vec<PlacedAttribute>,
        height: f64,
        scale: f64,
    ) -> Self {
        Self {
            block,
            attributes,
            height,
            scale,
            placed: 0,
        }
    }

    fn build(&self, at: DVec3) -> EntityType {
        let position = Vector3::new(at.x, at.y, at.z);
        let insert = codec::entities::Insert::new(self.block.clone(), position)
            .with_uniform_scale(self.scale);
        let mut insert = insert;
        let step = self.height * 1.25;
        for (row, attribute) in self.attributes.iter().enumerate() {
            let mut entity =
                codec::entities::AttributeEntity::new(attribute.tag.clone(), attribute.value.clone());
            entity.height = self.height;
            entity.insertion_point =
                Vector3::new(at.x, at.y - self.height - row as f64 * step, at.z);
            entity.alignment_point = entity.insertion_point;
            insert.attributes.push(entity);
        }
        EntityType::Insert(insert)
    }
}

impl CadCommand for PlaceSymbolCommand {
    fn name(&self) -> &'static str {
        "ELECPLACE"
    }

    fn prompt(&self) -> String {
        if self.placed == 0 {
            format!("{}  Specify insertion point:", self.block)
        } else {
            format!(
                "{} placed. Specify insertion point for another  [Enter to finish]:",
                self.block
            )
        }
    }

    fn on_point(&mut self, point: DVec3) -> CmdResult {
        self.placed += 1;
        CmdResult::CommitAndExit(self.build(point))
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Done
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pick_commits_an_insert_with_its_attributes() {
        let mut command = PlaceSymbolCommand::new(
            "HCR1".to_string(),
            vec![
                PlacedAttribute {
                    tag: "TAG1".to_string(),
                    value: "-K7".to_string(),
                },
                PlacedAttribute {
                    tag: "CAT".to_string(),
                    value: "3RT".to_string(),
                },
            ],
        );
        match command.on_point(DVec3::new(10.0, 20.0, 0.0)) {
            CmdResult::CommitAndExit(EntityType::Insert(insert)) => {
                assert_eq!(insert.block_name, "HCR1");
                assert_eq!(insert.insert_point.x, 10.0);
                assert_eq!(insert.insert_point.y, 20.0);
                assert_eq!(insert.attributes.len(), 2);
                assert_eq!(insert.attributes[0].tag, "TAG1");
                assert_eq!(insert.attributes[0].value, "-K7");
                assert_eq!(insert.attributes[1].tag, "CAT");
            }
            other => panic!("expected an insert commit, got {other:?}"),
        }
        assert!(command.prompt().contains("placed"));
    }

    #[test]
    fn finishing_on_enter_leaves_the_command() {
        let mut command = PlaceSymbolCommand::new("X".to_string(), Vec::new());
        assert_eq!(command.on_enter(), CmdResult::Finished);

    }
}





