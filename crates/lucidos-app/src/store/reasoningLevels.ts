export const REASONING_LEVELS = [
  { value: 'none', label: 'Off' },
  { value: 'low', label: 'Low' },
  { value: 'medium', label: 'Med' },
  { value: 'high', label: 'High' },
  { value: 'xhigh', label: 'X-High' },
  { value: 'max', label: 'Max' },
];

export function availableReasoningLevels(
  supported?: readonly string[],
): typeof REASONING_LEVELS {
  const offered = supported ? REASONING_LEVELS.filter(l => supported.includes(l.value)) : [];
  return offered.length > 0 ? offered : REASONING_LEVELS;
}
