import { describe, it, expect } from 'vitest';
import { landNextHomeOnDashboard, takeHomeLanding } from './dashboards';

/** On a phone only the Home icon, the Home row and a cold start land on Home's
 *  dashboard face. A link or a notification lands on the chat, where a reply is. */
describe('where an arrival in Home lands', () => {
  it('lands on the chat unless the dashboard was asked for, once', () => {
    expect(takeHomeLanding()).toBe('chat');
    landNextHomeOnDashboard();
    expect(takeHomeLanding()).toBe('dashboard');
    expect(takeHomeLanding()).toBe('chat');
  });
});
