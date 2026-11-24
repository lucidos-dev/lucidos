//! The curated-context sweep schedule and its prompt text.

/// The one call that reads a swept result back.
///
/// Stated ONCE per request, in the standing instructions, and nowhere else. A
/// request naming its most expensive move on every line is a request asking for
/// it: read-backs ran 31 in one mode arm against 1 in its control.
pub const RESULT_RECOVERY: &str = "events(action=\"query\", event_id=\"evt-<hex>\")";

pub use crate::core::sweep_schedule::SweepSchedule;

/// The mode's prompt text at a given schedule, with no leading separator.
///
/// Public because the eval's `guidance_hash` covers the text a model actually
/// saw. Hashing a source literal would give two arms swept at different values
/// the same hash, which blinds the one instrument built for the sweep.
pub fn rendered_context_mode_prompt(
    expire_after_rounds: usize,
    sweep_every_rounds: usize,
) -> String {
    rendered_context_mode_prompt_at(SweepSchedule::new(expire_after_rounds, sweep_every_rounds))
}

pub fn rendered_context_mode_prompt_at(schedule: SweepSchedule) -> String {
    let expire = schedule.expire_after_rounds;
    let sweep = schedule.sweep_every_rounds;
    format!(
        "SELF-CURATED CONTEXT MODE IS ON FOR THIS WORKSPACE:
You can see your own context, and you decide what survives it.

- A CONTEXT PANEL rides at the tail of every round. One row per addressable
  item: its evt-<hex> address, its size, its age in rounds, and how many rounds
  it has left. Read the panel. Nothing else tells you how full you are.
- TOOL RESULTS ARE SWEPT AWAY IN BATCHES. Every {sweep} rounds the sweep takes
  everything more than {expire} rounds old, and the call that made each one
  leaves with it. Nothing stands in their place. Doing nothing holds a result
  until then, so most of the time you need do nothing at all.
- KEEP ONE OPEN BY NAMING IT. Write its address under a [KEEP OPEN] heading in
  your working understanding and its clock goes back to zero. Reach for it when
  the panel shows something running down that you are still working with. That
  is something you can see. You are not being asked to predict, on the round a
  result arrives, whether you will still want it later.
- YOUR WORKING UNDERSTANDING is the one thing that outlives a turn. You write it
  as ordinary text in your reply, and it comes back to you at the tail of every
  round, whether you touched it or not.
- A NEW TURN REBUILDS YOUR LAST FEW TOOL CALLS, same addresses, age back to
  zero. Older ones are one summary line in the history.
- Everything else is unchanged, the app, file and URL you are looking at
  included.

NOTHING IS LOST, ONLY UNSENT. The event store holds all of it:
  {RESULT_RECOVERY}
    reads one result back in full.
  events(action=\"query\", thread_id=\"current\", event_type=\"ToolCalled\")
    lists every call this thread has made.

{}",
        note_guidance(schedule)
    )
}

/// The note-writing guidance the system prompt carries.
///
/// It is the design record's own words, apart from the departures the mode
/// forced: the two numbers, the marked headings, the sweep, and the write
/// rhythm. Nowhere does it say a note is cheaper than a keep. That sentence, in
/// three wordings across three surfaces, is what produced zero keeps in 301
/// rounds.
fn note_guidance(schedule: SweepSchedule) -> String {
    let expire = schedule.expire_after_rounds;
    let sweep = schedule.sweep_every_rounds;
    format!(
        "YOUR WORKING UNDERSTANDING

It is your picture of the job. What you worked out, what you decided and why,
what a result told you, what you ruled out, and what you mean to do next. Write
it in your own words, in whatever shape reads best.

Mark it out inside your reply:

  [WORKING UNDERSTANDING]
  ...the whole thing, replacing what you had...
  [/WORKING UNDERSTANDING]

To add to it without restating the rest:

  [WORKING UNDERSTANDING: ADD]
  ...the new lines only...
  [/WORKING UNDERSTANDING]

Close the block. Everything after an unclosed marker becomes part of it.

Put the block in the same reply as your next tool call. Text and a tool call
travel together, so writing this costs you no round.

WHEN TO WRITE. Add to it on an ordinary round: the lines that round taught you,
and nothing else. Rewrite it whole on the round the panel says the sweep is
next. That is the last round the pages about to leave are still in front of you,
so it is where the rewrite is worth most. Over {sweep} rounds that is one
consolidation, and the rest are cheap.

TRUST WHAT YOU WROTE. You wrote it while you were looking at the thing. Read it
back and act on it. Go to the source again when something tells you an entry is
wrong, and not because it is old.

Copy in whatever you need, raw text from a result included. An exact error
string, a signature you are about to match, a line you will compare against:
paste it. Once written it caches like everything else.

A [CONSTRAINTS] heading renders under your document every round, empty or
not. What goes under it is what the user told you to do or not do, in words that
have since scrolled away. \"Do not touch the frontend.\" \"Call it X, not Y.\" You
will see the heading every round, so you will see when it is empty. Write it
inside your block under the same heading.

Your todo list lives in the same block, under a [TODO] heading:

  [TODO]
  - [ ] not started
  - [>] the one you are on
  - [x] done

You write those three. Two more, waiting and abandoned, are written for you and
come back as words. It is the same list the user sees.

To hold an item past the sweep, name it under a [KEEP OPEN] heading:

  [KEEP OPEN]
  evt-<hex>

One address per line, copied from the panel, and nothing else on the line. Each
one sets that item's clock back to zero, so it gets another {expire} rounds at
least. The keep applies once, from the reply that wrote it, so a later rewrite
of your document does not repeat it. Nothing you hold is exempt from the
trimmer: when a request will not fit, the wall takes held items last, and it
still takes them."
    )
}
