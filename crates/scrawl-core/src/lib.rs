//! Scrawl's bundle compiler: picks keyframes, describes and draws ink, crops to the active
//! region, and renders the contact sheet and `summary.md` from a capture's frames, strokes,
//! events and transcript. No OS calls, config file or clock (`docs/library.md`); the app in the
//! repository root records captures and passes them in.

pub use scrawl_bundle as model;

pub mod agent_context;
pub mod autocrop;
pub mod build;
pub mod keyframes;
pub mod page;
pub mod render;
pub mod sketch;
