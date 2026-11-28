#!/usr/bin/env node

import { main } from './claude.mjs';

const asksForCalendar = (prompt) => prompt.trimEnd().endsWith('Check my calendar');

main(process.argv.slice(2), {
  answer: (prompt) => {
    if (prompt === '/review') return 'Received review verbatim';
    if (asksForCalendar(prompt)) return 'The calendar needs a sign-in';
    return undefined;
  },
  toolCall: (prompt) => asksForCalendar(prompt) ? 'mcp__calendar__list_events' : undefined,
  mcpServers: [
    { name: 'calendar', status: 'needs-auth' },
    { name: 'search', status: 'connected' },
  ],
});
