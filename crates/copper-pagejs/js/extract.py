#!/usr/bin/env python3
"""Regenerate the source-accurate JavaScript extraction for PLAN T-006.

Only reads copper-ref; writes .js assets and INVENTORY.md beside this script.
Explicit source-line allowlists deliberately exclude SQL, shell and prose literals.
"""
from __future__ import annotations

import html
import re
from pathlib import Path

SOURCE = Path('/Users/felipearce/src/copper-ref/Sources/Search')
HERE = Path(__file__).resolve().parent
OUT = HERE / 'extracted'

# Literal-opening lines (not references to the literal). The dynamic Swift
# interpolation remains a /*INTERP: ...*/ placeholder for the later Rust port.
MULTILINE = {
    'Bench.swift': [907],
    'Curtain.swift': [148, 163],
    'ExtensionPopup.swift': [152, 231],
    'ExtensionShims.swift': [173, 2369, 2380],
    'Float.swift': [490, 566, 578, 587, 597],
    'Fork/Canvas/CanvasBench.swift': [287, 300, 331],
    'Fork/Canvas/CanvasChatUI.swift': [246, 316, 327, 333],
    'Fork/Canvas/CanvasChecklistBench.swift': [72],
    'Fork/Canvas/CanvasFrames.swift': [114],
    'Fork/Canvas/CanvasHost.swift': [658, 788, 801, 868, 902, 1109, 1144],
    'Fork/Canvas/CanvasImport.swift': [243],
    'Fork/Canvas/CanvasLean.swift': [78],
    'Fork/MCP/Page.swift': [138],
    'Fork/MCP/Probe.swift': [22],
    'Fork/MCP/Trail.swift': [146],
    'Fork/MCP/Ultrafast.swift': [132],
    'Fork/Passkeys/Passkeys.swift': [22],
    'Fork/StorageImport.swift': [169],
    'Forms.swift': [90, 102],
    'Icons.swift': [238],
    'ImageMenu.swift': [32],
    'Reader.swift': [11],
    'StoreRelay.swift': [40],
    'Swipe.swift': [33, 45],
    'Tab.swift': [1395, 1597],
}
SINGLE = {
    'Bench.swift': [382, 755],
    'Browser.swift': [198],
    'ExtensionPopup.swift': [202, 255, 265],
    'Fork/Agent/Agent.swift': [329],
    'Fork/AnimatedBackdrop.swift': [323, 345],
    'Fork/Canvas/CanvasChat.swift': [205, 218, 254, 272, 468],
    'Fork/Canvas/CanvasChatUI.swift': [281, 304, 311, 312, 313],
    'Fork/Canvas/CanvasFrames.swift': [98, 106],
    'Fork/Canvas/CanvasHost.swift': [614, 711, 742, 821, 827, 835, 984, 998, 1034, 1062],
    'Fork/MCP/Page.swift': [38, 73, 82, 110],
    'Fork/MCP/Tools.swift': [310, 320, 334, 338, 341, 349, 360, 370, 377, 390, 405, 407, 418, 424, 439, 444, 447, 451],
    'Fork/MCP/Trail.swift': [55, 106, 126, 140],
    'Fork/MCP/Ultrafast.swift': [320, 354, 365, 377, 385, 688, 691, 712, 721, 725, 731, 1038],
    'Fork/StorageImport.swift': [162],
    'Tab.swift': [486, 487, 546, 566, 574, 582, 599, 613, 627, 637, 647, 658, 668, 682, 687, 813, 901, 966, 1015, 1376],
}


def interpolation(text: str, start: int) -> tuple[str, int]:
    """Swift interpolation expression through its balancing ')' (nested strings allowed)."""
    depth = 1
    quoted = False
    escaped = False
    at = start
    while at < len(text):
        c = text[at]
        if quoted:
            if escaped:
                escaped = False
            elif c == '\\':
                escaped = True
            elif c == '"':
                quoted = False
        elif c == '"':
            quoted = True
        elif c == '(':
            depth += 1
        elif c == ')':
            depth -= 1
            if depth == 0:
                return text[start:at], at + 1
        at += 1
    raise ValueError(f'Unclosed Swift interpolation at {start}: {text[start:start+100]}')


def swift_value(body: str, hashes: int) -> str:
    out = []
    at = 0
    while at < len(body):
        if body[at] != '\\':
            out.append(body[at])
            at += 1
            continue
        marker = '\\' + '#' * hashes + '('
        if body.startswith(marker, at):
            expr, at = interpolation(body, at + len(marker))
            out.append('/*INTERP: ' + expr.strip() + '*/')
        elif not hashes and at + 1 < len(body):
            special = {'\\': '\\', '"': '"', "'": "'", 'n': '\n', 'r': '\r', 't': '\t', '0': '\0'}
            if body[at + 1] in special:
                out.append(special[body[at + 1]])
                at += 2
            else:
                # Unrecognised escape: retain source spelling instead of
                # silently changing JavaScript (a Rust-port review can flag it).
                out.append('\\' + body[at + 1])
                at += 2
        else:
            out.append('\\')
            at += 1
    return ''.join(out)


def multiline(source: str, line: int) -> str:
    lines = source.splitlines(keepends=True)
    prefix = ''.join(lines[:line-1])
    startline = lines[line-1]
    hit = re.search(r'(#{0,3})"""', startline)
    assert hit, (line, startline)
    hashes = len(hit.group(1))
    start = len(prefix) + hit.end()
    # In these JS literals the closing delimiter is on a line of its own.
    close = re.search(r'(?m)^([ \t]*)"""' + '#' * hashes, source[start:])
    assert close, (line, source[start:start+100])
    raw = source[start:start+close.start()]
    assert raw.startswith('\n'), (line, raw[:80])
    raw = raw[1:]
    indent = close.group(1)
    if indent:
        raw = ''.join(t[len(indent):] if t.startswith(indent) else t for t in raw.splitlines(keepends=True))
    # Swift excludes the newline before its closing delimiter.
    if raw.endswith('\n'):
        raw = raw[:-1]
    return swift_value(raw, hashes)


def quoted_literal(text: str, at: int) -> str:
    """Read the top-level Swift string following `at`, skipping nested interpolation."""
    match = re.search(r'(#{0,3})"(?!")', text[at:])
    assert match, text[at:]
    hashes = len(match.group(1))
    at += match.end()
    out = []
    while at < len(text):
        if text.startswith('"' + '#' * hashes, at):
            return swift_value(''.join(out), hashes)
        marker = '\\' + '#' * hashes + '('
        if text.startswith(marker, at):
            expr, at = interpolation(text, at + len(marker))
            # Keep the interpolation in source spelling so swift_value can
            # replace it in exactly one pass (including raw # strings).
            out.append(marker + expr + ')')
        elif not hashes and text[at] == '\\' and at + 1 < len(text):
            out.append(text[at:at+2]); at += 2
        else:
            out.append(text[at]); at += 1
    raise ValueError(f'Unclosed Swift string: {text[:200]}')


def single(source: str, line: int) -> str:
    text = source.splitlines()[line-1]
    # Keep only the argument or assignment containing JS, not Swift dictionary
    # keys, case labels, or string literals nested in interpolation.
    anchors = ('Page.raw(', 'Page.js(', 'js(web, ', 'page(host, ',
               'evaluateJavaScript(', 'callAsyncJavaScript(', 'script =', 'js =',
               'let js =', '.post(', 'post(', '.call(', 'call(', 'raw(web, ', '?? ')
    if line == 755 and source.splitlines()[line-1].find('?? ') >= 0:
        text = text[text.find('?? ')+3:]
    elif line in (405, 407) and 'script =' in text:
        text = text[text.find('script =') + len('script ='):]
    elif line in (311, 312, 313) and 'js =' in text:
        text = text[text.find('js =') + len('js ='):]
    elif 'let js =' in text:
        text = text[text.find('let js =') + len('let js ='):]
    elif 'post(' in text and ('CanvasHost.swift' in source[:0] or '.post(' in text):
        text = text[text.find('post(') + len('post('):]
    else:
        matches = [(text.find(anchor), anchor) for anchor in anchors if text.find(anchor) >= 0]
        if matches:
            pos, anchor = min(matches)
            text = text[pos + len(anchor):]
    return quoted_literal(text, 0)


def symbol(source: str, line: int, path: str) -> str:
    above = source.splitlines()[:line]
    owner = ''
    for row in reversed(above):
        m = re.match(r'^\s*(?:(?:private|fileprivate|final|public|internal)\s+)*(?:class|enum|struct|extension)\s+(\w+)', row)
        if m:
            owner = m.group(1)
            break
    # Inner model declarations (Failure, PageSocket, Step, SmartZoom) are
    # nested in the actual script host and must not replace its symbol.
    owners = {
        'Bench.swift': 'Bench', 'Browser.swift': 'Browser',
        'ExtensionShims.swift': 'ExtensionShims', 'Icons.swift': 'Favicons',
        'Fork/Agent/Agent.swift': 'Agent',
        'Fork/AnimatedBackdrop.swift': 'BackdropWeb',
        'Fork/Canvas/CanvasChat.swift': 'CanvasChat',
        'Fork/Canvas/CanvasHost.swift': 'CanvasHost',
        'Fork/Canvas/CanvasImport.swift': 'CanvasImport',
        'Fork/StorageImport.swift': 'StorageImport',
    }
    owner = owners.get(path, owner)
    if path.startswith('Fork/MCP/'):
        owner = Path(path).stem
    if path == 'Tab.swift':
        owner = 'Tab' if line < 1186 else ('PageView' if line < 1575 else 'ScrollRelay')
    if path == 'Fork/Canvas/CanvasChatUI.swift':
        owner = 'CanvasChat'
    first = source.splitlines()[line-1]
    static = re.match(r'^\s*(?:(?:private|fileprivate|nonisolated)\s+)*static\s+(?:let|var)\s+(\w+)\s*=', first)
    if static:
        member = static.group(1)
    else:
        member = ''
        for row in reversed(above):
            # Members are indented exactly once; skip nested local helpers.
            m = re.match(r'^    (?:(?:private|fileprivate|nonisolated|static|override|mutating|@MainActor)\s+)*func\s+(\w+)\s*\(', row)
            if m:
                member = m.group(1)
                break
    return '.'.join(x for x in (owner, member) if x) or f'line {line}'


def feature(path: str, line: int, body: str) -> str:
    stem = Path(path).stem
    if path == 'Fork/Canvas/Resources/canvas.html': return 'canvas app/bridge (bundled inline module)'
    if path.endswith('.js'): return 'animated backdrop module / three.js dependency'
    if stem == 'Tab':
        if line in (1395, 1376): return 'smart zoom'
        if line in (1597,): return 'scroll position relay'
        if line in (486,487,682,687): return 'curtain/hide picker'
        if line == 901: return 'extension store button'
        if line in (966,1015): return 'navigation readiness'
        return 'forms/password detection & fill' if line < 900 else 'page state'
    if stem == 'Forms': return 'forms/password detection & fill'
    if stem in ('Curtain',): return 'curtain/hide picker & persistent CSS'
    if stem == 'Reader': return 'reader mode'
    if stem == 'Page': return 'snapshot/refs, get_text, find, console capture & click/type helpers' if line == 138 else ('snapshot/refs' if 'snapshot(' in body or 'rect(' in body else 'MCP page helper probe')
    if stem == 'Tools':
        for keyword, tag in [('findText','find'), ('.text(', 'get_text'), ('.fill(', 'fill'), ('.setValue(', 'native-setter typing'), ('.console(', 'console capture'), ('.scroll(', 'scroll'), ('.click(', 'click'), ('.hover(', 'hover'), ('.select(', 'select'), ('.drag(', 'drag')]:
            if keyword in body: return 'MCP ' + tag
        return 'MCP page action/evaluation' if line not in (418,424) else 'MCP wait for text'
    if stem == 'Ultrafast': return 'Jev fast page observation & actions' if line == 132 else 'Jev action/probe'
    if stem == 'Probe': return 'perf probe'
    if stem == 'Trail': return 'agent trails/visual cursor'
    if stem in ('CanvasHost','CanvasFrames','CanvasLean','CanvasChat','CanvasChatUI','CanvasChecklistBench','CanvasImport','CanvasBench'):
        if 'Chat' in stem: return 'canvas chat'
        if stem == 'CanvasFrames': return 'canvas frame bridge'
        if stem == 'CanvasLean': return 'canvas typing relay'
        if 'Bench' in stem: return 'canvas perf/diagnostics'
        if stem == 'CanvasImport': return 'canvas legacy import'
        return 'canvas bridge'
    if stem == 'ExtensionShims': return 'extension shims/user scripts'
    if stem == 'ExtensionPopup': return 'extension popup sizing'
    if stem == 'Passkeys': return 'passkey shim'
    if stem == 'StorageImport': return 'import localStorage'
    if stem == 'Float': return 'picture-in-picture isolate'
    if stem == 'AnimatedBackdrop': return 'animated backdrop'
    if stem == 'StoreRelay': return 'extension store button'
    if stem == 'Swipe': return 'horizontal scroll/swipe arbitration'
    if stem == 'ImageMenu': return 'image right-click relay'
    if stem == 'Icons': return 'favicon detection'
    if stem == 'Bench': return 'test-bench page interaction'
    if stem == 'Agent': return 'agent get_text'
    if stem == 'Browser': return 'clear selection'
    return 'page JS'


def metadata(path: str, line: int, body: str) -> tuple[str, str, str, str]:
    stem = Path(path).stem
    when, world = 'eval', 'page'
    if path.endswith('.js') or path.endswith('.html'):
        when, world = 'bundled module (page load)', 'page (bundled file)'
    if stem == 'CanvasFrames' and line == 114:
        when, world = 'documentStart (all frames)', 'isolated: CopperCanvasFrames'
    if stem == 'CanvasLean' and line == 78: when = 'documentStart (main frame)'
    if stem == 'Tab' and line == 1597: when = 'documentEnd (main frame)'
    if stem == 'Curtain' and line in (148, 163):
        when = 'documentStart (main frame)' if line == 163 else 'documentStart (main frame); eval on update'
    if stem == 'Forms' and line in (90,102): when = 'documentStart (all frames)' if line == 90 else 'documentEnd (main frame)'
    if stem == 'Swipe' and line in (33,45): when = 'documentStart (main frame)' if line == 33 else 'documentStart (all frames)'
    if stem == 'ImageMenu' and line == 32: when = 'documentStart (all frames)'
    if stem == 'StoreRelay' and line == 40: when = 'documentEnd (main frame)'
    if stem == 'Passkeys' and line == 22: when = 'documentStart (all frames)'
    if stem == 'ExtensionShims':
        when, world = (('extension script load (background/content)', 'extension runtime') if line == 173
                       else ('extension user script run_at (configured)', 'MAIN or USER_SCRIPT (configured)'))
    handlers = []
    for name in ('officeScroll','officeVeil','officeImages','officeStore','officeForms'):
        if re.search(r'messageHandlers\.'+name+r'\.postMessage\(', body): handlers.append(name)
    if stem == 'Tab' and line == 1597: handlers = ['officeScroll']
    if stem == 'CanvasFrames' and line == 114: handlers = ['copperFrame']
    if stem == 'CanvasLean' and line == 78: handlers = ['officeForms']
    if stem == 'Passkeys' and line == 22: handlers = ['copperPasskeys']
    if path == 'Fork/Canvas/Resources/canvas.html': handlers = ['canvas']
    payload = {'officeScroll': '{y:number,max:number} | {side:"taken"|"free"}',
               'officeVeil': '{selector,label,note} | {off:true} | {trouble:string}',
               'officeImages': '{src:string}', 'officeStore': '{placed:pageID} | {add:true}',
               'officeForms': '{kind:"form"|"submit"|"settled"|"focus"|"fullscreen", user?,password?,typing?,rect?,field?,hint?,on?}',
               'copperPasskeys': '{kind:"create"|"get",origin,topOrigin,crossOrigin,rpId,mediation,requestId,publicKey…} (promise reply)',
               'copperFrame': '{kind:"nav"|"escape"|"blank",name,url?,title?,icon?}',
               'canvas': '{type:"ready"|"update"|"selection"|"share"|"presence"|"ws"|"wsOpen"|"wsClose"|"openUrl"|"log"|"chat"|"chatRead"|"chatMembers"|"mention", b64?,ids?,people?,url?,msg?…}'}
    posts = '; '.join(f'`{name}` {payload[name]}' for name in handlers) or '—'
    needs = 'no' if stem in ('ExtensionShims','ExtensionPopup','Passkeys','Float','AnimatedBackdrop','Bench','CanvasBench','CanvasChecklistBench','CanvasImport','StoreRelay') or path.endswith('/Backdrop/scene.js') or path.endswith('/Backdrop/three.module.min.js') else 'yes'
    if stem == 'Tab' and line == 901: needs = 'no (extension store)'
    if stem == 'Forms' and line == 90: needs = 'no (Apple passkeys workaround)'
    if stem == 'CanvasChatUI' and line >= 260: needs = 'no (bench only)'
    return when, world, posts, needs


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    files = []
    for kind, selected in (('multi', MULTILINE), ('single', SINGLE)):
        for path, lines in selected.items():
            source = (SOURCE/path).read_text()
            for line in lines:
                body = multiline(source, line) if kind == 'multi' else single(source, line)
                assert body, (path, line)
                files.append((path, line, symbol(source, line, path), body))
    for path in ('Fork/Backdrop/scene.js', 'Fork/Backdrop/three.module.min.js'):
        files.append((path, 1, 'resource module', (SOURCE/path).read_text()))
    canvas_path = 'Fork/Canvas/Resources/canvas.html'
    canvas = (SOURCE/canvas_path).read_text()
    opener = re.search(r'<script type="module" crossorigin>', canvas)
    assert opener
    end = canvas.rfind('</script>')
    assert end > opener.end()
    files.append((canvas_path, canvas[:opener.start()].count('\n')+1, 'inline module/bridge', canvas[opener.end():end]))
    rows = []
    for path, line, name, body in sorted(files):
        basename = Path(path).stem
        filename = f'{basename}__L{line}.js'
        assert not (OUT/filename).exists() or (OUT/filename).read_text().startswith('// from Sources/Search/'+path+':'+str(line)+' '), filename
        text = f'// from Sources/Search/{path}:{line} {name}\n' + body
        (OUT/filename).write_text(text if text.endswith('\n') else text + '\n')
        when, world, posts, needs = metadata(path, line, body)
        f = feature(path, line, body)
        posts = posts.replace('|', '\\|')
        rows.append(f'| [`extracted/{filename}`](extracted/{filename}) | `{path}:{line}` `{name}` | {f} | {when} | {world} | {posts} | {needs} |')
    message_rows = [
        ('officeScroll', 'Tab.swift:350', 'ScrollRelay.name', 'page', '{y,max} or {side:"taken"|"free"}', 'yes'),
        ('officeVeil', 'Tab.swift:351', 'VeilRelay.name', 'page', '{selector,label,note}, {off:true}, {trouble:string}', 'yes'),
        ('officeImages', 'Tab.swift:352', 'ImageRelay.name', 'page', '{src:string}', 'yes'),
        ('officeStore', 'Tab.swift:353', 'StoreRelay.name', 'page', '{placed:pageID} or {add:true}', 'no (extension store)'),
        ('officeForms', 'Tab.swift:354', 'FormRelay.name', 'page', '{kind, user?,password?,typing?,rect?,field?,hint?,on?}', 'yes'),
        ('copperPasskeys', 'Tab.swift:355', 'Passkeys.name', 'page', '{kind,origin,rpId,requestId,publicKey…}, reply', 'no (passkeys)'),
        ('canvas', 'Fork/Canvas/CanvasHost.swift:301,320', 'CanvasHost.handler', 'page (bundled main frame)', '{type, b64?,ids?,people?,url?,msg?…} or JSON string', 'yes'),
        ('copperFrame', 'Fork/Canvas/CanvasFrames.swift:67', 'CanvasHost.frameHandler', 'isolated CopperCanvasFrames', '{kind,name,url?,title?,icon?}', 'yes'),
    ]
    inventory = '''# Copper page JavaScript — PLAN T-006 extraction inventory

Source of truth: `/Users/felipearce/src/copper-ref/Sources/Search/` (read-only). This is **extraction only**, not the Rust port; regenerate via `python3 js/extract.py` after Swift changes. One row per static literal/call site (even repeated bodies), plus the bundled `.js` resources and canvas HTML's inline module. The header names the *literal opening line*, not the Swift call site for shared constants. `/*INTERP: expr*/` replaces each Swift `\\(expr)` or raw `\\#(expr)` interpolation; these templates are **not executable until their placeholders are substituted**. Ordinary Swift string escapes are decoded to the JavaScript body seen by WebKit; multiline indentation is stripped as Swift does. Dynamic user-supplied `browser_evaluate`, bench `js`, and third-party extension user code cannot have static bodies extracted.

`eval` means `evaluateJavaScript` or `callAsyncJavaScript` (the latter is a function body, not a standalone expression); unless noted, world is WebKit's `.page`. Shared `Page.helper`, `Ultrafast.script`, and `Trail.script` are installed lazily with eval, not at documentStart. A bundled module is loaded by its HTML page, not a WKUserScript. The macOS-specific animated backdrop, passkeys, extension shims, and PiP float do not block Linux v1.

## Scripts

| Extracted file | Swift source:line / symbol | Feature | Injection time | World | Posts to handler / payload | Linux v1? |
| --- | --- | --- | --- | --- | --- | --- |
''' + '\n'.join(rows) + '''

## Script message handlers (every registration)

| Registered name | Swift registration | Constant | World | Accepted page payload | Linux v1? |
| --- | --- | --- | --- | --- | --- |
''' + '\n'.join('| `' + n + '` | `' + where + '` | `' + constant + '` | ' + world + ' | `' + payload.replace('`','').replace('|','\\|') + '` | ' + needs + ' |' for n, where, constant, world, payload, needs in message_rows) + '''

Notes: `officeScroll` carries both `ScrollRelay.script` positions and `Swipe.watch` ownership reports. `CanvasLean.typing` posts `officeForms` even though the lean board removes other ordinary page handlers. The canvas `canvas` bridge accepts a dictionary or a JSON string and only accepts messages from the bundled main frame. `copperFrame` is registered in its named isolated content world and reports only direct HTTP(S) subframes. No voice-specific page JS or message handler was found under `Sources/Search/Fork/Voice/`.
'''
    interpolated = sum('/*INTERP:' in body for _, _, _, body in files)
    swift_sources = len(set(path for path, _, _, _ in files if path.endswith('.swift')))
    counts = (f'**Coverage:** {len(files)} extracted scripts across {swift_sources} Swift files, '
              f'2 bundled `.js` resources and 1 inline HTML module; '
              f'{interpolated} interpolation-bearing templates; '
              f'{len(message_rows)} distinct script message handler names.\n')
    inventory = inventory.replace('\n## Scripts\n', '\n' + counts + '\n## Scripts\n', 1)
    (HERE/'INVENTORY.md').write_text(inventory)
    print(f'Extracted {len(files)} JS assets ({interpolated} with Swift interpolation) across {len(set(x[0] for x in files))} source files; {len(message_rows)} distinct handlers')

if __name__ == '__main__':
    main()
