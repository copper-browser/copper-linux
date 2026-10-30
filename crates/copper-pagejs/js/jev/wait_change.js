// from Sources/Search/Fork/MCP/UltrafastStrategies.swift:348 Ultrafast.waitForChange
// callAsyncJavaScript body on the Mac (`watch`): resolves on the first change outside the trail's layer, or at the cap; the caller keeps the cap too.
async ({ ms }) => {
return await new Promise(resolve => {
  const ours = n => { const e = n && (n.nodeType === 1 ? n : n.parentElement); return !!(e && e.closest && e.closest('#__jev-trail')); };
  const o = new MutationObserver(list => {
    if (list.some(m => !ours(m.target))) { clearTimeout(t); o.disconnect(); setTimeout(() => resolve(true), 30); }
  });
  const t = setTimeout(() => { o.disconnect(); resolve(false); }, ms);
  o.observe(document.documentElement, {subtree: true, childList: true, attributes: true, characterData: true});
});
}
