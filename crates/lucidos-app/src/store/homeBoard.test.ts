import { describe, it, expect } from 'vitest';
import { landNextHomeOnBoard, takeHomeLanding } from './homeBoard';

/** On a phone only the Home icon, the Home row and a cold start land on Home's
 *  board face. A link or a notification lands on the chat, where a reply is. */
describe('where an arrival in Home lands', () => {
  it('lands on the chat unless the board was asked for, once', () => {
    expect(takeHomeLanding()).toBe('chat');
    landNextHomeOnBoard();
    expect(takeHomeLanding()).toBe('board');
    expect(takeHomeLanding()).toBe('chat');
  });
});
