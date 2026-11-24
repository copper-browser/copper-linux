// from Sources/Search/Fork/MCP/Ultrafast.swift:1246 Ultrafast.observe
// The settle alone (its read is jev/settle_read.js): {ms, capped, why}, or null without the helpers. The caller caps it at 1 s (a navigation can leave it unresolved).
async ({ action }) => {
if (window.__jevFast && window.__jevFast.settle) { return await window.__jevFast.settle(action); } return null;
}
