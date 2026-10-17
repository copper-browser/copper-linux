// from Sources/Search/Fork/Canvas/CanvasPosters.swift:127 CanvasPosters.send
// callAsyncJavaScript body on the Mac (its n, d are name, url here), answering whether the page took it; the arguments are destructured.
async ({ name, url }) => {
const c = window.copperCanvas;
if (!c || typeof c.framePoster !== 'function') return false;
c.framePoster(name, url);
return true;
}
