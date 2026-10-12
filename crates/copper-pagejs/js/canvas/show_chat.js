// from Sources/Search/Fork/Canvas/CanvasChat.swift:272 CanvasChat.received
// callAsyncJavaScript body on the Mac; the body is verbatim, its arguments are destructured.
async ({ o }) => {
const c = window.copperCanvas; if (c && typeof c.showChat === 'function') c.showChat(o);
}
