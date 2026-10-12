// from Sources/Search/Fork/Canvas/CanvasChecklistBench.swift:72 CanvasChecklistBench.locate
const card = [...document.querySelectorAll('[data-kind="checklist"]')].find(el => el.dataset.id === shape);
if (!card) return JSON.stringify({ error: 'no checklist ' + shape + ' on the board in front' });
const fold = s => s.trim().toLowerCase();
const cells = [...card.querySelectorAll('[data-cell]')];
let rowId = cells.some(c => c.dataset.row === row) ? row : null;
if (!rowId) {
  const named = [...card.querySelectorAll('[role="row"]')].filter(r => {
    const t = r.querySelector('.cl-label-text');
    return t && fold(t.textContent || '') === fold(row);
  });
  if (named.length > 1) return JSON.stringify({ error: 'more than one row is called ' + row + ' — use its id' });
  rowId = named[0]?.querySelector('[data-cell]')?.dataset.row ?? null;
}
if (!rowId) return JSON.stringify({ error: 'no row ' + row + ' in ' + shape });
const cell = cells.find(c => c.dataset.row === rowId && fold(c.dataset.col || '') === fold(column));
if (!cell) return JSON.stringify({ error: 'no column ' + column + ' in ' + shape });
const r = cell.getBoundingClientRect();
const x = r.left + r.width / 2, y = r.top + r.height / 2;
const hit = document.elementFromPoint(x, y);
return JSON.stringify({
  x, y, row: rowId, column: cell.dataset.col,
  checked: cell.getAttribute('aria-checked') === 'true',
  visible: x > 0 && y > 0 && x < innerWidth && y < innerHeight && !!hit && cell.contains(hit),
});
