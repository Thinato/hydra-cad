//! Hydra domain model: catalogue rows, symbol definitions, circuits and their validation.
//!
//! No UI or database code lives here, so it can be tested alone and reused.

mod bom;
mod catalogue;
mod circuit;
mod edit;
mod import;
mod proposal;
mod status;
mod symbol;

pub use bom::{BomLine, bom_csv};
pub use catalogue::{
    Block, Catalogue, CatalogueRef, Manufacturer, MountFace, Part, Ratings, SymbolDefRecord,
    ValveFunction, ValveType, key_problem,
};
pub use circuit::{
    Assembly, Circuit, Component, ComponentKind, ExtraBomLine, Import, Issue, Item, Layout,
    LoadError, Net, PORT_PIN, Placement, PortRef, Requested, Rotation, SCHEMA_VERSION, Severity,
    Valve,
};
pub use edit::{Command, Document, WireEnd, next_id};
pub use import::{
    MAX_CIRCUIT_NAME, MAX_IMPORT_COMPONENTS, MAX_IMPORT_EXTRA_LINES, MAX_IMPORT_TEXT,
    MAX_IMPORT_WIRE_POINTS, MAX_LISTED, MAX_SHEET_MM, MAX_WIRE_POINTS, MAX_WIRES_PER_NET,
};
pub use proposal::{MAX_PROPOSAL_NOTES, MAX_PROPOSAL_PARTS, PartDraft, Proposal};
pub use status::{Available, CircuitStatus, NoStandIn, StandIn, ValveStatus, stand_ins};
pub use symbol::{Actuator, DirectionalValve, FlowActuator, Port, Side, SpoolPosition, SymbolDef};
