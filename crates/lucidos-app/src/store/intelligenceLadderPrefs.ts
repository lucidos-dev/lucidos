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

export const CHAT_LADDER_PREF = PREF_INTELLIGENCE_LADDER_CHAT.key;

export const CHAT_DEFAULT_PREFS = [PREF_CHAT_MODEL.key, PREF_CHAT_REASONING_EFFORTS.key] as const;

export const LONG_PROMPT_SHARE_PREF = PREF_LONG_PROMPT_SHARE.key;
