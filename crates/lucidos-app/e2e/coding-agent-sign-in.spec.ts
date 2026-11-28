import { fileURLToPath } from 'node:url';
import { test, expect } from './fixtures';
import { psql } from './db-helpers';
import {
  apiRequest, assertHealthy, navigateToApp, newThread, pickComposeDestination, sendMessage,
  sendMessageCapturingThreadId,
} from './helpers';

const FAKE_CLAUDE = fileURLToPath(new URL('./fake-claude-code/sign-in.mjs', import.meta.url));
const PREF = 'coding_agent_claude_path';

function events(threadId: string, eventType: string): number {
  return Number(psql(`SELECT count(*) FROM events WHERE thread_id = '${threadId}' AND event_type = '${eventType}'`));
}

test.describe('coding agent sign-in in a thread', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    const res = await apiRequest(page).put(`/api/v1/preferences?key=${PREF}`, { data: { value: FAKE_CLAUDE } });
    expect(res.ok()).toBe(true);
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);
  });

  test.afterEach(() => {
    psql(`DELETE FROM preferences WHERE key = '${PREF}'`);
  });

  test('answers a command without starting a session and offers MCP sign-in only on a tool call or /mcp', async ({ page }) => {
    const threadId = await sendMessageCapturingThreadId(page, '/mcp');
    await expect(page.getByText('MCP server status is unavailable', { exact: false })).toBeVisible();
    await sendMessage(page, '/login');
    await expect(page.getByText('Run the sign-in command below in the thread terminal.')).toBeVisible();
    const suggestions = page.locator('[data-role="terminal-suggestion"]');
    const login = suggestions.filter({ hasText: 'Sign in to Claude Code' });
    await expect(login).toContainText(`env BROWSER=false ${FAKE_CLAUDE} auth login`);
    expect(events(threadId, 'SessionStarted')).toBe(0);

    await sendMessage(page, '/review');
    await expect(page.locator('.response-content').getByText('Received review verbatim')).toBeVisible({ timeout: 60_000 });
    await expect.poll(() => events(threadId, 'CodingAgentIdled')).toBe(1);
    await expect.poll(() => events(threadId, 'CodingAgentMcpStatusesReported')).toBe(1);
    const calendar = suggestions.filter({ hasText: 'calendar MCP server' });
    await expect(calendar).toHaveCount(0);

    await sendMessage(page, 'Check my calendar');
    await expect(page.locator('.response-content').getByText('The calendar needs a sign-in')).toBeVisible({ timeout: 60_000 });
    const toolCall = suggestions.filter({ hasText: 'A tool call needs the calendar MCP server. Sign in to it' });
    await expect(toolCall).toHaveCount(1);
    await expect(toolCall).toContainText(`${FAKE_CLAUDE} mcp login calendar --no-browser`);

    await sendMessage(page, '/mcp');
    await expect(page.getByText('Last reported MCP servers')).toBeVisible();
    const status = page.locator('[data-role="mcp-server-status"]');
    await expect(status.filter({ hasText: 'search' })).toContainText('connected');
    await expect(status.getByRole('button')).toHaveCount(0);
    const requested = suggestions.filter({ hasText: 'Sign in to the calendar MCP server' });
    await expect(requested).toHaveCount(1);
    await expect(requested).toContainText(`${FAKE_CLAUDE} mcp login calendar --no-browser`);
    expect(events(threadId, 'CodingAgentCommandReceived')).toBe(3);
    expect(events(threadId, 'CodingAgentCommandAnswered')).toBe(3);
    expect(events(threadId, 'AgentSignInRequested')).toBe(0);
    expect(events(threadId, 'MessageReceived')).toBe(2);
  });
});
