/// Ceiling on `timeout_secs`. There is no unbounded wait: a wait that outlives
/// every reason anyone had for it is indistinguishable from a stalled thread.
pub const MAX_TIMEOUT_SECS: i64 = 24 * 60 * 60;

/// How many counted waits a thread may start inside
/// [`RECENT_SUBSCRIPTION_WINDOW_SECS`] with no human action in between (S8).
/// `recent_subscriptions` says which waits count. Mirrors
/// `max_event_trigger_depth` in intent: the events still persist, the fan-out
/// just stops.
///
/// Twenty, not ten: a thread's own background tasks still count, and a thread
/// working through short ones reached ten in an hour. A hot loop re-arms in
/// seconds, so it reaches twenty within a minute or two anyway (ADR 0280).
pub const MAX_RECENT_SUBSCRIPTIONS: i64 = 20;

/// The rolling window [`MAX_RECENT_SUBSCRIPTIONS`] counts over. The cap bounds a
/// loop, and every loop it names re-arms within seconds. A serial workflow of
/// half-hour waits does not. A lifetime count cannot tell the two apart, so it
/// starved every thread with no human in it (ADR 0265).
pub const RECENT_SUBSCRIPTION_WINDOW_SECS: i64 = 60 * 60;
