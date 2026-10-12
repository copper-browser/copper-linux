// from Sources/Search/Fork/MCP/Trail.swift:55 Trail.glide
// callAsyncJavaScript body on the Mac; the caller caps the wait at 400 ms.
async ({ a }) => {
if (window.__jevTrail && window.__jevTrail.glide) { return await window.__jevTrail.glide(a); } return false;
}
