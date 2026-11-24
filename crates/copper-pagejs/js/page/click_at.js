// from Sources/Search/Fork/MCP/Tools.swift:977 Tools.clickAt
// With no real input: the same click as DOM events at the point.
({ x, y }) => {
  var h = document.elementFromPoint(x, y); if (!h) return false;
  var o = {bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, view: window, button: 0};
  ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click'].forEach(function (t) {
    h.dispatchEvent(t.indexOf('pointer') === 0 ? new PointerEvent(t, o) : new MouseEvent(t, o));
  });
  return true;
}
