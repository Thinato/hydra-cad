//! Hydra domain model: catalogue rows, symbol definitions, circuits and their validation.
//!
//! No UI or database code lives here, so it can be tested alone and reused.

mod catalogue;
mod circuit;
mod symbol;

pub use catalogue::{Catalogue, Manufacturer, SymbolDefRecord, ValveType};
pub use circuit::{
    Circuit, Component, ComponentKind, Issue, Layout, LoadError, Net, PORT_PIN, Placement, PortRef,
    Rotation, SCHEMA_VERSION, Severity,
};
pub use symbol::{Actuator, DirectionalValve, Port, Side, SpoolPosition, SymbolDef};
