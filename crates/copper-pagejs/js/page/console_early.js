// from Sources/Search/Fork/MCP/Page.swift:138 Page.helper
// Linux: the helper's console capture on its own, as a document-start user
// script (page world, main frame), so what a page logs while it loads is kept
// too. Same entries ({level, text}), same text, same 200-line cap as the
// helper; page/helper.js adopts window.__copperLogs instead of hooking again.
// The Mac starts capturing at the first page-tool call.
function () {
  if (window.__copperLogs) return;
  var logs = [];
  Object.defineProperty(window, '__copperLogs', { value: logs });
  ['log', 'warn', 'error', 'info'].forEach(function (level) {
    var original = console[level];
    console[level] = function () {
      try { logs.push({ level: level, text: Array.prototype.map.call(arguments, function (a) { try { return typeof a === 'string' ? a : JSON.stringify(a); } catch (e) { return String(a); } }).join(' ') }); if (logs.length > 200) logs.shift(); } catch (e) {}
      return original.apply(console, arguments);
    };
  });
  window.addEventListener('error', function (e) { logs.push({ level: 'error', text: String(e.message) }); });
}
