// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import {
  handleComposeCodingAgentSelection,
  handleComposeDestinationSelection,
} from '../ComposeDestinationRow';
import { REGISTER_REPO_OPTION_VALUE } from '../../../store/composeDestination';

const composeRowSource = readFileSync(new URL('../ComposeDestinationRow.tsx', import.meta.url), 'utf-8');

function destinationDeps() {
  return {
    apply: vi.fn(),
    focusPrompt: vi.fn(),
    switchMenuItem: vi.fn(),
    openSettingsSubview: vi.fn(),
    scrollToSetting: vi.fn(),
  };
}

describe('compose destination final selection focus', () => {
  it('focuses the prompt after selecting the Lucidos Agent', () => {
    const deps = destinationDeps();

    handleComposeDestinationSelection('thread-1', 'agent', deps);

    expect(deps.apply).toHaveBeenCalledWith('thread-1', { kind: 'lucidos-agent' });
    expect(deps.focusPrompt).toHaveBeenCalledOnce();
    expect(deps.switchMenuItem).not.toHaveBeenCalled();
  });

  it('focuses the prompt after selecting a coding destination', () => {
    const deps = destinationDeps();

    handleComposeDestinationSelection(null, 'code:lucidos', deps);

    expect(deps.apply).toHaveBeenCalledWith(null, { kind: 'coding', scope: { kind: 'lucidos' } });
    expect(deps.focusPrompt).toHaveBeenCalledOnce();
  });

  it('does not focus the prompt for the register-repository action row', () => {
    const deps = destinationDeps();

    handleComposeDestinationSelection('thread-1', REGISTER_REPO_OPTION_VALUE, deps);

    expect(deps.apply).not.toHaveBeenCalled();
    expect(deps.focusPrompt).not.toHaveBeenCalled();
    expect(deps.switchMenuItem).toHaveBeenCalledWith('settings');
    // Repositories share the Coding Agents page with the binaries that run
    // them, so the jump names the page AND the section it wants inside it.
    expect(deps.openSettingsSubview).toHaveBeenCalledWith('coding-agents');
    expect(deps.scrollToSetting).toHaveBeenCalledWith('coding-agents:repositories');
  });

  it('writes the coding agent to the focused draft, never a global, and focuses the prompt', () => {
    const deps = { patchSelection: vi.fn(), focusPrompt: vi.fn() };

    handleComposeCodingAgentSelection('thread-1', 'codex', deps);

    // Per-draft: only this draft's backend changes; the account default is left
    // to Settings (draft-only).
    expect(deps.patchSelection).toHaveBeenCalledWith('thread-1', { codingAgent: 'codex' });
    expect(deps.focusPrompt).toHaveBeenCalledOnce();
  });

  it('with no focused draft, writes the PENDING slot (null), never a global', () => {
    const deps = { patchSelection: vi.fn(), focusPrompt: vi.fn() };

    handleComposeCodingAgentSelection(null, 'claude-code', deps);

    // patchComposeSelection(null, …) routes to the pending slot — a fresh-compose
    // pick must NOT write `coding_agent_default` (every override-less draft reads
    // it, so that would leak the pick to all drafts).
    expect(deps.patchSelection).toHaveBeenCalledWith(null, { codingAgent: 'claude-code' });
    expect(deps.focusPrompt).toHaveBeenCalledOnce();
  });

  it('keeps keyboard selection from restoring focus to the compose dropdown trigger', () => {
    const composeDropdowns = composeRowSource.match(/<Dropdown[\s\S]*?\/>/g) ?? [];
    expect(composeDropdowns.length).toBeGreaterThanOrEqual(2);
    expect(composeDropdowns[0]).toMatch(/class="compose-destination-picker"[\s\S]*restoreFocusOnSelect=\{false\}/);
    expect(composeDropdowns[1]).toMatch(/class="compose-coding-agent-chip"[\s\S]*restoreFocusOnSelect=\{false\}/);
  });

  it('does not mark the last-used option in the compose destination or coding-agent pickers', () => {
    // The trigger already shows the current value; marking it inside the open
    // list would only compete with the arrow-key focus row (see Dropdown
    // `markCurrent`). Both compose pickers must opt out.
    const composeDropdowns = composeRowSource.match(/<Dropdown[\s\S]*?\/>/g) ?? [];
    expect(composeDropdowns[0]).toMatch(/class="compose-destination-picker"[\s\S]*markCurrent=\{false\}/);
    expect(composeDropdowns[1]).toMatch(/class="compose-coding-agent-chip"[\s\S]*markCurrent=\{false\}/);
  });
});
