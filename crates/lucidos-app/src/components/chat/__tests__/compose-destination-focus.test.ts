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
    focusCodingAgent: vi.fn(),
    switchMenuItem: vi.fn(),
    openSettingsSubview: vi.fn(),
  };
}

describe('compose destination final selection focus', () => {
  it('focuses the prompt after selecting the Lucidos Agent', () => {
    const deps = destinationDeps();

    handleComposeDestinationSelection('thread-1', 'agent', deps);

    expect(deps.apply).toHaveBeenCalledWith('thread-1', { kind: 'lucidos-agent' });
    expect(deps.focusPrompt).toHaveBeenCalledOnce();
    expect(deps.focusCodingAgent).not.toHaveBeenCalled();
    expect(deps.switchMenuItem).not.toHaveBeenCalled();
  });

  it('focuses the coding-agent picker after selecting a coding destination', () => {
    const deps = destinationDeps();

    handleComposeDestinationSelection(null, 'code:lucidos', deps);

    expect(deps.apply).toHaveBeenCalledWith(null, { kind: 'coding', scope: { kind: 'lucidos' } });
    expect(deps.focusPrompt).not.toHaveBeenCalled();
    expect(deps.focusCodingAgent).toHaveBeenCalledOnce();
  });

  it('does not focus the prompt for the register-repository action row', () => {
    const deps = destinationDeps();

    handleComposeDestinationSelection('thread-1', REGISTER_REPO_OPTION_VALUE, deps);

    expect(deps.apply).not.toHaveBeenCalled();
    expect(deps.focusPrompt).not.toHaveBeenCalled();
    expect(deps.focusCodingAgent).not.toHaveBeenCalled();
    expect(deps.switchMenuItem).toHaveBeenCalledWith('settings');
    expect(deps.openSettingsSubview).toHaveBeenCalledWith('repositories');
  });

  it('focuses the prompt after selecting a coding agent chip value', () => {
    const deps = {
      setCodingAgentDefault: vi.fn().mockResolvedValue(undefined),
      focusPrompt: vi.fn(),
    };

    handleComposeCodingAgentSelection('claude-code', deps);

    expect(deps.setCodingAgentDefault).toHaveBeenCalledWith('claude-code');
    expect(deps.focusPrompt).toHaveBeenCalledOnce();
  });

  it('keeps keyboard selection from restoring focus to the compose dropdown trigger', () => {
    const composeDropdowns = composeRowSource.match(/<Dropdown[\s\S]*?\/>/g) ?? [];
    expect(composeDropdowns.length).toBeGreaterThanOrEqual(2);
    expect(composeDropdowns[0]).toMatch(/class="compose-destination-picker"[\s\S]*restoreFocusOnSelect=\{false\}/);
    expect(composeDropdowns[1]).toMatch(/class="compose-coding-agent-chip"[\s\S]*restoreFocusOnSelect=\{false\}/);
  });
});
