//! Protocol types and judgement rules shared by `skym` and `skym-server`.
//!
//! Pure: no I/O, no async runtime.

#[cfg(feature = "fixtures")]
pub mod fixtures;
pub mod judge;
pub mod model;
pub mod report;
pub mod rules;
pub mod subject;
pub mod time;
pub mod view;
