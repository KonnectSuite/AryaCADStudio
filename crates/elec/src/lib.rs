//! AryaCAD Electrical Ã¢â‚¬â€ the ECAD domain core.
//!
//! This crate holds the parts of an electrical CAD that are independent of the drawing
//! host: how devices and wires are numbered, how label templates are parsed and rendered,
//! and how placed components roll up into a bill of material. It is deliberately free of
//! document, UI and file-format dependencies so the same logic serves the in-app module,
//! report exports and command-line tools.
//!
//! The behaviour is modelled on two shipped ECAD systems that were reverse engineered for
//! this project: SkyCAD Electrical's numbering formats and label language, and the drawing
//! setup an AutoCAD Electrical sheet stores in its `WD_M` block.

pub mod catalogue;
pub mod component;
pub mod expr;
pub mod geometry;
pub mod junction;
pub mod netlist;
pub mod numbering;
pub mod report;
pub mod sides;
pub mod terminal;
pub mod xref;

pub use catalogue::{Catalogue, CatalogueError, PartRecord};
pub use component::{bill_of_materials, BomRow, Component};
pub use geometry::{netlist_from_geometry, wire_rows, Anchor, GeoNet, GeoNetlist, GeoWireRow, Point, WireSeg};
pub use junction::junction_points;
pub use netlist::{build_nets, wire_list, Net, Port, Wire, WireRow};
pub use sides::{terminal_sides, SideNet, TerminalSides};
pub use report::{bom_sheet, terminal_sheet, wire_sheet, wire_sheet_from, Cell, Sheet};
pub use expr::{Branch, Compare, Conditional, Join, Part, Ref, Resolve, Step, Template, Value};
pub use xref::{cross_references, multi_part, DeviceRef, PartRef, PlacedPart};
pub use terminal::{parse_terminal_tag, terminal_strips, StripTerminal, TerminalRef, TerminalStrip};
pub use numbering::{
    default_prefix, infer_tag_prefix, next_tag, reading_order_key, renumber, Alphabet, Kind, Numbered,
    NumberingFormat, Renumbered, Uniqueness,
};









