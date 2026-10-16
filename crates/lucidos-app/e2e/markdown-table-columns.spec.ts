import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy } from './helpers';

// Phone width in every project: the collapse needed `.response-content`'s
// mobile `word-break` rule to be active, and the stacked layout is gated on
// the same 768px line.
test.use({ viewport: { width: 390, height: 844 } });

/**
 * Regression: a markdown table rendered its KEY column one character per line,
 * so the column identifying each row was the only unreadable one.
 *
 * Three rules compounded. `.response-content { word-break: break-word }`
 * (mobile.css) is the deprecated alias for `overflow-wrap: anywhere`, which
 * unlike `break-word` DOES count toward min-content intrinsic sizing, so every
 * cell could be one character wide; `.markdown-content a { word-break:
 * break-all }` did the same for a cell holding a link; and `width: 100%` on the
 * table meant auto layout HAD to fit the pane, so it spent the width on the
 * column with the most content and starved the rest.
 *
 * These are layout properties: the emitted HTML was always correct, so no
 * amount of unit testing on `renderMarkdown` output can see them. The
 * transform's own contract (`data-stack` / `data-label`) is covered in
 * src/utils/renderMarkdown.test.ts.
 *
 * Markup is injected into a real `.response-content.markdown-content` host so
 * the live app cascade applies, the same technique as
 * markdown-nested-list-markers.spec.ts.
 */

/** The exact shape that was reported: a short key column beside a prose one
 *  whose longest token is far longer than anything in the key column. */
const KEY_VALUE_TABLE = `
<div class="table-scroll-wrapper">
  <table>
    <thead><tr><th>Commit</th><th>What it does</th></tr></thead>
    <tbody>
      <tr>
        <td id="t-key-cell">docs(plans): open a file at a line from navigate</td>
        <td><code>docs/plans/2026-08-05-navigate-file-at-a-line.md</code>, the
            approved plan, later amended with two invariants that emerged
            during implementation.</td>
      </tr>
    </tbody>
  </table>
</div>`;

test.describe('Markdown table columns', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
  });

  test('no column is laid out narrower than its longest word', async ({ page }) => {
    const measured = await page.evaluate((markup) => {
      const host = document.createElement('div');
      host.className = 'response-content markdown-content';
      host.style.width = '360px';
      host.innerHTML = markup;
      document.body.appendChild(host);

      const cell = document.getElementById('t-key-cell')!;
      const style = getComputedStyle(cell);
      const contentWidth =
        cell.getBoundingClientRect().width -
        parseFloat(style.paddingLeft) -
        parseFloat(style.paddingRight);

      // Probe the longest unbreakable word in the cell's OWN font, by
      // measuring it inside the cell. Absolute + hidden so it cannot
      // perturb the layout it is measuring.
      const probe = document.createElement('span');
      probe.style.position = 'absolute';
      probe.style.visibility = 'hidden';
      probe.style.whiteSpace = 'pre';
      probe.textContent = 'docs(plans):';
      cell.appendChild(probe);
      const longestWordWidth = probe.getBoundingClientRect().width;

      host.remove();
      return { contentWidth, longestWordWidth };
    }, KEY_VALUE_TABLE);

    // The invariant, stated exactly: the column fits its longest word. Before
    // the fix the content width was about one character.
    expect(measured.longestWordWidth).toBeGreaterThan(0);
    expect(measured.contentWidth).toBeGreaterThanOrEqual(measured.longestWordWidth);
  });

  test('a table absorbs its own overflow and never widens the page', async ({ page }) => {
    const widths = await page.evaluate((markup) => {
      const before = document.documentElement.scrollWidth;

      const host = document.createElement('div');
      host.className = 'response-content markdown-content';
      host.style.width = '360px';
      host.innerHTML = markup;
      document.body.appendChild(host);

      const wrapper = host.querySelector('.table-scroll-wrapper')!;
      const after = document.documentElement.scrollWidth;
      const result = {
        before,
        after,
        wrapperClientWidth: wrapper.clientWidth,
        wrapperScrollWidth: wrapper.scrollWidth,
        overscrollBehaviorX: getComputedStyle(wrapper).overscrollBehaviorX,
      };
      host.remove();
      return result;
    }, KEY_VALUE_TABLE);

    // Overflow, if any, lives in the wrapper. The page itself does not grow.
    expect(widths.after).toBe(widths.before);
    expect(widths.wrapperScrollWidth).toBeGreaterThanOrEqual(widths.wrapperClientWidth);
    // Panning a table to its end must not chain into the mobile pane swipe.
    expect(widths.overscrollBehaviorX).toBe('contain');
  });

  test('a table that could grow fits the pane instead of panning', async ({ page }) => {
    const measured = await page.evaluate(() => {
      const sentence = 'the quick brown fox jumps over the lazy dog '.repeat(10);
      const host = document.createElement('div');
      host.className = 'response-content markdown-content';
      host.style.width = '360px';
      host.innerHTML = `
        <div class="table-scroll-wrapper">
          <table>
            <thead><tr><th>Key</th><th>Prose</th></tr></thead>
            <tbody><tr><td>short</td><td>${sentence}</td></tr></tbody>
          </table>
        </div>`;
      document.body.appendChild(host);

      const wrapper = host.querySelector('.table-scroll-wrapper') as HTMLElement;
      const table = host.querySelector('table') as HTMLElement;
      const result = {
        tableWidth: table.getBoundingClientRect().width,
        wrapperWidth: wrapper.clientWidth,
      };
      host.remove();
      return result;
    });

    // A table always fits its pane. Letting one grow and pan was tried and
    // reverted: it cost a full pane of sideways scrolling at every width.
    // Only a token wider than the pane can overflow now, and there is none here.
    expect(measured.tableWidth).toBeLessThanOrEqual(measured.wrapperWidth + 1);
    // It does still use the space it is given.
    expect(measured.tableWidth).toBeGreaterThanOrEqual(measured.wrapperWidth - 1);
  });

  test('a wide table stacks into labeled cards, a narrow one keeps the grid', async ({ page }) => {
    const layout = await page.evaluate(() => {
      const host = document.createElement('div');
      host.className = 'response-content markdown-content';
      host.style.width = '360px';
      host.innerHTML = `
        <div class="table-scroll-wrapper">
          <table data-stack>
            <thead><tr><th>A</th><th>B</th><th>C</th><th>D</th></tr></thead>
            <tbody><tr>
              <td id="t-stacked-cell" data-label="A">w</td>
              <td data-label="B">x</td><td data-label="C">y</td><td data-label="D">z</td>
            </tr></tbody>
          </table>
        </div>
        <div class="table-scroll-wrapper">
          <table>
            <thead><tr><th>E</th><th>F</th></tr></thead>
            <tbody><tr><td id="t-grid-cell">1</td><td>2</td></tr></tbody>
          </table>
        </div>`;
      document.body.appendChild(host);

      const stacked = document.getElementById('t-stacked-cell')!;
      const grid = document.getElementById('t-grid-cell')!;
      const stackedTable = stacked.closest('table')!;
      const result = {
        stackedDisplay: getComputedStyle(stacked).display,
        stackedLabel: getComputedStyle(stacked, '::before').content,
        stackedHeadDisplay: getComputedStyle(stackedTable.querySelector('thead')!).display,
        // A stacked cell is a block, so it spans its row.
        stackedCellWidth: stacked.getBoundingClientRect().width,
        stackedTableWidth: stackedTable.getBoundingClientRect().width,
        gridDisplay: getComputedStyle(grid).display,
        gridLabel: getComputedStyle(grid, '::before').content,
      };
      host.remove();
      return result;
    });

    expect(layout.stackedDisplay).toBe('block');
    expect(layout.stackedHeadDisplay).toBe('none');
    // Chromium RESOLVES attr() in the computed value ("A"), WebKit may report
    // the function verbatim. Either proves the rule applied; 'none' proves it
    // did not.
    expect(layout.stackedLabel).toMatch(/^"A"$|attr\(data-label\)/);
    expect(layout.stackedCellWidth).toBeGreaterThan(layout.stackedTableWidth * 0.9);

    // Below the threshold the table is still a table.
    expect(layout.gridDisplay).toBe('table-cell');
    expect(layout.gridLabel).toBe('none');
  });
});

test.describe('Markdown table columns at desktop width', () => {
  test.use({ viewport: { width: 1280, height: 900 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
  });

  test('a prose-heavy table fits the pane and splits it fairly', async ({ page }) => {
    const measured = await page.evaluate(() => {
      // The reported shape: a short key column beside a long prose one. Built
      // inside the page, since page.evaluate serializes its argument as JSON
      // and cannot carry a helper function across the boundary.
      const row = (key: string, prose: string) =>
        `<tr><td><code>${key}</code></td><td>${prose}</td></tr>`;
      const host = document.createElement('div');
      host.className = 'response-content markdown-content';
      host.style.width = '900px';
      host.innerHTML = `
        <div class="table-scroll-wrapper">
          <table>
            <thead><tr><th>Commit</th><th>What it does</th></tr></thead>
            <tbody>
              ${row(
                'fix(markdown): stop tables squeezing the key column to one character',
                'The core fix. Cells break only between words; the scroll wrapper becomes real; the stray table rules move out of settings/toggle.css and the dead response-content rule leaves mobile.css.'
              )}
              ${row('docs(markdown): correct two comments', 'Comment accuracy.')}
            </tbody>
          </table>
        </div>`;
      document.body.appendChild(host);

      const wrapper = host.querySelector('.table-scroll-wrapper') as HTMLElement;
      const table = host.querySelector('table') as HTMLElement;
      const cells = [...table.querySelectorAll('tbody tr:first-child td')];
      const result = {
        pan: wrapper.scrollWidth - wrapper.clientWidth,
        tableWidth: table.getBoundingClientRect().width,
        col1: cells[0].getBoundingClientRect().width,
        col2: cells[1].getBoundingClientRect().width,
        pageWidth: document.documentElement.scrollWidth,
      };
      host.remove();
      return result;
    });

    // No sideways scrolling at a desktop width either. Growing to twice the
    // pane and panning was tried and reverted: it cost a full pane of scroll.
    expect(measured.pan).toBeLessThanOrEqual(1);
    expect(measured.pageWidth).toBeLessThanOrEqual(1280);
    // And the prose column does not swallow the pane. Uncapped, auto layout
    // gave it 671 of 894px against the key column's 223; the readable-measure
    // cap on cells hands the remainder back, landing both near half.
    expect(measured.col1).toBeGreaterThan(measured.tableWidth * 0.35);
    expect(measured.col2).toBeGreaterThan(measured.tableWidth * 0.35);
  });

  test('a naturally narrow column keeps its natural width', async ({ page }) => {
    const measured = await page.evaluate(() => {
      const host = document.createElement('div');
      host.className = 'response-content markdown-content';
      host.style.width = '900px';
      host.innerHTML = `
        <div class="table-scroll-wrapper">
          <table>
            <thead><tr><th>Check</th><th>OK</th><th>Invariants</th></tr></thead>
            <tbody>
              <tr><td>Markdown transform unit tests</td><td>yes</td>
                  <td>output contract, attribute safety, threshold, and the rest of what the suite covers here</td></tr>
              <tr><td>Every stylesheet parses</td><td>no</td><td>stylesheets parse</td></tr>
            </tbody>
          </table></div>`;
      document.body.appendChild(host);

      const table = host.querySelector('table') as HTMLElement;
      const cells = [...table.querySelectorAll('tbody tr:first-child td')];
      const result = {
        tableWidth: table.getBoundingClientRect().width,
        okColumn: cells[1].getBoundingClientRect().width,
      };
      host.remove();
      return result;
    });

    // The cap is a MAX, not a share: a yes/no column must not be inflated to
    // an equal third. This is exactly what ruled out `table-layout: fixed`,
    // which made a table of this shape 60% taller by equalizing its columns.
    expect(measured.okColumn).toBeLessThan(measured.tableWidth / 4);
  });

  /**
   * shared-components.css is `include_str!`d into /api/v1/sdk-iframe.css, so
   * these rules also reach app iframes, where a hand-written table may have no
   * `.table-scroll-wrapper` around it. With nothing to absorb an overflow, a
   * table permitted to exceed its container would just widen the iframe body,
   * so the growth above is granted to WRAPPED tables only.
   */
  test('an unwrapped table stays inside its container', async ({ page }) => {
    const measured = await page.evaluate(() => {
      const sentence = 'the quick brown fox jumps over the lazy dog '.repeat(40);
      const host = document.createElement('div');
      host.className = 'markdown-content';
      host.style.width = '800px';
      // No .table-scroll-wrapper: the shape an app author writes by hand.
      host.innerHTML = `
        <table>
          <thead><tr><th>Key</th><th>Prose</th></tr></thead>
          <tbody><tr><td>short</td><td>${sentence}</td></tr></tbody>
        </table>`;
      document.body.appendChild(host);

      const table = host.querySelector('table') as HTMLElement;
      const result = {
        tableWidth: table.getBoundingClientRect().width,
        hostWidth: host.clientWidth,
        pageWidth: document.documentElement.scrollWidth,
      };
      host.remove();
      return result;
    });

    expect(measured.tableWidth).toBeLessThanOrEqual(measured.hostWidth + 1);
    expect(measured.pageWidth).toBeLessThanOrEqual(1280);
  });
});
