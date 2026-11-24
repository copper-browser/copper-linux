// from Sources/Search/Fork/MCP/Ultrafast.swift:634 Ultrafast.script
// Linux: installer, run before every jev/* call (idempotent: cache.ready short-circuits).
() => {
  const cache = window.__jevFast ||= {ids:new WeakMap(), nodes:new Map(), next:1};
  if (cache.ready) return true;
  const identity = e => {
    if (!cache.ids.has(e)) cache.ids.set(e,cache.next++);
    const id=cache.ids.get(e); cache.nodes.set(id,e); return id;
  };
  const safe = e => !['password','file','hidden'].includes(e.type);
  // Secret redaction: a box whose value is a secret (a password box, an
  // autocomplete that promises a password, a code or a card's number or code,
  // or one Copper filled with a secret: window.__copperSecretFields) reads as
  // SECRET, never as itself.
  const SECRET='\u2022\u2022\u2022\u2022 (secret, hidden)';
  const secret = e => { try {
    const kept=e?.ownerDocument?.defaultView?.__copperSecretFields;
    if (kept && typeof kept.has==='function' && kept.has(e)) return true;
    if (e?.tagName==='INPUT' && String(e.getAttribute('type')||'').toLowerCase()==='password') return true;
    return /(^|\s)(current-password|new-password|one-time-code|cc-number|cc-csc)(\s|$)/i.test(e?.getAttribute?.('autocomplete')||'');
  } catch (_) { return false; } };
  const valueOf = e => secret(e) && e.value ? SECRET+'#'+String(e.value).length : e.value;
  // deep (cache.deep, set by each read): the page's open shadow roots and
  // same-origin frames are read too, as browser-use's ax.py does: every
  // root a control can live in, the top document first, 40 at most.
  cache.roots=()=>{
    const out=[document];
    if (!cache.deep) return out;
    for (let i=0;i<out.length && out.length<40;i++) for (const e of out[i].querySelectorAll('*')) {
      if (e.shadowRoot) { if (!secret(e)) out.push(e.shadowRoot); } // Fork (secret-redact): a secret box's inner tree is its value
      else if (e.tagName==='IFRAME' || e.tagName==='FRAME') { try { const d=e.contentDocument; if (d?.body) out.push(d); } catch (_) {} }
      if (out.length>=40) break;
    }
    return out;
  };
  // Where a frame's content sits on the top page: each frame's content box, added up.
  const shift=e=>{ let x=0,y=0;
    for (let w=e.ownerDocument.defaultView; w && w!==window && w.frameElement; w=w.parent) {
      const f=w.frameElement, fr=f.getBoundingClientRect(), cs=getComputedStyle(f);
      x+=fr.x+f.clientLeft+parseFloat(cs.paddingLeft); y+=fr.y+f.clientTop+parseFloat(cs.paddingTop);
    }
    return {x,y}; };
  // The field a person is typing in, through shadow roots and frames.
  const active=()=>{ let a=document.activeElement;
    for (;;) { const n=a?.shadowRoot?.activeElement || (a?.tagName==='IFRAME' ? (()=>{ try { return a.contentDocument?.activeElement; } catch (_) { return null; } })() : null);
      if (!n || n===a) return a; a=n; } };
  const drawn = e => e.checkVisibility({checkOpacity:true,checkVisibilityCSS:true});
  const visible = e => !e.closest('[aria-hidden="true"],[inert]') && drawn(e);
  // A checkbox or radio drawn by its label (the input itself transparent,
  // clipped away or display:none — a styled toggle, a menu built on the
  // checkbox trick): the label is what a finger can hit, so it stands in
  // for the input's place on the page. Anything else is its own face.
  const face = e => {
    if (!e || e.tagName!=='INPUT' || !['checkbox','radio'].includes(e.type)) return e;
    const r=e.getBoundingClientRect();
    if (visible(e) && r.width>=4 && r.height>=4) return e;
    // Only the label's drawing counts: it may be aria-hidden while the
    // input carries the name (Wikipedia's "164 languages" button).
    return [...(e.labels||[])].find(l=>{
      const b=l.getBoundingClientRect(); return drawn(l) && !l.closest('[inert]') && b.width>0 && b.height>0;
    }) || e;
  };
  // Is the control there for a person: itself in the accessibility tree,
  // and its face (itself, or the label standing in for it) drawn.
  const shown = (e,f) => f===e ? visible(e) : !e.closest('[aria-hidden="true"],[inert]') && drawn(f);
  const name = (e,seen=new Set()) => {
    if (!e || seen.has(e)) return '';
    seen.add(e);
    const referenced=(e.getAttribute('aria-labelledby')||'').split(/\s+/)
      .map(id=>name(e.getRootNode().getElementById?.(id),seen)).filter(Boolean).join(' ');
    return referenced || e.getAttribute('aria-label') ||
      [...(e.labels||[])].map(l=>name(l,seen)).filter(Boolean).join(' ') ||
      (['button','submit','reset'].includes(e.type) ? e.value : '') || e.getAttribute('alt') ||
      (e.tagName==='INPUT' ? '' : [...e.childNodes].map(n=>n.nodeType===3 ? n.textContent :
        n.nodeType===1 && n.getAttribute('aria-hidden')!=='true' ? name(n,seen) : '').join(' ').trim()) ||
      e.getAttribute('title') || e.getAttribute('placeholder') || '';
  };
  const roles=['button','link','checkbox','radio','switch','tab','menuitem','menuitemradio',
    'option','gridcell','combobox','textbox','searchbox','spinbutton'];
  const selector='a[href],button,input,textarea,select,summary,[contenteditable="true"],'+
    roles.map(role=>'[role="'+role+'"]').join(',');
  const role = e => {
    const explicit=e.getAttribute('role');
    if (roles.includes(explicit)) return explicit;
    if (e.tagName==='BUTTON' || e.tagName==='SUMMARY') return 'button';
    if (e.tagName==='A') return 'link';
    if (e.tagName==='SELECT') return 'combobox';
    if (e.tagName==='TEXTAREA' || e.isContentEditable) return 'textbox';
    if (e.tagName==='INPUT') {
      if (['checkbox','radio'].includes(e.type)) return e.type;
      if (['button','submit','reset','image'].includes(e.type)) return 'button';
      if (e.type==='search') return 'searchbox';
      if (e.type==='number') return 'spinbutton';
      if (['text','email','url','tel'].includes(e.type)) return 'textbox';
    }
    return null;
  };
  // A control that sends its form: a submit button (or image) inside one.
  const submitter = t => !!t && !!(t.form || t.closest?.('form')) &&
    t.matches('button:not([type]),button[type="submit"],input[type="submit"],input[type="image"]');
  // In a search box: a role=search, or a form whose only text field is a
  // query (its Go button too). Sending it again only looks again.
  const query = t => {
    const box=t.closest?.('form,[role="search"]');
    if (!box) return false;
    if (box.matches('[role="search"]')) return true;
    const fields=[...box.querySelectorAll('input,textarea')].filter(f=>!['hidden','submit','button','image','reset','checkbox','radio'].includes(f.type));
    return fields.length===1 && (fields[0].type==='search' || /^(?:q|s|k|query|search|keywords?|term)$|search|query/i.test(fields[0].name||fields[0].id||''));
  };
  const formState = () => [...document.querySelectorAll('input,textarea,select')].filter(safe)
    .map(e=>[identity(e),valueOf(e),e.checked,e.selectedIndex,e.disabled,e.readOnly]);
  cache.pageKey=()=>JSON.stringify([performance.timeOrigin,location.href,scrollX,scrollY,innerWidth,innerHeight,formState()]);
  cache.guard=e=>{
    if (!e?.isConnected || !shown(e,face(e))) return null;
    const scope=e.closest('form,dialog,[role="dialog"],article,li,tr,[role="row"]') || e.parentElement;
    return JSON.stringify([identity(e),role(e),name(e),valueOf(e)??null,e.checked??null,e.selectedIndex??null,
      e.readOnly??null,e.matches(':disabled'),e.getAttribute('aria-disabled'),
      e.getAttribute('aria-expanded'),e.getAttribute('aria-checked'),e.getAttribute('aria-selected'),
      e.getAttribute('href'),scope?.innerText?.slice(0,6000)||'']);
  };
  // Lowercase, accents off, words split on anything else: "Gödel's" → "godel s".
  const fold=s=>' '+String(s||'').toLowerCase().normalize('NFD').replace(/[\u0300-\u036f]/g,'')
    .replace(/[^\p{L}\p{N}]+/gu,' ').trim()+' ';
  const decoded=s=>{ try { return decodeURIComponent(s); } catch (_) { return s; } };
  // Which of the goal's words a control's words start with, and how
  // much each is worth on this page: a word most links share ("wiki",
  // "javascript" in every href) is worth nothing. Named values (quoted,
  // or a dotted name like Array.prototype.flatMap) count whole.
  const relevance=(items,goal)=>{
    const words=(goal.words||[]).map(w=>' '+w), phrases=(goal.phrases||[]).map(p=>' '+p+' ');
    const df=new Map(words.map(w=>[w,0]));
    for (const it of items) for (const w of words) if (it.text.includes(w) || it.href.includes(w)) df.set(w,df.get(w)+1);
    const n=Math.max(items.length,1);
    for (const it of items) {
      let s=0;
      for (const w of words) {
        const d=df.get(w); if (!d || d>Math.max(3,n*0.2)) continue;
        if (it.text.includes(w)) s+=Math.log(n/d)+1; else if (it.href.includes(w)) s+=(Math.log(n/d)+1)/2;
      }
      // a named value whole, the more the more of the link it is ("Austria" 12, "Duchy of Austria" 4)
      const size=it.text.trim().split(' ').length;
      for (const p of phrases) {
        if (it.text.includes(p)) s+=12*Math.min(1,p.trim().split(' ').length/size);
        else if (it.href.includes(p)) s+=3;
      }
      it.score=s;
    }
  };
  // goal: {words, phrases} from the run (Tools.Ultrafast.goalTerms), or
  // nothing for a plain read. With it, the links the goal names that
  // are above or below the viewport are offered too (a click scrolls
  // them in first): a page's useful link is often a screen or ten away,
  // and a scroll per screen was most of a run (the Wikipedia game).
  // Not a way through for `reach`: a link in the page's footer, a
  // category box or a navbox, or to a namespace page (/wiki/Category:X,
  // Help:, File: — a word and a colon, no space after it). Beethoven's
  // game clicked Category: links three runs of three (r4).
  const furniture=e=>!!e.closest('footer,[role="contentinfo"],.catlinks,.navbox,.mw-footer,.printfooter');
  const namespaced=e=>/\/[A-Za-z]+(?:_talk)?:[^\/_\s]/.test(decoded(e.pathname||''));
  const sponsored=e=>/\/sspa\/|[?&]store_ref=SB|\/gp\/slredirect\/|\/aclk\?/.test(e.href||'') ||
    !!e.closest('[aria-label="Sponsored" i],[data-component-type="sp-sponsored-result"],.AdHolder');
  cache.observe=(goal,opts)=>{
    if (!document.body) return null;
    if (opts) cache.deep=!!opts.deep;
    for (const [id,e] of cache.nodes) if (!e.isConnected) cache.nodes.delete(id);
    const actions=[], away=[], roots=cache.roots();
    for (const root of roots) for (const e of root.querySelectorAll(selector)) {
      const f=face(e);
      if (!safe(e) || !shown(e,f) || e.matches(':disabled') || e.closest('[aria-disabled="true"],[inert]')) continue;
      let r=f.getBoundingClientRect(); const rname=role(e);
      if (!rname || r.width<=0 || r.height<=0) continue;
      // An ad is never the way (Amazon: a sponsored brand-store banner taken for the product, DONE, r6).
      if (rname==='link' && sponsored(e)) continue;
      // In a frame: on that frame's screen, then where that is on the top page.
      const view=e.ownerDocument.defaultView;
      if (view!==window) {
        const lx=r.x+r.width/2, ly=r.y+r.height/2;
        if (lx<0 || ly<0 || lx>=view.innerWidth || ly>=view.innerHeight) continue;
        const o=shift(e); r=new DOMRect(r.x+o.x,r.y+o.y,r.width,r.height);
      }
      const x=r.x+r.width/2, y=r.y+r.height/2;
      if (x<0 || y<0 || x>=innerWidth || y>=innerHeight) {
        if (view!==window) continue;
        const href=e.getAttribute('href')||'';
        if (goal && goal.words && rname==='link' && e.tagName==='A' && x>=0 && x<innerWidth && href && !href.startsWith('#') && !/^javascript:/i.test(href) &&
            !furniture(e) && !namespaced(e)) away.push({e,r});
        continue;
      }
      if (rname==='gridcell' && e.querySelector('button,[role="button"]')) continue;
      const base={node:identity(e),role:rname,label:name(e)||rname,
        rect:{x:r.x,y:r.y,w:r.width,h:r.height}};
      // Where a link goes (two links there are one choice) and whether a
      // button sends its form (never twice without the page answering).
      if (rname==='link' && e.href) base.href=e.href.split('#')[0];
      if (submitter(e)) base.submits=true;
      if (query(e)) base.query=true;
      else if (e.form || e.closest('form')) base.inForm=true;
      for (const key of ['checked','selected','expanded']) {
        const value=e.getAttribute('aria-'+key);
        if (value!==null) base[key]=value;
      }
      if (['checkbox','radio'].includes(e.type)) base.checked=String(e.checked);
      if (e.tagName==='SELECT') {
        for (const o of e.options) if (!o.selected && !o.disabled && !o.closest('optgroup[disabled]'))
          actions.push({...base,kind:'select',value:o.value,
            current_value:[...e.selectedOptions].map(o=>o.label).join(', '),label:base.label+' → '+o.label});
      } else {
        const editable=!e.readOnly && e.getAttribute('aria-readonly')!=='true' &&
          (['textbox','searchbox','spinbutton'].includes(rname) ||
            (rname==='combobox' && ['INPUT','TEXTAREA'].includes(e.tagName)));
        const value='value' in e ? (secret(e) && e.value ? SECRET : String(e.value)) :
          e.isContentEditable || rname==='combobox' ? e.innerText.trim() : '';
        // Which document and which form a field is in: the run's memory
        // of what it typed is kept per field, and "Search" on one form
        // is not "Search" on the next.
        const holder=e.form || e.closest('dialog,[role="dialog"],[role="search"]');
        const scope=editable ? performance.timeOrigin+'|'+(holder ? holder.id || holder.getAttribute('name') ||
          holder.getAttribute('action') || holder.getAttribute('aria-label') || holder.tagName+[...document.forms].indexOf(holder) : '') : undefined;
        actions.push({...base,kind:editable?'fill':'click',value,scope});
        if (editable) actions.push({...base,kind:'click',value,label:'Open '+base.label});
      }
    }
    const words=[]; let length=0;
    for (const root of roots) {
      const doc=root.ownerDocument || root, o=root===document ? {x:0,y:0} : shift(root.host || root.documentElement);
      const walker=doc.createTreeWalker(root.body || root,NodeFilter.SHOW_TEXT), range=doc.createRange(); let node;
      while ((node=walker.nextNode()) && length<6000) {
        const value=node.textContent.trim(), parent=node.parentElement;
        if (!value || !parent || parent.closest('script,style,noscript,template') || !visible(parent)) continue;
        range.selectNodeContents(node); const b=range.getBoundingClientRect(), r={top:b.top+o.y,bottom:b.bottom+o.y,left:b.left+o.x,right:b.right+o.x};
        if (b.width>0 && b.height>0 && r.bottom>0 && r.top<innerHeight && r.right>0 && r.left<innerWidth) {
          words.push(value); length+=value.length;
        }
      }
    }
    const text=words.join('\n').slice(0,6000), height=document.documentElement.scrollHeight;
    const page_key=cache.pageKey(), guards={};
    for (const a of actions) if (!(a.node in guards)) guards[a.node]=cache.guard(cache.nodes.get(a.node));
    // Compare meaning and identity. Geometry is resolved and hit-tested again just before input.
    const semantics=actions.map(({rect,href,...action})=>action);
    const marker=JSON.stringify([performance.timeOrigin,location.href,scrollX,scrollY,innerWidth,innerHeight,
      document.title,text,semantics,formState()]);
    const omitted_actions=Math.max(0,actions.length-250);
    actions.splice(250);
    // Controls by one name that do different things (a row of "Approve",
    // "Delete" per file): each says its row, so the table tells them
    // apart and no second question with a picture is needed. Links to
    // one place are one choice and keep their name.
    const byName=new Map();
    for (const a of actions) if (a.kind==='click') {
      const k=a.role+'|'+String(a.label).trim().toLowerCase();
      if (!byName.has(k)) byName.set(k,[]); byName.get(k).push(a);
    }
    let told=0;
    for (const group of byName.values()) {
      if (group.length<2 || new Set(group.map(a=>a.href||'#'+a.node)).size<2) continue;
      for (const a of group) {
        const e=cache.nodes.get(a.node), row=e?.closest('tr,li,[role="row"],[role="listitem"],article,[role="article"]');
        if (!row || told>=24) continue;
        const said=String(row.innerText||'').replace(/\s+/g,' ').replace(String(a.label).trim(),'').trim().slice(0,40);
        if (said) { a.label=a.label+' · '+said; told++; }
      }
    }
    // Off the marker on purpose: what is on screen decides freshness.
    let relevant=[];
    if (goal && ((goal.words||[]).length || (goal.phrases||[]).length)) {
      const items=[...actions.filter(a=>a.kind==='click').map(a=>({a,text:fold(a.label),href:''})),
        ...away.map(o=>({o,text:fold(o.e.textContent.slice(0,300)),href:fold(decoded((o.e.pathname||'')+' '+(o.e.hash||'')))}))];
      relevance(items,goal);
      relevant=[...new Set(items.filter(it=>it.score>0).map(it=>it.a ? it.a.node : identity(it.o.e)))].slice(0,500);
      const byHref=new Map();
      for (const it of items) if (it.o && it.score>0) {
        const key=it.o.e.href.split('#')[0], have=byHref.get(key);
        const dist=it.o.r.y<0 ? -it.o.r.y : it.o.r.y-innerHeight;
        if (!have || it.score>have.score || (it.score===have.score && dist<have.dist)) byHref.set(key,{...it,dist});
      }
      // No link reads as a value the goal names ("Austria" whole): its
      // way there is a link that only mentions it, so more are offered.
      const named=(goal.phrases||[]).map(p=>' '+p+' ');
      const found=items.some(it=>named.includes(it.text));
      const chosen=[...byHref.values()].sort((a,b)=>b.score-a.score || a.dist-b.dist).slice(0,found || !named.length ? 12 : 30);
      if (!found && named.length) {
        // Nor one that only mentions it: the next links of the text below,
        // in reading order, so a way round (Vienna for Austria) is on offer.
        const have=new Set(chosen.map(it=>it.o.e.href.split('#')[0]));
        let more=0;
        for (const it of items.filter(it=>it.o && it.o.r.y>=innerHeight).sort((a,b)=>a.o.r.y-b.o.r.y)) {
          const key=it.o.e.href.split('#')[0];
          if (have.has(key) || location.href.split('#')[0]===key) continue;
          have.add(key); chosen.push({...it,dist:it.o.r.y-innerHeight});
          if (++more>=12) break;
        }
      }
      for (const it of chosen) {
        const e=it.o.e, r=it.o.r;
        actions.push({node:identity(e),role:'link',label:name(e)||e.textContent.trim().slice(0,90)||'link',
          rect:{x:r.x,y:r.y,w:r.width,h:r.height},kind:'click',value:'',offscreen:r.y<0?'above':'below',href:e.href.split('#')[0]});
        guards[identity(e)]=cache.guard(e);
      }
    }
    actions.forEach((a,i)=>a.id='e'+(i+1));
    // A screen at a time, less a line of overlap to keep one's place.
    const step=Math.max(200,Math.round(innerHeight*0.9));
    if (scrollY+innerHeight<height-2) actions.push({id:'scroll_down',kind:'scroll',label:'Scroll down',delta:step});
    if (scrollY>0) actions.push({id:'scroll_up',kind:'scroll',label:'Scroll up',delta:-step});
    actions.push({id:'wait',kind:'wait',label:'Wait for the page to update'});
    return {url:location.href,title:document.title,w:innerWidth,h:innerHeight,text,
      scroll:{y:scrollY,height},actions,marker,page_key,guards,omitted_actions,relevant};
  };
  cache.marker=()=>{ const s=cache.observe(); return s ? s.marker : null; };
  // An offered link above or below the viewport, brought in before the click.
  cache.reveal=node=>{ const e=cache.nodes.get(node); if (!e?.isConnected) return false;
    e.scrollIntoView({block:'center',inline:'nearest',behavior:'instant'}); return true; };
  cache.check=node=>[cache.pageKey(),cache.guard(cache.nodes.get(node))];
  // Where to put the input, now — or null when the target moved, hid,
  // got covered or (for a dropdown) lost the option. A select is done
  // here in full, since a real click can't open a native popup for us.
  cache.resolve=action=>{
    const e=cache.nodes.get(action.node), f=face(e);
    if (!e?.isConnected || e.matches(':disabled') || e.closest('[aria-disabled="true"],[inert]') ||
        !f.checkVisibility({checkOpacity:true,checkVisibilityCSS:true})) return null;
    if (action.kind==='fill' && (e.readOnly || e.getAttribute('aria-readonly')==='true')) return null;
    const view=e.ownerDocument.defaultView;
    const r=f.getBoundingClientRect(), x=r.x+r.width/2, y=r.y+r.height/2;
    if (!r.width || !r.height || x<0 || y<0 || x>=view.innerWidth || y>=view.innerHeight) return null;
    // The face, or the control drawn over it (a transparent checkbox laid
    // on its label), asked in its own tree: a shadow root or a frame.
    const hit=f.getRootNode().elementFromPoint(x,y);
    // Drawn over (an open search panel, a dialog): said so, since the same click would only be refused again.
    if (!f.contains(hit) && !e.contains(hit)) return hit ? {covered:hit.tagName.toLowerCase()+(hit.id ? '#'+hit.id : '')} : null;
    // In a frame: that frame on screen at that spot of the top page, and the point there.
    let o={x:0,y:0};
    if (view!==window) {
      o=shift(e); let w=view; while (w.parent!==window) w=w.parent;
      const outer=w.frameElement;
      if (x+o.x<0 || y+o.y<0 || x+o.x>=innerWidth || y+o.y>=innerHeight || outer.getRootNode().elementFromPoint(x+o.x,y+o.y)!==outer) return null;
    }
    if (action.kind==='select') {
      if (e.tagName!=='SELECT' || ![...e.options].some(o=>o.value===action.value &&
          !o.disabled && !o.closest('optgroup[disabled]'))) return null;
      e.value=action.value;
      e.dispatchEvent(new Event('input',{bubbles:true}));
      e.dispatchEvent(new Event('change',{bubbles:true}));
    }
    return {x:x+o.x,y:y+o.y};
  };
  // The offered on-screen controls (top document) something else is drawn over, at the spot a click would go.
  cache.covered=nodes=>nodes.filter(n=>{ const e=cache.nodes.get(n); if (!e?.isConnected || e.ownerDocument!==document) return false;
    const f=face(e), r=f.getBoundingClientRect(), x=r.x+r.width/2, y=r.y+r.height/2;
    if (!r.width || !r.height || x<0 || y<0 || x>=innerWidth || y>=innerHeight) return false;
    const hit=f.getRootNode().elementFromPoint(x,y); return !!hit && !f.contains(hit) && !e.contains(hit); });
  cache.selectAll=node=>{
    const e=cache.nodes.get(node); if (!e) return false;
    e.focus();
    if (typeof e.select==='function') { e.select(); return true; }
    if (e.isContentEditable) {
      const d=e.ownerDocument, r=d.createRange(); r.selectNodeContents(e);
      const s=d.getSelection(); s.removeAllRanges(); s.addRange(r); return true;
    }
    return false;
  };
  // For a tab with no window to take real events: the DOM's own click,
  // and a value set the way frameworks notice.
  cache.domClick=node=>{ const e=cache.nodes.get(node); if (!e) return false; e.focus?.(); e.click?.(); return true; };
  cache.setValue=(node,text)=>{
    const e=cache.nodes.get(node); if (!e) return false;
    e.focus();
    if (e.isContentEditable) { e.textContent=text; }
    else {
      const proto=Object.getPrototypeOf(e), d=Object.getOwnPropertyDescriptor(proto,'value');
      if (d && d.set) d.set.call(e,text); else e.value=text;
    }
    e.dispatchEvent(new Event('input',{bubbles:true}));
    e.dispatchEvent(new Event('change',{bubbles:true}));
    return true;
  };
  // Type as one trusted edit, the way upstream's Input.insertText does:
  // select what is there, insert over it. The page hears beforeinput
  // and input as from a keyboard, so an autocomplete opens and a
  // framework keeps its own state — which a value set from script and a
  // made-up event did not always get. The text goes where the click
  // left the caret: a page may answer the click by opening its real
  // field over the one observed (Google Flights' "Where from?") and
  // moving focus there, and that is the field keys would reach. Never a
  // password field. A field that refuses the command has its value set
  // the old way. Returns what the field holds now (the read back), or
  // null when there is no field left to type in.
  cache.insert=(node,text)=>{
    const takes=x=>!!x && x.isConnected && (x.isContentEditable ||
      (['INPUT','TEXTAREA'].includes(x.tagName) && safe(x) && !x.readOnly && !x.disabled &&
        !['checkbox','radio','button','submit','reset','image','range','color'].includes(x.type)));
    let e=active();
    if (!takes(e)) { e=cache.nodes.get(node); if (!e?.isConnected) return null; e.focus(); }
    const read=()=>'value' in e ? String(e.value) : (e.innerText||'');
    const before=read(), d=e.ownerDocument;
    let done=false;
    try {
      if (typeof e.select==='function') e.select();
      else { const r=d.createRange(); r.selectNodeContents(e); const s=d.getSelection(); s.removeAllRanges(); s.addRange(r); }
      done=d.execCommand('insertText',false,text) && (read()!==before || before===text);
    } catch (_) {}
    if (!done) {
      if (e.isContentEditable) e.textContent=text;
      else { const d=Object.getOwnPropertyDescriptor(Object.getPrototypeOf(e),'value'); if (d && d.set) d.set.call(e,text); else e.value=text; }
      e.dispatchEvent(new Event('input',{bubbles:true}));
      e.dispatchEvent(new Event('change',{bubbles:true}));
    }
    return read();
  };
  // A search box that is its form's only field, holding what was just
  // typed, with no list of suggestions under it: the next thing a
  // person does there is press Return.
  cache.loneSearch=node=>{
    const e=cache.nodes.get(node);
    if (!e?.isConnected || !e.form || !e.value || active()!==e) return false;
    // A plain text field too when it is a query box (arXiv's header panel: name "query", "Press Enter to search").
    if (e.type!=='search' && e.getAttribute('role')!=='searchbox' && !(e.type==='text' && query(e))) return false;
    if ([...e.form.elements].some(x=>x!==e && ['INPUT','TEXTAREA','SELECT'].includes(x.tagName) &&
        !['hidden','submit','button','reset','image'].includes(x.type))) return false;
    return ![...document.querySelectorAll('[role="option"]')].some(o=>{
      const r=o.getBoundingClientRect(); return r.width>0 && r.height>0 && visible(o);
    });
  };
  cache.submit=node=>{ const e=cache.nodes.get(node); if (!e?.form) return false; e.form.requestSubmit(); return true; };
  // Before a click or a Return: will it leave this document? A link the
  // page lets go to another address, or a form it lets submit, means a
  // navigation is coming — the next read must wait for the new page, not
  // read this one in the moment before it goes. null until the input has
  // been seen and every handler has had its say (a page that keeps the
  // click, a router, a search drawn in place: false). A target that is
  // no link and submits no form is false at once.
  cache.arm=node=>{
    const t=cache.nodes.get(node), link=t?.closest?.('a[href]');
    // A frame's own navigation is not the tab's: nothing to wait for.
    if (t && t.ownerDocument!==document) { cache.fired=false; cache.leaving=false;
      t.ownerDocument.defaultView.addEventListener('submit',()=>{ cache.fired=true; },{capture:true,once:true}); return true; }
    const submits=!!t && !!(t.form || t.closest?.('form')) &&
      (t.matches('button:not([type]),button[type="submit"],input[type="submit"],input[type="image"]') || ['INPUT','TEXTAREA'].includes(t.tagName));
    cache.fired=false;
    if (!link && !submits) { cache.leaving=false; return true; }
    cache.leaving=null; let sent=null;
    const away=a=>{ if (a.hasAttribute('download') || (a.target && a.target!=='_self')) return false;
      try { const u=new URL(a.href,location.href);
        return /^https?:$/.test(u.protocol) && !(u.origin===location.origin && u.pathname===location.pathname && u.search===location.search);
      } catch (_) { return false; } };
    const decide=e=>setTimeout(()=>{ if (cache.leaving!==null) return;
      const a=e.type==='click' && e.target?.closest?.('a[href]');
      cache.leaving=a ? !e.defaultPrevented && away(a) : sent ? !sent.defaultPrevented : false; },0);
    // fired: a form went in, whether the page then takes it in place or not.
    addEventListener('submit',e=>{ sent=e; cache.fired=true; },{capture:true,once:true});
    addEventListener('click',decide,{capture:true,once:true});
    addEventListener('keydown',decide,{capture:true,once:true});
    return true; };
  // After an input: the page stands still — 33 ms on a page that does
  // not move, the length of the animation on one that slides a dialog
  // shut (Google Flights, 10 runs each way: 16 decisions a run with this,
  // 20 with upstream's flat two frames or 50 ms, the extra ones made on a
  // page still moving and thrown away as stale). An autocomplete field
  // also needs an option showing. 400 ms at most. Read-only, so a
  // navigation mid-way costs nothing.
  //   whole page (classic): from the second frame on, a frame with no
  //     change anywhere since the last and no animation with an end
  //     running anywhere.
  //   near (Fast mode): from the second frame on, no transition left
  //     running on screen — a ripple drawn in a ::before, an animation
  //     that repeats, a colour or a shadow easing in don't count — and
  //     either nothing changed on
  //     screen since the last frame, or two frames in a row read the same
  //     (the marker: what Jev reads, controls and visible text): a page
  //     that keeps flipping classes and hrefs (Google Flights, which sat
  //     at the cap on most clicks) is not waited for when a read can't
  //     tell.
  // Answers how long it took, whether the cap ended it, and what was
  // still moving then.
  // calm (browser-use's ax.py filter): writing an attribute's own value
  // again is no change (Google rewrites a class every frame), and a
  // style or class write counts only when its element had none for
  // 200 ms: a menu shown once is a change, a loop that restyles every
  // frame goes quiet. (Off-screen churn: `near` already skips it.)
  // Answers whether a mutation record is a change; keeps time per element.
  cache.change=calm=>{ const styled=new WeakMap();
    return m=>{ if (!calm || m.type!=='attributes') return true;
      if (m.oldValue===m.target.getAttributeNS(m.attributeNamespace,m.attributeName)) return false;
      if (m.attributeName!=='style' && m.attributeName!=='class') return true;
      const now=performance.now(), was=styled.get(m.target); styled.set(m.target,now);
      return was===undefined || now-was>200; }; };
  cache.settle=action=>new Promise(resolve=>{
    const field=cache.nodes.get(action?.node);
    const autocomplete=action?.kind==='fill' && field?.getAttribute('role')==='combobox';
    const near=!!action?.near, started=performance.now();
    // A list option picked (an airport from the suggestions): the page
    // fills its field and changes its address a frame or two after the
    // list closes, so the form must read the same 2 frames running —
    // two stale decisions a Flights run (r4) were made in between.
    const steady=near && action?.kind==='click' && action?.role==='option';
    let frames=0, dirty=false, stopped=false, last=[], seen=null, reads=0, key=null, same=0;
    const ours=n=>{ const e=n && (n.nodeType===1 ? n : n.parentElement); return !!(e && e.closest && e.closest('#__jev-trail')); };
    const calm=!!action?.calm, real=cache.change(calm);
    if (action && 'deep' in action) cache.deep=!!action.deep;
    const desc=e=>!e || !e.tagName ? '?' : e.tagName.toLowerCase()+(e.id ? '#'+e.id : '')+(e.classList && e.classList[0] ? '.'+e.classList[0] : '');
    const onScreen=n=>{ const e=n && (n.nodeType===1 ? n : n.parentElement); if (!e || !e.isConnected) return false;
      const r=e.getBoundingClientRect(); return r.width>0 && r.height>0 && r.bottom>0 && r.right>0 && r.top<innerHeight && r.left<innerWidth; };
    const counts=m=>!ours(m.target) && (onScreen(m.target) || [...m.addedNodes].some(n=>n.nodeType===1 && onScreen(n)));
    const watcher=new MutationObserver(list=>{
      // real() first and once per record: it keeps time per element.
      const seen=near ? list.filter(real).slice(0,40).filter(counts) : list.filter(m=>real(m) && !ours(m.target));
      if (seen.length) { dirty=true; last=seen.slice(0,2).map(m=>'mut '+desc(m.target.nodeType===1 ? m.target : m.target.parentElement)+' '+(m.attributeName||m.type)); }
    });
    for (const root of cache.roots()) watcher.observe(root===document ? document.documentElement : root,
      {subtree:true,childList:true,attributes:true,characterData:true,attributeOldValue:calm});
    // Only colours and shadows easing: nothing a read or a click sees.
    const looks=a=>{ const p=a.transitionProperty; if (p) return /color|shadow/i.test(p);
      try { const keys=a.effect.getKeyframes().flatMap(k=>Object.keys(k)).filter(k=>!['offset','easing','composite','computedOffset'].includes(k));
        return keys.length>0 && keys.every(k=>/color|shadow/i.test(k)); } catch (_) { return false; } };
    // A spinner turns forever; only an animation with an end is waited for.
    const moving=()=>{ try { return document.getAnimations().filter(a=>{
      if (a.playState!=='running' || ours(a.effect?.target)) return false;
      const t=a.effect?.getComputedTiming();
      if (!t || !Number.isFinite(t.endTime)) return false;
      return !near || (!a.effect.pseudoElement && t.iterations<=1 && !looks(a) && onScreen(a.effect.target));
    }); } catch (_) { return []; } };
    const finish=capped=>{ if (stopped) return; stopped=true; watcher.disconnect();
      let why='';
      if (capped) { const m=moving(); why=[...last,...m.slice(0,2).map(a=>'anim '+desc(a.effect?.target)+(a.effect?.pseudoElement||'')+' '+(a.animationName||a.transitionProperty||'')+' x'+a.effect?.getComputedTiming().iterations),
        ...(reads ? [reads+' reads'] : [])].join('; ').slice(0,300); }
      resolve({ms:Math.round(performance.now()-started),capped,why}); };
    setTimeout(()=>finish(true),400);
    const ready=()=>{
      if (stopped) return;
      frames++;
      const clean=!dirty; dirty=false;
      const ids=(field?.getAttribute('aria-controls')||field?.getAttribute('aria-owns')||'')
        .split(/\s+/).filter(Boolean);
      const roots=ids.length ? ids.map(id=>field.getRootNode().getElementById?.(id)).filter(Boolean) : [document];
      const options=roots.flatMap(root=>[...root.querySelectorAll('[role="option"]')]);
      let still;
      // Never the first frame: a click's handlers start their work (a
      // dialog closing, a field filled in) a frame or so after it lands.
      if (!near) still=clean && frames>=2 && !moving().length;
      else if (frames<2 || moving().length) still=false;
      else if (clean) still=true;
      else { const m=cache.marker(); reads++; still=m!==null && m===seen; seen=m; }
      if (steady) { const k=cache.pageKey(); same=k===key ? same+1 : 0; key=k; if (same<2) still=false; }
      if (still && (!autocomplete || options.some(e=>{
        const r=e.getBoundingClientRect();
        return r.width && r.height && r.bottom>0 && r.top<innerHeight &&
          e.checkVisibility({checkOpacity:true,checkVisibilityCSS:true});
      }))) finish(false);
      else { if (autocomplete && !last.length) last=['options']; requestAnimationFrame(ready); }
    };
    requestAnimationFrame(ready);
  });
  cache.ready=true;
  return true;
}
