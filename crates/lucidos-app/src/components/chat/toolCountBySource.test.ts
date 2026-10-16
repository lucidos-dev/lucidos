import { describe, expect, it } from 'vitest';
import { toolCountBySource } from './toolCountBySource';

describe('toolCountBySource', () => {
  it('splits the count into the producer and each MCP server, in array order', () => {
    const tools = ['read_file', 'run_bash', 'mcp', 'mcp__slack__search', 'mcp__slack__post', 'mcp__jira__issue'];
    expect(toolCountBySource(tools, 'Lucidos')).toBe('6 tools: 3 Lucidos, 2 slack, 1 jira');
  });

  it('states the bare count when no MCP server sent a tool', () => {
    expect(toolCountBySource(['read_file', 'mcp'], 'Lucidos')).toBe('2 tools');
    expect(toolCountBySource(['read_file'], 'Lucidos')).toBe('1 tool');
  });

  it('never reads a malformed name as a server', () => {
    expect(toolCountBySource(['mcp____tool', 'mcp__server__', 'mcp_x'], 'Lucidos')).toBe('3 tools');
  });
});
