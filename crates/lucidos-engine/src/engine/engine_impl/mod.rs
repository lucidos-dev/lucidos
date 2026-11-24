//! `LucidosEngine` method definitions, split by domain into child modules.
//!
//! Each child holds its own `impl LucidosEngine` block; no re-exports are
//! needed because the methods are inherent on `LucidosEngine` and therefore
//! visible crate-wide regardless of which module defines the block. Struct
//! fields, `pub mod` declarations, and free-function helpers stay in the
//! engine `mod.rs`.

mod accessors;
pub use accessors::release_change_claim;
mod construction;
mod domain_events;
mod ordinary_submission;
mod scripts;
mod shutdown;
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub use shutdown::emit_teardown_abort_unless_question_parked;
mod threads;
mod trigger_runs;

pub use {scripts::mask_stored_events, scripts::stored_secret_values, scripts::ScriptEnv};
