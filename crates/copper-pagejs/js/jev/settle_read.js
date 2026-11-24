// from Sources/Search/Fork/MCP/Ultrafast.swift:1246 Ultrafast.observe
// callAsyncJavaScript body on the Mac: the settle and the read in one round trip; null without the helpers (a new document). The caller caps it at 1 s.
async ({ action, goal }) => {
const c = window.__jevFast; if (!c || !c.ready) return null; const s = await c.settle(action); const o = c.observe(goal || undefined, {deep: action.deep}); if (o) o.settle = s; return o;
}
