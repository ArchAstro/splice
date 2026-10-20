//! Platform-free core of Splice: the timeline model and its edits, undo, the
//! single-file project format, and export plans. The app crate owns windows,
//! decoding and sound.

pub mod export;
pub mod history;
pub mod look;
pub mod model;
pub mod score;
pub mod store;
pub mod time;
