// from Sources/Search/Fork/Canvas/CanvasChat.swift:205 CanvasChat.tell
// also CanvasChat.swift:218 and :254 (the same call)
// callAsyncJavaScript body on the Mac; the body is verbatim, its arguments are destructured.
async ({ s }) => {
const c = window.copperCanvas; if (c && typeof c.setChat === 'function') c.setChat(s);
}
