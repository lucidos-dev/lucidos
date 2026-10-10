import { describe, it, expect } from 'vitest';
// @ts-expect-error Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error same
import { dirname, resolve } from 'node:path';
// @ts-expect-error same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const settingsView: string = readFileSync(resolve(here, '../SettingsView.tsx'), 'utf-8');

/** The Command safety section says which agent's commands it reaches, because
 *  nothing else on the page does and the answer is not the obvious one. The
 *  guard covers the Lucidos Agent's own tools. A coding agent's permissions
 *  live in the two lists below it, and a Codex sandbox escape asks on its own
 *  whatever this section says. The one overlap is the LLM judge, which places
 *  an ambiguous Codex escape only while the master toggle is on.
 *
 *  Asserted over WHITESPACE-COLLAPSED source, so a reflow of the JSX cannot
 *  fail a claim that is still on the page. What each assertion pins is the
 *  claim, never the line break that happens to sit inside it. */
describe('Settings → Permissions: the Command safety explainer', () => {
  const section = settingsView.indexOf('data-search-anchor="command-safety"');
  const body = settingsView.slice(section, section + 1800).replace(/\s+/g, ' ');

  it('is on the page at all', () => {
    expect(section).toBeGreaterThan(-1);
  });

  it('explains from the section title, like the two allowlists under it', () => {
    expect(body).toMatch(
      /data-search-anchor="command-safety"> Command safety <Explainer title="Command safety">/,
    );
  });

  it('names the agent the guard covers, and both its channels', () => {
    expect(body).toMatch(/Lucidos Agent's<\/strong> own shell and Python tools/);
    expect(body).toMatch(/in chat and in triggers/);
  });

  /** A trigger runs unattended, so nothing it does can raise a card.
   *  `action_for_lane` resolves a trigger's irreversible command against its
   *  side-effect grant and never asks. Copy that promised an ask without
   *  naming the split told a trigger author they would be consulted. */
  it('says a trigger is never asked, and what decides for it instead', () => {
    expect(body).toMatch(/In chat a risky one asks you first/);
    expect(body).toMatch(/A trigger has nobody to ask/);
    expect(body).toMatch(/side-effect grant it declared/);
  });

  it('says a coding agent is not covered, and where its permissions are', () => {
    expect(body).toMatch(/does not gate a coding agent's tools/);
    expect(body).toMatch(/Claude Code asks through its own permissions list below/);
  });

  /** Two exceptions send a Codex escape through without a card, and only one
   *  of them is this section's. The static pass in `attended_escalation_allowed`
   *  runs ahead of both toggles, so a plain read is answered even with Command
   *  safety off. Attributing that to the judge would promise a card the guard
   *  does not raise. */
  it('separates the always-on static answer from the toggled judge', () => {
    expect(body).toMatch(/Lucidos answers the plain reads for you, whatever these toggles say/);
  });

  /** The one claim a reader could not derive from the rest of the page: the
   *  judge half of this section decides Codex sandbox escapes too, so turning
   *  the master off moves those back to a permission card. */
  it('says the judge reaches Codex, and what turning the guard off does to that', () => {
    expect(body).toMatch(/reaches past the Lucidos Agent/);
    expect(body).toMatch(/Codex sandbox escape/);
    expect(body).toMatch(/Turn the guard off and those ask you instead/);
  });
});
