//! Electrical (ECAD) ribbon tab.
//!
//! The tab is the entry point for the electrical layer: component tools, numbering and
//! the reports that make an electrical drawing worth drawing. Tools dispatch the same
//! commands the command line uses, so every feature is scriptable.

pub mod place;
pub mod wire;

use crate::modules::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

pub struct ElectricalModule;

impl CadModule for ElectricalModule {
    fn id(&self) -> &'static str {
        "electrical"
    }

    fn title(&self) -> &'static str {
        "Electrical"
    }

    fn ribbon_groups(&self) -> &[RibbonGroup] {
        static GROUPS: std::sync::OnceLock<Vec<RibbonGroup>> = std::sync::OnceLock::new();
        GROUPS.get_or_init(|| {
            vec![
                RibbonGroup {
                    title: "Numbering",
                    tools: vec![
                        RibbonItem::LargeTool(ToolDef {
                            id: "ELECNUMBER",
                            label: "Number\nDevices",
                            icon: IconKind::Glyph("\u{2116}"),
                            event: ModuleEvent::Command("ELECNUMBER".to_string()),
                        }),
                        RibbonItem::Tool(ToolDef {
                            id: "ELECNUMBERPREVIEW",
                            label: "Preview",
                            icon: IconKind::Glyph("\u{1F50D}"),
                            event: ModuleEvent::Command("ELECNUMBERPREVIEW".to_string()),
                        }),
                    ],
                },
                RibbonGroup {
                    title: "Components",
                    tools: vec![
                        RibbonItem::LargeTool(ToolDef {
                            id: "ELECLIST",
                            label: "Components",
                            icon: IconKind::Glyph("\u{26A1}"),
                            event: ModuleEvent::Command("ELECLIST".to_string()),
                        }),
                        RibbonItem::LargeTool(ToolDef {
                            id: "ELECPALETTE",
                            label: "Library",
                            icon: IconKind::Glyph("\u{1F4D6}"),
                            event: ModuleEvent::Command("ELECPALETTE".to_string()),
                        }),
                        RibbonItem::Tool(ToolDef {
                            id: "ELECINSERT",
                            label: "Insert\nSymbol",
                            icon: IconKind::Glyph("\\u{2795}"),
                            event: ModuleEvent::Command("ELECINSERT".to_string()),
                        }),
                        RibbonItem::Tool(ToolDef {
                            id: "ATTDEF",
                            label: "Attribute\nDefinition",
                            icon: IconKind::Glyph("\u{0040}"),
                            event: ModuleEvent::Command("ATTDEF".to_string()),
                        }),
                        RibbonItem::Tool(ToolDef {
                            id: "ATTEDIT",
                            label: "Edit\nAttributes",
                            icon: IconKind::Glyph("\u{270E}"),
                            event: ModuleEvent::Command("ATTEDIT".to_string()),
                        }),
                    ],
                },
                RibbonGroup {
                    title: "Reports",
                    tools: vec![
                        RibbonItem::LargeTool(ToolDef {
                            id: "ELECBOM",
                            label: "Bill of\nMaterial",
                            icon: IconKind::Glyph("\u{2637}"),
                            event: ModuleEvent::Command("ELECBOM".to_string()),
                        }),
                        RibbonItem::LargeTool(ToolDef {
                            id: "ELECWIRELIST",
                            label: "Wire\nList",
                            icon: IconKind::Glyph("\u{1F5C7}"),
                            event: ModuleEvent::Command("ELECWIRELIST".to_string()),
                        }),
                        RibbonItem::LargeTool(ToolDef {
                            id: "ELECWIRENUMBER",
                            label: "Number\nWires",
                            icon: IconKind::Glyph("\u{2318}"),
                            event: ModuleEvent::Command("ELECWIRENUMBER".to_string()),
                        }),
                        RibbonItem::LargeTool(ToolDef {
                            id: "ELECTERMINALS",
                            label: "Terminal\nStrips",
                            icon: IconKind::Glyph("\u{2393}"),
                            event: ModuleEvent::Command("ELECTERMINALS".to_string()),
                        }),
                        RibbonItem::Tool(ToolDef {
                            id: "ELECTSIDES",
                            label: "Terminal\nSides",
                            icon: IconKind::Glyph("\u{21C6}"),
                            event: ModuleEvent::Command("ELECTSIDES".to_string()),
                        }),
                        RibbonItem::LargeTool(ToolDef {
                            id: "ELECXREF",
                            label: "Cross\nReference",
                            icon: IconKind::Glyph("\u{21C4}"),
                            event: ModuleEvent::Command("ELECXREF".to_string()),
                        }),
                        RibbonItem::Tool(ToolDef {
                            id: "ELECXREFWRITE",
                            label: "Write\nXrefs",
                            icon: IconKind::Glyph("\u{270E}"),
                            event: ModuleEvent::Command("ELECXREFWRITE".to_string()),
                        }),
                    ],
                },
            ]
        })
    }
}





