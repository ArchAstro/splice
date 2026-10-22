//! Platform-free core of Splice: the timeline model and its edits, undo, the
//! single-file project format, and export plans. The app crate owns windows,
//! decoding and sound.

pub mod bible;
pub mod brief;
pub mod captions;
pub mod compose;
pub mod export;
pub mod history;
pub mod leaks;
pub mod look;
pub mod model;
pub mod motion;
pub mod score;
pub mod store;
pub mod story;
pub mod symbol;
pub mod textmotion;
pub mod time;
pub mod transition;
pub mod zoom;
