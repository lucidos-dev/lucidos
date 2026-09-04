/**
 * The frontend suppresses one standing-apply drop toast: the one the owner
 * caused by cancelling. It tells that drop from the rest by its `reason`, which
 * the engine writes.
 *
 * So the two spellings have to match, and a silent drift here is invisible: the
 * toast simply starts firing on the user's own click. This reads the Rust
 * constant instead of trusting the copy.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { resolve, dirname } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { STANDING_APPLY_CANCELED } from './chat-changes';

const here = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(here, '../../../../..');
const STANDING_APPLY_RS = 'crates/lucidos-engine/src/engine/standing_apply.rs';

describe('the canceled-drop reason', () => {
  it('is spelled the same in the engine and here', () => {
    const src = readFileSync(resolve(REPO_ROOT, STANDING_APPLY_RS), 'utf8');
    const match = /pub const DISARMED_BY_OWNER: &str = "([^"]*)";/.exec(src);
    expect(match, `DISARMED_BY_OWNER not found in ${STANDING_APPLY_RS}`).not.toBeNull();
    expect(STANDING_APPLY_CANCELED).toBe(match![1]);
  });
});
