use uuid::Uuid;

/// What an agent writes for "the thread I am in", instead of an id it has to go
/// and look up. Matched case-insensitively, after a trim.
///
/// Private on purpose: [`resolve_thread_id_arg`] is the only door. A second
/// reader matching this list inline is how one slot resolves the alias and the
/// next one only ignores it. That is the bug that shipped.
const CURRENT_THREAD_ALIASES: [&str; 2] = ["current", "this"];

/// Resolve a thread id an agent or a script wrote into the uuid it names.
///
/// `caller` is the thread the call runs in, for a surface that has one. That is
/// what an alias resolves to, so the writer cannot aim it at somebody else's
/// conversation. `/api/v1` and the SDK pass `None`: they have no ambient
/// caller, and an alias there would name whichever thread the engine happened
/// to be serving. So the alias is refused rather than guessed.
///
/// Every thread-id slot an agent can write goes through here, because a slot
/// that only IGNORES the alias is the bug this exists to close: the string was
/// stored verbatim and became a dead deep link
/// (`docs/plans/2026-09-18-notification-tap-thread-id-is-a-uuid.md`).
pub fn resolve_thread_id_arg(raw: &str, caller: Option<Uuid>) -> Result<Uuid, String> {
    let raw = raw.trim();
    if CURRENT_THREAD_ALIASES
        .iter()
        .any(|alias| raw.eq_ignore_ascii_case(alias))
    {
        return caller.ok_or_else(|| {
            format!(
                "thread id '{raw}' names the thread the caller is working in, and this \
                 surface has none. Pass the thread's uuid."
            )
        });
    }
    // The advice fits the surface that asked. Only a caller with an ambient
    // thread can spend the alias, so offering it to one that cannot is a
    // second dead end.
    Uuid::parse_str(raw).map_err(|_| match caller {
        Some(_) => format!(
            "thread id '{raw}' is not a uuid. Pass 'current' for this thread. For \
             another thread, copy its id from the `threads` tool's 'search' or \
             'list' result."
        ),
        None => format!("thread id '{raw}' is not a uuid."),
    })
}
