/// SSE keep-alive cadence. Clients derive their stall threshold from it
/// (`generated/engine-constants`), so a stream silent for several intervals
/// reads as dropped.
pub const SSE_KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Name of the keep-alive SSE event. A named event reaches only a listener
/// registered for it, so `onmessage` consumers never see it (ADR 0146).
pub const SSE_KEEPALIVE_EVENT: &str = "keepalive";

/// The frame the SSE route writes when a subscriber falls behind.
pub const LAGGED_FRAME: &str = "Lagged";

/// Every frame type the SSE route writes itself rather than relays. A domain
/// event may not take one of these names, or any app could make every client
/// act on a frame it forged.
pub(crate) const ROUTE_FRAMES: &[&str] = &[LAGGED_FRAME];
