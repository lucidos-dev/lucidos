import type { CodingAgent } from '../../api/types';
import { ClaudeIcon, CodexIcon } from './icons';
import { LucidosMark } from './LucidosMark';
import { CategoryIcon } from './CategoryIcon';

/**
 * Icon for a thread's TYPE — which agent / source drives it. Mirrors the chat
 * executor mapping in `describeExecutor` (chat-exchange-parts.tsx) so a thread
 * wears the same mark wherever it's shown:
 *   - Codex coding-agent thread      → the Codex mark
 *   - Claude Code coding-agent thread → the Claude mark
 *   - trigger thread                 → the trigger glyph (matches the drawer's
 *                                       "Trigger" chip; triggers run the Lucidos
 *                                       Agent but read as their own type)
 *   - plain Lucidos Agent (chat) thread → the Lucidos mark
 *
 * Used by the thread-pane back/forward history menu (ThreadNav) to label each
 * remembered thread by its type. The `channel`/`codingAgent` come straight off
 * `ThreadMeta`; absent/legacy values default to a Lucidos / Claude Code reading
 * exactly as `formatThreadChannelLabel` does.
 */
export function ThreadTypeIcon({ channel, codingAgent, size = '1rem' }: {
  channel: string;
  codingAgent?: CodingAgent | null;
  /** Size for the self-sizing Lucidos mark; the stroke/fill glyphs size from
   *  the `.nav-history-icon svg` CSS rule instead. */
  size?: string;
}) {
  if (channel === 'claude_code') {
    return codingAgent === 'codex' ? <CodexIcon /> : <ClaudeIcon />;
  }
  if (channel === 'trigger') return <CategoryIcon category="triggers" />;
  return <LucidosMark size={size} />;
}
