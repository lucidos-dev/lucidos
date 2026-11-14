import {
  PREF_CHAT_MODEL,
  PREF_CHAT_REASONING_EFFORTS,
  PREF_INTELLIGENCE_LADDER_CHAT,
  PREF_INTELLIGENCE_LADDER_CLAUDE_CODE,
  PREF_INTELLIGENCE_LADDER_CODEX,
  PREF_LONG_PROMPT_SHARE,
  PREF_MODEL_CLAUDE_CODE,
  PREF_MODEL_CODEX,
  PREF_REASONING_CLAUDE_CODE,
  PREF_REASONING_CODEX,
} from '@lucidos/preference-catalog';
import type { CodingAgent } from '../api/types';

/** The preferences one backend's intelligence ladder reads and its editor
 *  writes in Settings → Intelligence. */
export const LADDER_PREFS: Record<CodingAgent, { ladder: string; model: string; effort: string }> = {
  'claude-code': {
    ladder: PREF_INTELLIGENCE_LADDER_CLAUDE_CODE.key,
    model: PREF_MODEL_CLAUDE_CODE.key,
    effort: PREF_REASONING_CLAUDE_CODE.key,
  },
  codex: {
    ladder: PREF_INTELLIGENCE_LADDER_CODEX.key,
    model: PREF_MODEL_CODEX.key,
    effort: PREF_REASONING_CODEX.key,
  },
};

/** The chat ladder's override, which decides what the chat picker lists. */
export const CHAT_LADDER_PREF = PREF_INTELLIGENCE_LADDER_CHAT.key;

/** The preferences the chat ladder's default rung reads. */
export const CHAT_DEFAULT_PREFS = [PREF_CHAT_MODEL.key, PREF_CHAT_REASONING_EFFORTS.key] as const;

/** The share of long-prompt calls, which every cost band reads. */
export const LONG_PROMPT_SHARE_PREF = PREF_LONG_PROMPT_SHARE.key;
