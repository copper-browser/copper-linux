// from Sources/Search/Fork/MCP/Ultrafast.swift:354 Ultrafast.observe
// callAsyncJavaScript body on the Mac; the caller caps it at 1 s (a navigation can leave it unresolved).
async ({ action }) => {
if (window.__jevFast && window.__jevFast.settle) { return await window.__jevFast.settle(action); } return false;
}
