import { describe, expect, it } from 'vitest';
import { parseMcpToolName } from './mcpToolName';

describe('parseMcpToolName', () => {
  it('splits a server and a tool, keeping a later separator in the tool', () => {
    expect(parseMcpToolName('mcp__slack__search')).toEqual({ server: 'slack', tool: 'search' });
    expect(parseMcpToolName('mcp__roblox_studio__run__code')).toEqual({ server: 'roblox_studio', tool: 'run__code' });
  });

  it('reads nothing into a name with an empty half or no prefix', () => {
    for (const name of ['mcp____tool', 'mcp__server__', 'mcp_x', 'read_file']) {
      expect(parseMcpToolName(name)).toBeNull();
    }
  });
});
