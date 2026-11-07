//! Domain model and rules for the D&D table.
//!
//! Port of the JS reference in `E:\DnD`: `server/state.js`,
//! `renderer/dnd5e.js`, `renderer/dice.js`.
//!
//! Normalizers take and return `serde_json::Value` on purpose: the JS code
//! normalizes plain objects and the same JSON goes to the renderer, so keeping
//! the boundary untyped matches the reference exactly. Typed views can be added
//! later where the server needs structure.

pub mod campaign;
pub mod content;
pub mod dice;
pub mod media;
pub mod model;
pub mod room;
pub mod rules;
pub mod session_log;
pub mod sheet;
