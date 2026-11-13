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
 *  live in the two lists below it. The guard never answers their cards:
 *  coding-agent auto-answer, under Coding agent permissions, does.
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

  /** `coding_agent_auto_answer` is independent of the guard, so turning the
   *  guard on answers no coding-agent card. The copy names the switch that does. */
  it('says the guard answers no coding-agent card, and which switch does', () => {
    expect(body).toMatch(/The guard does not answer a coding agent's cards/);
    expect(body).toMatch(/Claude Code asks through its own permissions list below/);
    expect(body).toMatch(/Auto-answer coding-agent cards, under Coding agent permissions, hands those to Lucidos/);
  });

  /** The static pass in `attended_command_allowed` runs ahead of both
   *  toggles, so a plain read is answered even with Command safety off.
   *  Attributing that to the judge would promise a card the guard does not
   *  raise. */
  it('separates the always-on static answer from the toggled judge', () => {
    expect(body).toMatch(/Lucidos answers the plain reads for you, whatever these toggles say/);
  });

  /** Turning the guard on must never read as asking less: the judge answers a
   *  coding agent only under auto-answer, never under these toggles. */
  it('says the judge answers a coding agent only under auto-answer', () => {
    expect(body).toMatch(/It answers a coding agent only under auto-answer/);
    expect(body).not.toMatch(/Turn the guard off and those ask you instead/);
  });

});

/** The auto-answer row widens what runs without a card, with no undo, so its
 *  own explainer has to say so beside the switch. */
describe('Settings → Permissions: the Auto-answer coding-agent cards explainer', () => {
  const row = settingsView.indexOf('data-search-anchor="permissions:coding-agent-auto-answer"');
  const body = settingsView.slice(row, row + 2200).replace(/\s+/g, ' ');

  it('explains from the row label', () => {
    expect(body).toMatch(/Auto-answer coding-agent cards <Explainer title="Auto-answer coding-agent cards">/);
  });

  /** The switch sits with the coding-agent list it answers beyond, never
   *  under the command guard, which it does not depend on. */
  it('sits in the Coding agent permissions section', () => {
    expect(settingsView).toMatch(/lead=\{\(\s*<>\s*\{codingAgentAutoAnswerRow\(\)\}/);
    expect(body).toMatch(/The command guard switch does not change this/);
  });

  it('says what the switch widens, and what it costs', () => {
    expect(body).toMatch(/Lucidos can answer for you/);
    expect(body).toMatch(/so a wrong reversible call has no undo/);
    expect(body).toMatch(/An irreversible or catastrophic one never runs unasked/);
  });

  /** The permission mode and the allowlist settle a command before the gate
   *  runs. "Off, every command asks" would promise cards that never come. */
  it('says auto-answer only sees what would otherwise ask', () => {
    expect(body).toMatch(/It sees only what would otherwise ask you/);
    expect(body).toMatch(/Off, Lucidos runs no Claude Code command for you/);
  });

  /** `prompt_coding_agent_permission` refuses a catastrophic command ahead of
   *  every switch, so "off, the guard stays out" would be false. */
  it('says the catastrophic floor holds whatever the switches say', () => {
    expect(body).toMatch(/Lucidos refuses a catastrophic command it recognises/);
  });

  /** `attended_command_allowed` lets a Safe Codex escape run whatever the
   *  switches say, so the copy pins that on the sandbox, not on this switch. */
  it('says a safe Codex sandbox escape runs regardless of the switches', () => {
    expect(body).toMatch(/Codex is different/);
    expect(body).toMatch(/Lucidos lets a safe one run whatever these switches say/);
  });
});
