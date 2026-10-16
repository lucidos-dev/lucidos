import { describe, it, expect } from 'vitest';
import { parseCsv, renderCsvTable } from './csv';

describe('parseCsv (RFC 4180)', () => {
  it('parses simple rows', () => {
    expect(parseCsv('a,b,c\n1,2,3')).toEqual([
      ['a', 'b', 'c'],
      ['1', '2', '3'],
    ]);
  });

  it('parses quoted fields with embedded commas', () => {
    expect(parseCsv('"a, b",c')).toEqual([['a, b', 'c']]);
  });

  it('parses escaped double quotes inside quoted fields', () => {
    expect(parseCsv('"a, b","c""d"')).toEqual([['a, b', 'c"d']]);
  });

  it('parses embedded newlines inside quoted fields', () => {
    expect(parseCsv('"line one\nline two",next')).toEqual([['line one\nline two', 'next']]);
  });

  it('handles CRLF line endings', () => {
    expect(parseCsv('a,b\r\n1,2\r\n3,4')).toEqual([
      ['a', 'b'],
      ['1', '2'],
      ['3', '4'],
    ]);
  });

  it('preserves leading/trailing spaces in unquoted fields', () => {
    // RFC 4180 §2.4: spaces are considered part of a field.
    expect(parseCsv(' a , b ')).toEqual([[' a ', ' b ']]);
  });

  it('treats a trailing newline as no extra row', () => {
    expect(parseCsv('a,b\n')).toEqual([['a', 'b']]);
  });

  it('returns an empty array for an empty string', () => {
    expect(parseCsv('')).toEqual([]);
  });

  it('parses an unterminated quoted field by closing it at EOF', () => {
    // Malformed input — best effort, no throw.
    expect(parseCsv('"unterminated')).toEqual([['unterminated']]);
  });
});

describe('renderCsvTable', () => {
  // renderCsvTable runs each cell through `escapeHtml`, which uses the DOM
  // (document.createElement). The vitest stub for document returns empty
  // strings for innerHTML, so we assert STRUCTURE (cell count, table
  // skeleton) here; cell-content correctness lives on the `parseCsv` tests
  // above and is verified end-to-end in the browser via the inline CSV
  // preview when an artifact is opened.

  it('renders the table skeleton with thead and tbody', () => {
    const html = renderCsvTable('a,b\n1,2');
    expect(html).toContain('<table class="csv-table">');
    expect(html).toContain('<thead>');
    expect(html).toContain('<tbody>');
    expect(html).toContain('</table>');
  });

  it('emits one <th> per first-row cell', () => {
    const oneCol = renderCsvTable('a').match(/<th>/g) ?? [];
    const twoCol = renderCsvTable('a,b').match(/<th>/g) ?? [];
    const quotedTwoCol = renderCsvTable('"a, b","c""d"').match(/<th>/g) ?? [];
    expect(oneCol.length).toBe(1);
    expect(twoCol.length).toBe(2);
    // Pre-fix `"a, b","c""d"` was split on bare commas into 4 cells; the
    // RFC 4180 parser correctly produces 2.
    expect(quotedTwoCol.length).toBe(2);
  });

  it('emits one <tr> per data row after the header', () => {
    const html = renderCsvTable('a,b\n1,2\n3,4');
    const bodyRows = (html.match(/<tr>/g) ?? []).length;
    // 1 header row + 2 data rows = 3 <tr> tags total.
    expect(bodyRows).toBe(3);
  });

  it('does not split an embedded newline into a new row', () => {
    // Pre-fix split on every '\n', so this CSV rendered as 3 <tr> rows.
    // RFC 4180 keeps the newline inside the quoted field → 2 rows.
    const html = renderCsvTable('"row\none","x"\n"r2","y"');
    const rowCount = (html.match(/<tr>/g) ?? []).length;
    expect(rowCount).toBe(2);
  });

  it('returns Empty CSV for an empty string', () => {
    expect(renderCsvTable('')).toBe('<p>Empty CSV</p>');
  });
});
