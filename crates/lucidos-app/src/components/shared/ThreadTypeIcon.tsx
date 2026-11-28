import type { CodingAgent } from '../../api/types';
import { ClaudeIcon, CodexIcon } from './icons';
import { LucidosGlyph } from './LucidosMark';
import { CategoryIcon } from './CategoryIcon';

export function ThreadTypeIcon({ channel, codingAgent, size = '1rem' }: {
  channel: string;
  codingAgent?: CodingAgent | null;
  size?: string;
}) {
  if (channel === 'claude_code') {
    return codingAgent === 'codex' ? <CodexIcon /> : <ClaudeIcon />;
  }
  if (channel === 'trigger') return <CategoryIcon category="triggers" />;
  return <LucidosGlyph size={size} />;
}
