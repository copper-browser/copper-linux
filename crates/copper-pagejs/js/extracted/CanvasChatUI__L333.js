// from Sources/Search/Fork/Canvas/CanvasChatUI.swift:333 CanvasChat.bench
const p = document.querySelector('[data-testid=canvas-chat]');
const list = document.querySelector('[role=listbox]');
return JSON.stringify({
  panel: !!p, width: p ? Math.round(p.getBoundingClientRect().width) : 0,
  options: list ? [...list.querySelectorAll('[role=option]')].map(o => o.dataset.name) : null,
  invited: list ? [...list.querySelectorAll('[aria-disabled=true]')].map(o => o.innerText.replace(/\s+/g, ' ').trim()) : [],
  chips: p ? [...p.querySelectorAll('[data-mention]')].map(c => ({id: c.dataset.mention, text: c.textContent, mine: c.className.includes('bg-accent ')})) : [],
  badge: (document.querySelector('[data-testid=chat-toggle] span[aria-hidden]') || {}).textContent || null,
  draft: (p && p.querySelector('textarea')) ? p.querySelector('textarea').value : null,
  flash: p ? [...p.querySelectorAll('.chat-flash')].map(e => e.dataset.message) : [],
  scroller: p ? (() => { const l = p.querySelector('[role=log]'); return {top: Math.round(l.scrollTop), height: l.scrollHeight, client: l.clientHeight, wide: l.scrollWidth > l.clientWidth}; })() : null,
});
