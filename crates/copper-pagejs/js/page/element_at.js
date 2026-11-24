// from Sources/Search/Fork/MCP/Tools.swift:962 Tools.elementAt
// The element at a viewport point (CSS px), through open shadow roots, as `tag#id "label"`; '' off the viewport.
({ x, y }) => {
  if (x < 0 || y < 0 || x >= innerWidth || y >= innerHeight) return '';
  var h = document.elementFromPoint(x, y);
  while (h && h.shadowRoot) { var i = h.shadowRoot.elementFromPoint(x, y); if (!i || i === h) break; h = i; }
  if (!h) return '';
  var s = h.tagName.toLowerCase() + (h.id ? '#' + h.id : '');
  var l = (h.getAttribute('aria-label') || h.innerText || '').replace(/\s+/g, ' ').trim().slice(0, 60);
  return l ? s + ' "' + l + '"' : s;
}
