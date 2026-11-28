import { describe, it, expect } from 'vitest';
// @ts-expect-error Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error same
import { dirname, resolve } from 'node:path';
// @ts-expect-error same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const settingsView: string = readFileSync(resolve(here, '../SettingsView.tsx'), 'utf-8');

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

  it('says a trigger is never asked, and what decides for it instead', () => {
    expect(body).toMatch(/In chat a risky one asks you first/);
    expect(body).toMatch(/A trigger has nobody to ask/);
    expect(body).toMatch(/side-effect grant it declared/);
  });

  it('says the guard answers no coding-agent card, and which switch does', () => {
    expect(body).toMatch(/The guard does not answer a coding agent's cards/);
    expect(body).toMatch(/Claude Code asks through its own permissions list below/);
    expect(body).toMatch(/Auto-answer coding-agent cards, under Coding agent permissions, hands those to Lucidos/);
  });

  it('separates the always-on static answer from the toggled judge', () => {
    expect(body).toMatch(/Lucidos answers the plain reads for you, whatever these toggles say/);
  });

  it('says the judge answers a coding agent only under auto-answer', () => {
    expect(body).toMatch(/It answers a coding agent only under auto-answer/);
    expect(body).not.toMatch(/Turn the guard off and those ask you instead/);
  });

});

describe('Settings → Permissions: the Auto-answer coding-agent cards explainer', () => {
  const row = settingsView.indexOf('data-search-anchor="permissions:coding-agent-auto-answer"');
  const body = settingsView.slice(row, row + 2200).replace(/\s+/g, ' ');

  it('explains from the row label', () => {
    expect(body).toMatch(/Auto-answer coding-agent cards <Explainer title="Auto-answer coding-agent cards">/);
  });

  it('sits in the Coding agent permissions section', () => {
    expect(settingsView).toMatch(/lead=\{\(\s*<>\s*\{codingAgentAutoAnswerRow\(\)\}/);
    expect(body).toMatch(/The command guard switch does not change this/);
  });

  it('says what the switch widens, and what it costs', () => {
    expect(body).toMatch(/Lucidos can answer for you/);
    expect(body).toMatch(/so a wrong reversible call has no undo/);
    expect(body).toMatch(/An irreversible or catastrophic one never runs unasked/);
  });

  it('says auto-answer only sees what would otherwise ask', () => {
    expect(body).toMatch(/It sees only what would otherwise ask you/);
    expect(body).toMatch(/Off, Lucidos runs no Claude Code command for you/);
  });

  it('says the catastrophic floor holds whatever the switches say', () => {
    expect(body).toMatch(/Lucidos refuses a catastrophic command it recognises/);
  });

  it('says a safe Codex sandbox escape runs regardless of the switches', () => {
    expect(body).toMatch(/Codex is different/);
    expect(body).toMatch(/Lucidos lets a safe one run whatever these switches say/);
  });
});
