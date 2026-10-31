import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { PLAIN_RESOLUTION_NOTES } from '../PermissionCard';
import { TOOL_CALL_CAP_OPENING } from '../ChatExchange';
import { LEGACY_REENTRY_OPENINGS } from '../../../store/thread-events';

/**
 * Three UI tables match on words the engine writes for the agent: permission
 * resolution reasons, the old re-entry openings, and the tool-call cap message.
 * If the engine rewords one, the card silently falls back to raw engine text.
 * This reads the engine source, so the rewording fails here instead.
 */

const here = dirname(fileURLToPath(import.meta.url));
const ENGINE = resolve(here, '../../../../../lucidos-engine/src');

const engineText = [
  'engine/cc_permission.rs',
  'engine/command_permission.rs',
  'engine/mcp_permission.rs',
  'engine/agent_recovery/has_diff.rs',
  'engine/event_wait/mod.rs',
  'engine/agentic_loop/helpers.rs',
].map((path) => readFileSync(resolve(ENGINE, path), 'utf8')).join('\n');

describe('the engine still writes the words the UI matches on', () => {
  it.each(PLAIN_RESOLUTION_NOTES.map(([opening]) => opening))('a permission reason opening "%s"', (opening) => {
    expect(engineText).toContain(opening);
  });

  it.each(LEGACY_REENTRY_OPENINGS.map(([opening]) => opening))('a re-entry opening "%s"', (opening) => {
    expect(engineText).toContain(opening);
  });

  it('the tool-call cap message', () => {
    expect(engineText).toContain(`[ENGINE-LIMIT] ${TOOL_CALL_CAP_OPENING}`);
  });
});
