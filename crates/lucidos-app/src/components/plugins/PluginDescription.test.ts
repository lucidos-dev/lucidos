import { describe, expect, it } from 'vitest';
import { splitPluginDescription } from './PluginDescription';

describe('splitPluginDescription', () => {
  it.each<[string, string[]]>([
    ['', []],
    [' \r\n\t\n ', []],
    ['  One paragraph.  ', ['One paragraph.']],
    ['First\n\nSecond', ['First', 'Second']],
    [' \r\n First\r\n \t\r\nSecond \r\n\r\n\r\n Third \r\n ', ['First', 'Second', 'Third']],
    ['First \n line\r\n continues.\n\nSecond.', ['First line continues.', 'Second.']],
    ['**Bold** <script>alert(1)</script>', ['**Bold** <script>alert(1)</script>']],
  ])('splits %j into paragraphs', (description, expected) => {
    expect(splitPluginDescription(description)).toEqual(expected);
  });
});
