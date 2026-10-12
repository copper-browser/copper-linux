// from Sources/Search/Fork/Canvas/CanvasHost.swift:902 CanvasHost.checkpoint
// also CanvasHost.swift:868 (compactIfNeeded: the same call without the null check on c)
// callAsyncJavaScript body on the Mac; the body is verbatim, its arguments are destructured.
async ({  }) => {
const c = window.copperCanvas;
if (!c || typeof c.exportState !== 'function') return null;
return await c.exportState();
}
