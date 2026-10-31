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
 *  live in the two lists below it. With the master toggle on, the guard also
 *  judges coding-agent commands, unless the coding-agents child is off.
 *
 *  Asserted over WHITESPACE-COLLAPSED source, so a reflow of the JSX cannot
 *  fail a claim that is still on the page. What each assertion pins is the
 *  claim, never the line break that happens to sit inside it. */
describe('Settings → Permissions: the Command safety explainer', () => {
  const section = settingsView.indexOf('data-search-anchor="command-safety"');
  const body = settingsView.slice(section, section + 2600).replace(/\s+/g, ' ');

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

  /** `command_guard_coding_agents` defaults on, so turning the guard on also
   *  answers coding-agent commands. Copy that said "by default it does not"
   *  promised cards the guard no longer raises. */
  it('says when a coding agent is covered, and where its permissions are', () => {
    expect(body).toMatch(/With the guard off, it does not gate a coding agent's tools/);
    expect(body).toMatch(/Claude Code asks through its own permissions list below/);
    expect(body).toMatch(/With the guard on, it also judges those commands, unless you turn off Enabled for coding agents/);
  });

  /** Two exceptions send a Codex escape through without a card, and only one
   *  of them is this section's. The static pass in `attended_command_allowed`
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

/** The coding-agents row widens what runs without a card, with no undo, so its
 *  own explainer has to say so beside the switch. */
describe('Settings → Permissions: the Enabled for coding agents explainer', () => {
  const row = settingsView.indexOf('data-search-anchor="command-safety:coding-agents"');
  const body = settingsView.slice(row, row + 1200).replace(/\s+/g, ' ');

  it('explains from the row label', () => {
    expect(body).toMatch(/Enabled for coding agents <Explainer title="Enabled for coding agents">/);
  });

  it('says what the switch widens, and what it costs', () => {
    expect(body).toMatch(/The guard also judges the commands of <strong>Claude Code<\/strong>/);
    expect(body).toMatch(/so a wrong reversible call has no undo/);
    expect(body).toMatch(/Irreversible and catastrophic commands still ask/);
  });

  it('says what the guard still does with the switch off', () => {
    expect(body).toMatch(/answers only the Codex commands it places as safe/);
  });
});
