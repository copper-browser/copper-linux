// from Sources/Search/Fork/Canvas/CanvasChat.swift:468 CanvasChat.open
// callAsyncJavaScript body on the Mac; the body is verbatim, its arguments are destructured.
async ({ t }) => {
const c = window.copperCanvas; if (c && typeof c.revealChat === 'function') return c.revealChat(t); return false;
}
