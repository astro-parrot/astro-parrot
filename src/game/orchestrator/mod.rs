//! Orchestrator subsystem: galaxy topology, request/ack communication, the
//! command-driven core, and the events it emits for the GUI.

mod comm;
mod core;
mod events;
mod factory;
mod galaxy;

pub use core::{Command, Orchestrator};
pub use events::GuiEvent;
