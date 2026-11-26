pub const MAX_SECS: u64 = 600;

pub const MIN_SECS: u64 = 1;

/// INVARIANT: How long a client waits on one proxied call: [`MAX_SECS`] plus a margin.
/// Every pipeline step and request is bounded by the cap, so the margin covers
/// only the work before the budget starts. A client deadline below the cap
/// would cut a call the engine still means to finish.
///
/// The SDK reads it from the generated `engine-constants.ts`. `lucidos proxy`
/// cannot import this crate, so a test pins its copy.
pub const CLIENT_WAIT_SECS: u64 = MAX_SECS + 60;
