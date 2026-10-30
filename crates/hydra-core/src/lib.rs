//! Hydra domain model: catalogue rows, symbol definitions, circuits and their validation.
//!
//! No UI or database code lives here, so it can be tested alone and reused.

mod catalogue;
mod circuit;
mod edit;
mod proposal;
mod symbol;

pub use catalogue::{
    Block, Catalogue, CatalogueRef, Manufacturer, MountFace, Part, SymbolDefRecord, ValveType,
    key_problem,
};
pub use circuit::{
    Assembly, Circuit, Component, ComponentKind, Issue, Layout, LoadError, Net, PORT_PIN,
    Placement, PortRef, Rotation, SCHEMA_VERSION, Severity,
};
pub use edit::{Command, Document, next_id};
pub use proposal::{MAX_PROPOSAL_NOTES, MAX_PROPOSAL_PARTS, PartDraft, Proposal};
pub use symbol::{Actuator, DirectionalValve, Port, Side, SpoolPosition, SymbolDef};
