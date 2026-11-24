// from Sources/Search/Fork/MCP/Tools.swift:613 Tools.dispatch
// Linux: browser_evaluate. `f` is the agent's function, or a plain
// expression, spliced in by page::evaluate. A function is called (with the
// element behind `target`); an expression is its own value. Either way a
// Promise is awaited, and the answer is text: a string as it is, anything
// else as JSON.
async ({ target }, f) => {
  const el = target ? window.__copper.find(target) : undefined;
  if (target && !el) throw new Error(target.slice(4) + ' is not on the page any more; take a fresh browser_snapshot');
  const v = await (typeof f === 'function' ? f(el) : f);
  if (v === undefined) return 'undefined';
  if (typeof v === 'string') return v;
  try { const s = JSON.stringify(v); if (s !== undefined) return s; } catch (e) {}
  return String(v);
}
