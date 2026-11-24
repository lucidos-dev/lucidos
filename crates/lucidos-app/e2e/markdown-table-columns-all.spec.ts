import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy } from './helpers';

test.use({ viewport: { width: 390, height: 844 } });


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

    expect(widths.after).toBe(widths.before);
    expect(widths.wrapperScrollWidth).toBeGreaterThanOrEqual(widths.wrapperClientWidth);
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

    expect(measured.tableWidth).toBeLessThanOrEqual(measured.wrapperWidth + 1);
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
    expect(layout.stackedLabel).toMatch(/^"A"$|attr\(data-label\)/);
    expect(layout.stackedCellWidth).toBeGreaterThan(layout.stackedTableWidth * 0.9);

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

    expect(measured.pan).toBeLessThanOrEqual(1);
    expect(measured.pageWidth).toBeLessThanOrEqual(1280);
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

    expect(measured.okColumn).toBeLessThan(measured.tableWidth / 4);
  });

  test('an unwrapped table stays inside its container', async ({ page }) => {
    const measured = await page.evaluate(() => {
      const sentence = 'the quick brown fox jumps over the lazy dog '.repeat(40);
      const host = document.createElement('div');
      host.className = 'markdown-content';
      host.style.width = '800px';
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
