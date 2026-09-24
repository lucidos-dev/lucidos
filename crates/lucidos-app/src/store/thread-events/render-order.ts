import { compareSortKeys, happenedAt, isUningestedMessage } from './exchange-grouping';
import { isTurnlessBoundary } from './thread-event-types';
import type { Exchange } from './exchange';
import type { StoredEvent } from './thread-event-types';

// ---------------------------------------------------------------------------
// One clock, checked
// ---------------------------------------------------------------------------

/**
 * The transcript reads top to bottom by one clock, and this is the rule stated
 * as code.
 *
 * Flatten the fold's output the way the reader meets it: each exchange's
 * boundary, then that exchange's steps, then the next exchange. Every neighbour
 * pair must be non-decreasing in the fold's OWN sort key. The named exceptions
 * on `renderOrderViolations` are the only departures, and anything else is a
 * defect.
 *
 * **Tests are the caller, never the render path.** A check reorders nothing, so
 * a per-frame run would walk the whole transcript to report a bug nobody can
 * act on. It runs where a bug can still be stopped: `thread-flows-helpers.ts`
 * asserts it for every flow test, and `render-order.test.ts` drives it over a
 * corpus of turn shapes.
 *
 * The defect it was written for is in
 * `docs/plans/2026-09-20-the-transcript-reads-by-one-clock.md`.
 */

/** One row as the reader meets it: a boundary, or a step inside one. */
export interface RenderRow {
  /** Index of the exchange this row renders in. */
  exchangeIndex: number;
  kind: 'boundary' | 'step';
  type: string;
  seq: number;
  /** When the row happened, by the same reading the fold files rows with. */
  micros: number | null;
}

/** A pair of neighbours the clock puts the other way round. */
export interface RenderOrderViolation {
  above: RenderRow;
  below: RenderRow;
  /** Readable in a failed assertion, which is where these are seen. */
  summary: string;
}

/**
 * The sort key of one row.
 *
 * `happenedAt` rather than `created`, because that is the reading the fold
 * places rows by. A spoken row covers a stretch of time and is filed where its
 * words BEGAN (ADR 0206). A live row carries the browser's stamp rather than
 * the database's.
 */
function rowOf(
  exchangeIndex: number,
  kind: 'boundary' | 'step',
  seq: number,
  event: StoredEvent,
): RenderRow {
  return { exchangeIndex, kind, type: event.type, seq, micros: happenedAt(event) };
}

/**
 * The key past which rows may no longer sit ABOVE this exchange.
 *
 * Normally the boundary's own key: the card is on screen from that moment, so
 * anything later belongs under it. `null` for a card that holds no turn at all,
 * which owes nothing below it. Those are exceptions A and C.
 */
function engagedKey(exchange: Exchange, index: number): RenderRow | null {
  // Never picked up. The turn above is still writing and owes this card
  // nothing, so no row above it can be late.
  if (isUningestedMessage(exchange)) return null;
  // The reader's Stop, or a stopped child's note: neither takes a turn or
  // draws a body.
  if (isTurnlessBoundary(exchange.userEvent)) return null;
  const pickedUp = ingestionStep(exchange);
  if (pickedUp) return rowOf(index, 'step', pickedUp.seq, pickedUp.event);
  return rowOf(index, 'boundary', exchange.userSeq, exchange.userEvent);
}

/**
 * The step that announces the loop picking this message up, or undefined.
 *
 * The chat fast path writes the `MessageReceived` first. The agentic loop then
 * emits a `UserPromptInjected` carrying its id when it ingests it, which
 * `findAbsorbTarget` files here. Between the two the message sits in the queue
 * and the running turn keeps writing above it. That window is legitimate, and
 * this is where it ends.
 */
function ingestionStep(exchange: Exchange) {
  const ownId = exchange.userEvent._eventId;
  if (exchange.userEvent.type !== 'MessageReceived' || !ownId) return undefined;
  return exchange.steps.find(step =>
    step.event.type === 'UserPromptInjected'
    && (step.event as { injected_message_id?: string }).injected_message_id === ownId,
  );
}

/** Does this exchange hold the call this result answers? */
function resultRejoinsItsCall(exchange: Exchange, event: StoredEvent): boolean {
  if (event.type === 'CodingAgentToolResult') {
    const id = (event as { tool_use_id?: string }).tool_use_id;
    return !!id && exchange.steps.some(step =>
      step.event.type === 'CodingAgentToolCalled'
      && (step.event as { tool_use_id?: string }).tool_use_id === id,
    );
  }
  if (event.type !== 'ToolResult') return false;
  const calledId = (event as { tool_called_event_id?: string }).tool_called_event_id;
  return !!calledId && exchange.steps.some(step =>
    step.event.type === 'ToolCalled' && step.event._eventId === calledId,
  );
}

/**
 * Every place the flattened transcript reads out of order. Empty is the
 * invariant holding.
 *
 * **Three exceptions, and the set is closed.** A fourth shape is a bug until
 * somebody adds it here with its own reason. The glossary's § Render order
 * states each one and why the fold produces it on purpose.
 *
 * - **A. A message still in the queue.** `engagedKey` ends that window.
 * - **B. A result rejoining its call**, forgiven against the card that opened
 *   between the two and against no other.
 * - **C. The reader's Stop-waiting panel**, which takes no turn (ADR 0049).
 */
export function renderOrderViolations(exchanges: Exchange[]): RenderOrderViolation[] {
  const violations: RenderOrderViolation[] = [];
  // The latest key each exchange still accepts rows above it.
  const walls: (RenderRow | null)[] = exchanges.map(engagedKey);
  for (let i = 0; i < exchanges.length; i++) {
    const exchange = exchanges[i];
    // The nearest card below that owns anything. Exception B is about the card
    // that opened BETWEEN a call and its result, which is this one.
    let nextEngaged = i + 1;
    while (nextEngaged < exchanges.length && !walls[nextEngaged]) nextEngaged += 1;
    const rows = [
      rowOf(i, 'boundary', exchange.userSeq, exchange.userEvent),
      ...exchange.steps.map(step => rowOf(i, 'step', step.seq, step.event)),
    ];
    for (let below = i + 1; below < exchanges.length; below++) {
      const wall = walls[below];
      // Exception A: nothing below has engaged yet, so nothing is late.
      if (!wall) continue;
      for (const row of rows) {
        if (compareSortKeys(row.micros, row.seq, wall.micros, wall.seq) <= 0) continue;
        // Exception B: the row completes a call this exchange already drew.
        const step = below === nextEngaged && row.kind === 'step'
          ? exchange.steps.find(s => s.seq === row.seq)
          : undefined;
        if (step && resultRejoinsItsCall(exchange, step.event)) continue;
        violations.push({
          above: row,
          below: wall,
          summary: `${row.kind} ${row.type} (seq ${row.seq}) renders in exchange ${i}, `
            + `above exchange ${below}'s ${wall.kind} ${wall.type} (seq ${wall.seq}), `
            + 'which the clock puts first',
        });
        // One per pair of exchanges. A turn filed in the wrong card puts every
        // one of its rows above the same boundary. A failed assertion listing
        // hundreds of them says no more than the first does.
        break;
      }
    }
  }
  return violations;
}
