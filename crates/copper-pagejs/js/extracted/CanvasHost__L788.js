// from Sources/Search/Fork/Canvas/CanvasHost.swift:788 CanvasHost.apply
const out = await window.copperCanvas.apply(input);
return typeof out === 'string' ? out : JSON.stringify(out);
