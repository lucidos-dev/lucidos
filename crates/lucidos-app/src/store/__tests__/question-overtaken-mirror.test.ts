/**
 * The event types that mean an agent raced past an unanswered question exist
 * twice, and the two must name the same set.
 *
 * Rust's `ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES` gates typed-text routing
 * and, unioned with three extras, the restart preserve guard. The client's
 * `QUESTION_OVERTAKEN_STEP_TYPES` gates the question card's buttons and whether
 * the exchange may still read "Needs your answer". Both lists carried a
 * keep-in-sync comment and nothing enforced it.
 *
 * Drift is silent and lands on the user. Add a progression event on the Rust
 * side alone and the engine abandons the question while the card keeps live
 * buttons. The reader presses Answer and nothing happens.
 *
 * Reading the Rust source from Vitest follows `wrapper-shells-mirror.test.ts`.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
/** Repo root, from `crates/lucidos-app/src/store/__tests__/`. */
const REPO_ROOT = resolve(here, '../../../../..');
const EVENT_IMPL = 'crates/lucidos-engine/src/engine/thread_events/event_impl.rs';
const EXCHANGE_GROUPING = 'crates/lucidos-app/src/store/thread-events/exchange-grouping.ts';

/** Everything between `opener` and the first `closer` after it, comments cut. */
function declarationBody(file: string, opener: string, closer: string): string {
  const src: string = readFileSync(resolve(REPO_ROOT, file), 'utf8');
  const start = src.indexOf(opener);
  expect(
    start,
    `could not find \`${opener}\` in ${file}. If it was renamed or moved, update this mirror rather than deleting it.`,
  ).toBeGreaterThan(-1);
  const end = src.indexOf(closer, start);
  expect(end, `\`${opener}\` in ${file} is never closed by \`${closer}\``).toBeGreaterThan(start);
  return src
    .slice(start, end)
    .split('\n')
    .filter((line) => !line.trim().startsWith('//'))
    .join('\n');
}

/** The names Rust lists, read out of the engine source. */
function rustNames(): string[] {
  const body = declarationBody(
    EVENT_IMPL,
    "pub const QUESTION_OVERTAKEN_EVENT_TYPES: &'static [&'static str] = &[",
    '];',
  );
  return [...body.matchAll(/"([^"]+)"/g)].map((m) => m[1]);
}

/** The names the client lists.
 *
 *  Read out of the source because the constant is module-private, and the
 *  exchange grouping should not export it only so a test can see it. */
function clientNames(): string[] {
  const body = declarationBody(
    EXCHANGE_GROUPING,
    'const QUESTION_OVERTAKEN_STEP_TYPES: ReadonlySet<string> = new Set([',
    ']);',
  );
  return [...body.matchAll(/'([^']+)'/g)].map((m) => m[1]);
}

describe('the client knows every event that overtakes a question', () => {
  it('parses a non-empty list from each side', () => {
    // A regex that quietly matches nothing would report green for ever, so the
    // count is asserted before the comparison that depends on it.
    expect(rustNames().length).toBeGreaterThan(0);
    expect(clientNames().length).toBeGreaterThan(0);
  });

  it('names exactly what `QUESTION_OVERTAKEN_EVENT_TYPES` names', () => {
    // Both sides are membership tests, so order carries no meaning. Comparing
    // sorted copies reports the difference in each direction.
    expect([...clientNames()].sort()).toEqual([...rustNames()].sort());
  });

  it('lists each name once', () => {
    // A duplicate would make the set smaller than the list and hide a missing
    // name behind a matching count.
    expect(new Set(rustNames()).size).toBe(rustNames().length);
    expect(new Set(clientNames()).size).toBe(clientNames().length);
  });
});
