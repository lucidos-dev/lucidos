/// INVARIANT: SSE keep-alive cadence. Clients derive their stall threshold from it
/// (`generated/engine-constants`), so a stream silent for several intervals
/// reads as dropped.
pub const SSE_KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

pub const SSE_KEEPALIVE_EVENT: &str = "keepalive";

pub const LAGGED_FRAME: &str = "Lagged";

/// INVARIANT: Every frame type the SSE route writes itself rather than relays. A domain
/// event may not take one of these names, or any app could make every client
/// act on a frame it forged.
pub(crate) const ROUTE_FRAMES: &[&str] = &[LAGGED_FRAME];
