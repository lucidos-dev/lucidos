/// The most ids one press may carry. A Current section past this is a
/// workspace to triage in passes, not a request to accept unbounded.
pub const MAX_IDS: usize = 2_000;

/// The other inbox threads of a kept family.
pub const SAME_FAMILY: &str = "same_family";

/// A pinned sub-thread the archive of its safe root leaves open.
pub const PINNED_SUB_THREAD: &str = "pinned_sub_thread";
