//! `LucidosEngine` method definitions, split by domain into child modules.
//!
//! Each child holds its own `impl LucidosEngine` block; no re-exports are
//! needed because the methods are inherent on `LucidosEngine` and therefore
//! visible crate-wide regardless of which module defines the block. Struct
//! fields, `pub mod` declarations, and free-function helpers stay in the
//! engine `mod.rs`.

mod accessors;
mod construction;
mod domain_events;
mod scripts;
mod shutdown;
mod threads;
mod trigger_runs;
