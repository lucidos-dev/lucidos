/**
 * **A fold needs a body to fold.**
 *
 * Collapsing swaps the response body for a `⋯` stub, so on a turn whose body
 * draws nothing it swaps nothing for a mark: the turn does not collapse, the
 * stub APPEARS. Reported on 2026-08-10 as "it now collapses to ... when there
 * is nothing, while in flight", and in flight is where a blank body lives
 * longest.
 *
 * The gate was `hasEvents`, which is `events.length > 0` and answers a
 * different question. Two shapes make it run ahead of anything on screen, and
 * a coding-agent turn produces both from its first second: a whitespace-only
 * text event is pushed for every `CodingAgentTextStreamed`, and step mechanics
 * are hidden from a reader who turned the steps control off. So a turn that
 * had only worked, and drawn nothing, offered a live collapse control.
 *
 * This is the same trap `abort-boundary-renders-its-turn.test.ts` guards one
 * exchange over, and its note is worth reading beside this one: there the wrong
 * gate produced an empty panel whose only content was a status badge reading
 * "Working" over a stopped engine. Two callers, two predicates, because they
 * disagree about a hidden step on purpose. `hasRenderableResponseContent` asks
 * whether a turn is worth a PANEL and counts one, since the header carries the
 * control that reveals it. `drawsResponseRow` asks what is on screen NOW and
 * does not.
 *
 * A source scan: the gate is an expression inside `ChatExchangeImpl`, which
 * renders nothing without a whole thread's worth of store state. What it gates
 * is unit-tested at each end instead, in `store/event-rendering.test.ts` (the
 * predicate) and `turn-controls.test.tsx` (the disabled control).
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source: string = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf-8');

describe('a turn is collapsible only while its body draws something', () => {
  it('gates canCollapse on what is drawn, not on the event count', () => {
    expect(source).toMatch(
      /const canCollapse = hasResponse \|\| events\.some\(\(e\) => drawsResponseRow\(e, showSteps\)\)/,
    );
    // Reverting to `hasEvents` is the whole regression, so name it.
    expect(source).not.toMatch(/const canCollapse = hasResponse \|\| hasEvents/);
  });

  it('holds the fold to the same gate, so a blank turn cannot render as folded', () => {
    // Without this, a key already in the persisted `collapsedExchanges` set
    // (the turn was folded when it had content) would draw a `⋯` over a body
    // that has since stopped drawing anything.
    expect(source).toMatch(/const isCollapsed = canCollapse && collapsedExchanges\.value\.has\(/);
  });

  it('asks the panel the same question, rather than spelling it a second way', () => {
    // `hasBody` decides whether the body box is rendered AND whether the `⋯`
    // stub stands in for it. It was a duplicate `hasResponse || hasEvents`,
    // which is exactly how the two got to disagree: the gate could be fixed
    // while the stub kept rendering off the old test.
    expect(source).toMatch(/hasBody=\{canCollapse\}/);
    expect(source).not.toMatch(/hasBody=\{hasResponse \|\| hasEvents\}/);
  });
});

/**
 * **A change turn and a user message carry no fold.**
 *
 * Both were dropped on request, and for the same reason. A change turn's body
 * is a summary, a description and a file list. A user message is the reader's
 * own text, already as short as they made it. The control cost a row of chrome
 * on every one of them to fold a few lines.
 *
 * What keeps a fold is an initiator turn whose body can run long and is not
 * yours: a forwarded agent message, a question, a permission prompt.
 *
 * The same expression also decides `isInitiatorCollapsed`, so a turn a reader
 * folded before this cannot render as a stuck `⋯` stub.
 */
describe('the initiator fold skips a change turn and a user message', () => {
  it('gates it on both predicates, beside the body test', () => {
    expect(source).toMatch(
      /const canCollapseInitiator = !isChangePanel && !isUserMessageBubble\s*\n?\s*&& \(!!initiator\.summary \|\| !!initiator\.details\)/,
    );
  });

  for (const predicate of ['isChangePanel', 'isUserMessageBubble']) {
    it(`reads ${predicate} from above, so the gate cannot see it undefined`, () => {
      // Each is a `const`, so using one a line early is a temporal dead zone
      // throw rather than a quiet `undefined`. Order is the whole guard.
      expect(source.indexOf(`const ${predicate} =`)).toBeLessThan(
        source.indexOf('const canCollapseInitiator ='),
      );
    });
  }
});

/**
 * **A reveal clicked on a folded turn unfolds that turn.**
 *
 * A folded turn draws no body, so "Show steps" / "Show the full response"
 * clicked from its header would land on every other turn in the transcript and
 * do nothing where the click was made. The setting stays transcript-wide; what
 * the unfold clears is the local override that would hide it here.
 */
describe('turning a reveal on lifts this turn\'s fold', () => {
  it('expands on the way ON only, and never folds on the way off', () => {
    const fn = source.match(/function reveal\(setting: Signal<boolean>\)[\s\S]*?\n  \}/);
    expect(fn, '`reveal` not found').not.toBeNull();
    const body = fn![0];
    expect(body).toMatch(/setting\.value = !setting\.value/);
    // Guarded on the NEW value: a fold is the reader's own explicit act, so
    // something else may lift it and nothing may impose it.
    expect(body).toMatch(/if \(setting\.value\) expandExchange\(threadId, exchange\.userSeq\)/);
    expect(body).not.toMatch(/toggleExchangeCollapsed/);
  });

  it('routes both reveals through it, so the two cannot drift', () => {
    expect(source).toMatch(/const toggleDetails = heldOnThePress\(\(\) => reveal\(detailsExpanded\)\)/);
    expect(source).toMatch(/const toggleSteps = heldOnThePress\(\(\) => reveal\(stepsExpanded\)\)/);
  });

  it('keeps the scroll anchor around the whole thing', () => {
    // The reveal grows the turn the reader is looking at, and the unfold grows
    // it further. Both have to happen inside one anchor or it measures a height
    // the second half then changes. `heldOnThePress` wraps the WHOLE callback,
    // so a caller cannot split them.
    const fn = source.match(/function heldOnThePress[\s\S]*?\n\}/);
    expect(fn, '`heldOnThePress` not found').not.toBeNull();
    expect(fn![0]).toMatch(/withScrollAnchor\(e\.currentTarget as HTMLElement \| null, fn\)/);
  });

  it('anchors every control that changes a turn\'s height', () => {
    // The two reveals are transcript-wide; the two folds are this turn's. All
    // four move content, so all four hold the control the reader pressed.
    for (const held of [
      'const toggleDetails = heldOnThePress(',
      'const toggleSteps = heldOnThePress(',
      'const toggleCollapsed = heldOnThePress(',
      'const toggleInitiator = heldOnThePress(',
    ]) {
      expect(source, `${held} is not anchored`).toContain(held);
    }
  });
});
