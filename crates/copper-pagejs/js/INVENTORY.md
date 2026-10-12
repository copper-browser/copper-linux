# Copper page JavaScript — PLAN T-006 extraction inventory

Source of truth: `/Users/felipearce/src/copper-ref/Sources/Search/` (read-only). This is **extraction only**, not the Rust port; regenerate via `python3 js/extract.py` after Swift changes. One row per static literal/call site (even repeated bodies), plus the bundled `.js` resources and canvas HTML's inline module. The header names the *literal opening line*, not the Swift call site for shared constants. `/*INTERP: expr*/` replaces each Swift `\(expr)` or raw `\#(expr)` interpolation; these templates are **not executable until their placeholders are substituted**. Ordinary Swift string escapes are decoded to the JavaScript body seen by WebKit; multiline indentation is stripped as Swift does. Dynamic user-supplied `browser_evaluate`, bench `js`, and third-party extension user code cannot have static bodies extracted.

`eval` means `evaluateJavaScript` or `callAsyncJavaScript` (the latter is a function body, not a standalone expression); unless noted, world is WebKit's `.page`. Shared `Page.helper`, `Ultrafast.script`, and `Trail.script` are installed lazily with eval, not at documentStart. A bundled module is loaded by its HTML page, not a WKUserScript. The macOS-specific animated backdrop, passkeys, extension shims, and PiP float do not block Linux v1.

**Coverage:** 140 extracted scripts across 30 Swift files, 2 bundled `.js` resources and 1 inline HTML module; 50 interpolation-bearing templates; 8 distinct script message handler names.

## Scripts

| Extracted file | Swift source:line / symbol | Feature | Injection time | World | Posts to handler / payload | Linux v1? |
| --- | --- | --- | --- | --- | --- | --- |
| [`extracted/Bench__L382.js`](extracted/Bench__L382.js) | `Bench.swift:382` `Bench.handle` | test-bench page interaction | eval | page | — | no |
| [`extracted/Bench__L755.js`](extracted/Bench__L755.js) | `Bench.swift:755` `Bench.extensionCommand` | test-bench page interaction | eval | page | — | no |
| [`extracted/Bench__L907.js`](extracted/Bench__L907.js) | `Bench.swift:907` `Bench.act` | test-bench page interaction | eval | page | — | no |
| [`extracted/Browser__L198.js`](extracted/Browser__L198.js) | `Browser.swift:198` `Browser.closeFind` | clear selection | eval | page | — | yes |
| [`extracted/Curtain__L148.js`](extracted/Curtain__L148.js) | `Curtain.swift:148` `Veiling.style` | curtain/hide picker & persistent CSS | documentStart (main frame); eval on update | page | — | yes |
| [`extracted/Curtain__L163.js`](extracted/Curtain__L163.js) | `Curtain.swift:163` `Veiling.picker` | curtain/hide picker & persistent CSS | documentStart (main frame) | page | `officeVeil` {selector,label,note} \| {off:true} \| {trouble:string} | yes |
| [`extracted/ExtensionPopup__L152.js`](extracted/ExtensionPopup__L152.js) | `ExtensionPopup.swift:152` `ExtensionPopup.preferred` | extension popup sizing | eval | page | — | no |
| [`extracted/ExtensionPopup__L202.js`](extracted/ExtensionPopup__L202.js) | `ExtensionPopup.swift:202` `ExtensionPopup.firstMeasure` | extension popup sizing | eval | page | — | no |
| [`extracted/ExtensionPopup__L231.js`](extracted/ExtensionPopup__L231.js) | `ExtensionPopup.swift:231` `ExtensionPopup.reach` | extension popup sizing | eval | page | — | no |
| [`extracted/ExtensionPopup__L255.js`](extracted/ExtensionPopup__L255.js) | `ExtensionPopup.swift:255` `ExtensionPopup.grow` | extension popup sizing | eval | page | — | no |
| [`extracted/ExtensionPopup__L265.js`](extracted/ExtensionPopup__L265.js) | `ExtensionPopup.swift:265` `ExtensionPopup.grow` | extension popup sizing | eval | page | — | no |
| [`extracted/ExtensionShims__L173.js`](extracted/ExtensionShims__L173.js) | `ExtensionShims.swift:173` `ExtensionShims.script` | extension shims/user scripts | extension script load (background/content) | extension runtime | — | no |
| [`extracted/ExtensionShims__L2369.js`](extracted/ExtensionShims__L2369.js) | `ExtensionShims.swift:2369` `ExtensionShims.userScriptFile` | extension shims/user scripts | extension user script run_at (configured) | MAIN or USER_SCRIPT (configured) | — | no |
| [`extracted/ExtensionShims__L2380.js`](extracted/ExtensionShims__L2380.js) | `ExtensionShims.swift:2380` `ExtensionShims.userScriptFile` | extension shims/user scripts | extension user script run_at (configured) | MAIN or USER_SCRIPT (configured) | — | no |
| [`extracted/Float__L490.js`](extracted/Float__L490.js) | `Float.swift:490` `Isolate.on` | picture-in-picture isolate | eval | page | — | no |
| [`extracted/Float__L566.js`](extracted/Float__L566.js) | `Float.swift:566` `Isolate.skip` | picture-in-picture isolate | eval | page | — | no |
| [`extracted/Float__L578.js`](extracted/Float__L578.js) | `Float.swift:578` `Isolate.where_` | picture-in-picture isolate | eval | page | — | no |
| [`extracted/Float__L587.js`](extracted/Float__L587.js) | `Float.swift:587` `Isolate.toggle` | picture-in-picture isolate | eval | page | — | no |
| [`extracted/Float__L597.js`](extracted/Float__L597.js) | `Float.swift:597` `Isolate.off` | picture-in-picture isolate | eval | page | — | no |
| [`extracted/Agent__L329.js`](extracted/Agent__L329.js) | `Fork/Agent/Agent.swift:329` `Agent.run` | agent get_text | eval | page | — | yes |
| [`extracted/AnimatedBackdrop__L323.js`](extracted/AnimatedBackdrop__L323.js) | `Fork/AnimatedBackdrop.swift:323` `BackdropWeb.call` | animated backdrop | eval | page | — | no |
| [`extracted/AnimatedBackdrop__L345.js`](extracted/AnimatedBackdrop__L345.js) | `Fork/AnimatedBackdrop.swift:345` `BackdropWeb.bench` | animated backdrop | eval | page | — | no |
| [`extracted/scene__L1.js`](extracted/scene__L1.js) | `Fork/Backdrop/scene.js:1` `resource module` | animated backdrop module / three.js dependency | bundled module (page load) | page (bundled file) | — | no |
| [`extracted/three.module.min__L1.js`](extracted/three.module.min__L1.js) | `Fork/Backdrop/three.module.min.js:1` `resource module` | animated backdrop module / three.js dependency | bundled module (page load) | page (bundled file) | — | no |
| [`extracted/CanvasBench__L287.js`](extracted/CanvasBench__L287.js) | `Fork/Canvas/CanvasBench.swift:287` `CanvasBench.perfStart` | canvas perf/diagnostics | eval | page | — | no |
| [`extracted/CanvasBench__L300.js`](extracted/CanvasBench__L300.js) | `Fork/Canvas/CanvasBench.swift:300` `CanvasBench.perfStop` | canvas perf/diagnostics | eval | page | — | no |
| [`extracted/CanvasBench__L331.js`](extracted/CanvasBench__L331.js) | `Fork/Canvas/CanvasBench.swift:331` `CanvasBench.lean` | canvas perf/diagnostics | eval | page | — | no |
| [`extracted/CanvasChat__L205.js`](extracted/CanvasChat__L205.js) | `Fork/Canvas/CanvasChat.swift:205` `CanvasChat.tell` | canvas chat | eval | page | — | yes |
| [`extracted/CanvasChat__L218.js`](extracted/CanvasChat__L218.js) | `Fork/Canvas/CanvasChat.swift:218` `CanvasChat.prepare` | canvas chat | eval | page | — | yes |
| [`extracted/CanvasChat__L254.js`](extracted/CanvasChat__L254.js) | `Fork/Canvas/CanvasChat.swift:254` `CanvasChat.readMembers` | canvas chat | eval | page | — | yes |
| [`extracted/CanvasChat__L272.js`](extracted/CanvasChat__L272.js) | `Fork/Canvas/CanvasChat.swift:272` `CanvasChat.received` | canvas chat | eval | page | — | yes |
| [`extracted/CanvasChat__L468.js`](extracted/CanvasChat__L468.js) | `Fork/Canvas/CanvasChat.swift:468` `CanvasChat.open` | canvas chat | eval | page | — | yes |
| [`extracted/CanvasChatUI__L246.js`](extracted/CanvasChatUI__L246.js) | `Fork/Canvas/CanvasChatUI.swift:246` `CanvasChat.runTool` | canvas chat | eval | page | — | yes |
| [`extracted/CanvasChatUI__L281.js`](extracted/CanvasChatUI__L281.js) | `Fork/Canvas/CanvasChatUI.swift:281` `CanvasChat.bench` | canvas chat | eval | page | — | no (bench only) |
| [`extracted/CanvasChatUI__L304.js`](extracted/CanvasChatUI__L304.js) | `Fork/Canvas/CanvasChatUI.swift:304` `CanvasChat.bench` | canvas chat | eval | page | — | no (bench only) |
| [`extracted/CanvasChatUI__L311.js`](extracted/CanvasChatUI__L311.js) | `Fork/Canvas/CanvasChatUI.swift:311` `CanvasChat.bench` | canvas chat | eval | page | — | no (bench only) |
| [`extracted/CanvasChatUI__L312.js`](extracted/CanvasChatUI__L312.js) | `Fork/Canvas/CanvasChatUI.swift:312` `CanvasChat.bench` | canvas chat | eval | page | — | no (bench only) |
| [`extracted/CanvasChatUI__L313.js`](extracted/CanvasChatUI__L313.js) | `Fork/Canvas/CanvasChatUI.swift:313` `CanvasChat.bench` | canvas chat | eval | page | — | no (bench only) |
| [`extracted/CanvasChatUI__L316.js`](extracted/CanvasChatUI__L316.js) | `Fork/Canvas/CanvasChatUI.swift:316` `CanvasChat.bench` | canvas chat | eval | page | — | no (bench only) |
| [`extracted/CanvasChatUI__L327.js`](extracted/CanvasChatUI__L327.js) | `Fork/Canvas/CanvasChatUI.swift:327` `CanvasChat.bench` | canvas chat | eval | page | — | no (bench only) |
| [`extracted/CanvasChatUI__L333.js`](extracted/CanvasChatUI__L333.js) | `Fork/Canvas/CanvasChatUI.swift:333` `CanvasChat.bench` | canvas chat | eval | page | — | no (bench only) |
| [`extracted/CanvasChecklistBench__L72.js`](extracted/CanvasChecklistBench__L72.js) | `Fork/Canvas/CanvasChecklistBench.swift:72` `CanvasChecklistBench.locate` | canvas perf/diagnostics | eval | page | — | no |
| [`extracted/CanvasFrames__L98.js`](extracted/CanvasFrames__L98.js) | `Fork/Canvas/CanvasFrames.swift:98` `CanvasHost.frameEvent` | canvas frame bridge | eval | page | — | yes |
| [`extracted/CanvasFrames__L106.js`](extracted/CanvasFrames__L106.js) | `Fork/Canvas/CanvasFrames.swift:106` `CanvasHost.tellFrameHost` | canvas frame bridge | eval | page | — | yes |
| [`extracted/CanvasFrames__L114.js`](extracted/CanvasFrames__L114.js) | `Fork/Canvas/CanvasFrames.swift:114` `CanvasHost.frameReporter` | canvas frame bridge | documentStart (all frames) | isolated: CopperCanvasFrames | `copperFrame` {kind:"nav"\|"escape"\|"blank",name,url?,title?,icon?} | yes |
| [`extracted/CanvasHost__L614.js`](extracted/CanvasHost__L614.js) | `Fork/Canvas/CanvasHost.swift:614` `CanvasHost.handToSiblings` | canvas bridge | eval | page | — | yes |
| [`extracted/CanvasHost__L658.js`](extracted/CanvasHost__L658.js) | `Fork/Canvas/CanvasHost.swift:658` `CanvasHost.ready` | canvas bridge | eval | page | — | yes |
| [`extracted/CanvasHost__L711.js`](extracted/CanvasHost__L711.js) | `Fork/Canvas/CanvasHost.swift:711` `CanvasHost.watchAppearance` | canvas bridge | eval | page | — | yes |
| [`extracted/CanvasHost__L742.js`](extracted/CanvasHost__L742.js) | `Fork/Canvas/CanvasHost.swift:742` `CanvasHost.adoptIfLive` | canvas bridge | eval | page | — | yes |
| [`extracted/CanvasHost__L788.js`](extracted/CanvasHost__L788.js) | `Fork/Canvas/CanvasHost.swift:788` `CanvasHost.apply` | canvas bridge | eval | page | — | yes |
| [`extracted/CanvasHost__L801.js`](extracted/CanvasHost__L801.js) | `Fork/Canvas/CanvasHost.swift:801` `CanvasHost.read` | canvas bridge | eval | page | — | yes |
| [`extracted/CanvasHost__L821.js`](extracted/CanvasHost__L821.js) | `Fork/Canvas/CanvasHost.swift:821` `CanvasHost.select` | canvas bridge | eval | page | — | yes |
| [`extracted/CanvasHost__L827.js`](extracted/CanvasHost__L827.js) | `Fork/Canvas/CanvasHost.swift:827` `CanvasHost.zoom` | canvas bridge | eval | page | — | yes |
| [`extracted/CanvasHost__L835.js`](extracted/CanvasHost__L835.js) | `Fork/Canvas/CanvasHost.swift:835` `CanvasHost.presence` | canvas bridge | eval | page | — | yes |
| [`extracted/CanvasHost__L868.js`](extracted/CanvasHost__L868.js) | `Fork/Canvas/CanvasHost.swift:868` `CanvasHost.compactIfNeeded` | canvas bridge | eval | page | — | yes |
| [`extracted/CanvasHost__L902.js`](extracted/CanvasHost__L902.js) | `Fork/Canvas/CanvasHost.swift:902` `CanvasHost.checkpoint` | canvas bridge | eval | page | — | yes |
| [`extracted/CanvasHost__L984.js`](extracted/CanvasHost__L984.js) | `Fork/Canvas/CanvasHost.swift:984` `CanvasHost.roomChanged` | canvas bridge | eval | page | — | yes |
| [`extracted/CanvasHost__L998.js`](extracted/CanvasHost__L998.js) | `Fork/Canvas/CanvasHost.swift:998` `CanvasHost.tellOpen` | canvas bridge | eval | page | — | yes |
| [`extracted/CanvasHost__L1034.js`](extracted/CanvasHost__L1034.js) | `Fork/Canvas/CanvasHost.swift:1034` `CanvasHost.hand` | canvas bridge | eval | page | — | yes |
| [`extracted/CanvasHost__L1062.js`](extracted/CanvasHost__L1062.js) | `Fork/Canvas/CanvasHost.swift:1062` `CanvasHost.shareNow` | canvas bridge | eval | page | — | yes |
| [`extracted/CanvasHost__L1109.js`](extracted/CanvasHost__L1109.js) | `Fork/Canvas/CanvasHost.swift:1109` `CanvasHost.statusNow` | canvas bridge | eval | page | — | yes |
| [`extracted/CanvasHost__L1144.js`](extracted/CanvasHost__L1144.js) | `Fork/Canvas/CanvasHost.swift:1144` `CanvasHost.renamed` | canvas bridge | eval | page | — | yes |
| [`extracted/CanvasImport__L243.js`](extracted/CanvasImport__L243.js) | `Fork/Canvas/CanvasImport.swift:243` `CanvasImport.migrate` | canvas legacy import | eval | page | — | no |
| [`extracted/CanvasLean__L78.js`](extracted/CanvasLean__L78.js) | `Fork/Canvas/CanvasLean.swift:78` `CanvasHost.typing` | canvas typing relay | documentStart (main frame) | page | `officeForms` {kind:"form"\|"submit"\|"settled"\|"focus"\|"fullscreen", user?,password?,typing?,rect?,field?,hint?,on?} | yes |
| [`extracted/canvas__L9.js`](extracted/canvas__L9.js) | `Fork/Canvas/Resources/canvas.html:9` `inline module/bridge` | canvas app/bridge (bundled inline module) | bundled module (page load) | page (bundled file) | `canvas` {type:"ready"\|"update"\|"selection"\|"share"\|"presence"\|"ws"\|"wsOpen"\|"wsClose"\|"openUrl"\|"log"\|"chat"\|"chatRead"\|"chatMembers"\|"mention", b64?,ids?,people?,url?,msg?…} | yes |
| [`extracted/Page__L38.js`](extracted/Page__L38.js) | `Fork/MCP/Page.swift:38` `Page.js` | MCP page helper probe | eval | page | — | yes |
| [`extracted/Page__L73.js`](extracted/Page__L73.js) | `Fork/MCP/Page.swift:73` `Page.snapshot` | snapshot/refs | eval | page | — | yes |
| [`extracted/Page__L82.js`](extracted/Page__L82.js) | `Fork/MCP/Page.swift:82` `Page.prepare` | snapshot/refs | eval | page | — | yes |
| [`extracted/Page__L110.js`](extracted/Page__L110.js) | `Fork/MCP/Page.swift:110` `Page.screenshot` | MCP page helper probe | eval | page | — | yes |
| [`extracted/Page__L138.js`](extracted/Page__L138.js) | `Fork/MCP/Page.swift:138` `Page.helper` | snapshot/refs, get_text, find, console capture & click/type helpers | eval | page | — | yes |
| [`extracted/Probe__L22.js`](extracted/Probe__L22.js) | `Fork/MCP/Probe.swift:22` `Probe.script` | perf probe | eval | page | — | yes |
| [`extracted/Tools__L310.js`](extracted/Tools__L310.js) | `Fork/MCP/Tools.swift:310` `Tools.dispatch` | MCP click | eval | page | — | yes |
| [`extracted/Tools__L320.js`](extracted/Tools__L320.js) | `Fork/MCP/Tools.swift:320` `Tools.dispatch` | MCP hover | eval | page | — | yes |
| [`extracted/Tools__L334.js`](extracted/Tools__L334.js) | `Fork/MCP/Tools.swift:334` `Tools.dispatch` | MCP page action/evaluation | eval | page | — | yes |
| [`extracted/Tools__L338.js`](extracted/Tools__L338.js) | `Fork/MCP/Tools.swift:338` `Tools.dispatch` | MCP native-setter typing | eval | page | — | yes |
| [`extracted/Tools__L341.js`](extracted/Tools__L341.js) | `Fork/MCP/Tools.swift:341` `Tools.dispatch` | MCP native-setter typing | eval | page | — | yes |
| [`extracted/Tools__L349.js`](extracted/Tools__L349.js) | `Fork/MCP/Tools.swift:349` `Tools.dispatch` | MCP page action/evaluation | eval | page | — | yes |
| [`extracted/Tools__L360.js`](extracted/Tools__L360.js) | `Fork/MCP/Tools.swift:360` `Tools.dispatch` | MCP fill | eval | page | — | yes |
| [`extracted/Tools__L370.js`](extracted/Tools__L370.js) | `Fork/MCP/Tools.swift:370` `Tools.dispatch` | MCP page action/evaluation | eval | page | — | yes |
| [`extracted/Tools__L377.js`](extracted/Tools__L377.js) | `Fork/MCP/Tools.swift:377` `Tools.dispatch` | MCP select | eval | page | — | yes |
| [`extracted/Tools__L390.js`](extracted/Tools__L390.js) | `Fork/MCP/Tools.swift:390` `Tools.dispatch` | MCP drag | eval | page | — | yes |
| [`extracted/Tools__L405.js`](extracted/Tools__L405.js) | `Fork/MCP/Tools.swift:405` `Tools.dispatch` | MCP page action/evaluation | eval | page | — | yes |
| [`extracted/Tools__L407.js`](extracted/Tools__L407.js) | `Fork/MCP/Tools.swift:407` `Tools.dispatch` | MCP page action/evaluation | eval | page | — | yes |
| [`extracted/Tools__L418.js`](extracted/Tools__L418.js) | `Fork/MCP/Tools.swift:418` `Tools.dispatch` | MCP wait for text | eval | page | — | yes |
| [`extracted/Tools__L424.js`](extracted/Tools__L424.js) | `Fork/MCP/Tools.swift:424` `Tools.dispatch` | MCP wait for text | eval | page | — | yes |
| [`extracted/Tools__L439.js`](extracted/Tools__L439.js) | `Fork/MCP/Tools.swift:439` `Tools.dispatch` | MCP scroll | eval | page | — | yes |
| [`extracted/Tools__L444.js`](extracted/Tools__L444.js) | `Fork/MCP/Tools.swift:444` `Tools.dispatch` | MCP get_text | eval | page | — | yes |
| [`extracted/Tools__L447.js`](extracted/Tools__L447.js) | `Fork/MCP/Tools.swift:447` `Tools.dispatch` | MCP console capture | eval | page | — | yes |
| [`extracted/Tools__L451.js`](extracted/Tools__L451.js) | `Fork/MCP/Tools.swift:451` `Tools.dispatch` | MCP find | eval | page | — | yes |
| [`extracted/Trail__L55.js`](extracted/Trail__L55.js) | `Fork/MCP/Trail.swift:55` `Trail.glide` | agent trails/visual cursor | eval | page | — | yes |
| [`extracted/Trail__L106.js`](extracted/Trail__L106.js) | `Fork/MCP/Trail.swift:106` `Trail.clear` | agent trails/visual cursor | eval | page | — | yes |
| [`extracted/Trail__L126.js`](extracted/Trail__L126.js) | `Fork/MCP/Trail.swift:126` `Trail.install` | agent trails/visual cursor | eval | page | — | yes |
| [`extracted/Trail__L140.js`](extracted/Trail__L140.js) | `Fork/MCP/Trail.swift:140` `Trail.fire` | agent trails/visual cursor | eval | page | — | yes |
| [`extracted/Trail__L146.js`](extracted/Trail__L146.js) | `Fork/MCP/Trail.swift:146` `Trail.script` | agent trails/visual cursor | eval | page | — | yes |
| [`extracted/Ultrafast__L132.js`](extracted/Ultrafast__L132.js) | `Fork/MCP/Ultrafast.swift:132` `Ultrafast.script` | Jev fast page observation & actions | eval | page | — | yes |
| [`extracted/Ultrafast__L320.js`](extracted/Ultrafast__L320.js) | `Fork/MCP/Ultrafast.swift:320` `Ultrafast.js` | Jev action/probe | eval | page | — | yes |
| [`extracted/Ultrafast__L354.js`](extracted/Ultrafast__L354.js) | `Fork/MCP/Ultrafast.swift:354` `Ultrafast.observe` | Jev action/probe | eval | page | — | yes |
| [`extracted/Ultrafast__L365.js`](extracted/Ultrafast__L365.js) | `Fork/MCP/Ultrafast.swift:365` `Ultrafast.observe` | Jev action/probe | eval | page | — | yes |
| [`extracted/Ultrafast__L377.js`](extracted/Ultrafast__L377.js) | `Fork/MCP/Ultrafast.swift:377` `Ultrafast.fresh` | Jev action/probe | eval | page | — | yes |
| [`extracted/Ultrafast__L385.js`](extracted/Ultrafast__L385.js) | `Fork/MCP/Ultrafast.swift:385` `Ultrafast.fresh` | Jev action/probe | eval | page | — | yes |
| [`extracted/Ultrafast__L688.js`](extracted/Ultrafast__L688.js) | `Fork/MCP/Ultrafast.swift:688` `Ultrafast.act` | Jev action/probe | eval | page | — | yes |
| [`extracted/Ultrafast__L691.js`](extracted/Ultrafast__L691.js) | `Fork/MCP/Ultrafast.swift:691` `Ultrafast.act` | Jev action/probe | eval | page | — | yes |
| [`extracted/Ultrafast__L712.js`](extracted/Ultrafast__L712.js) | `Fork/MCP/Ultrafast.swift:712` `Ultrafast.act` | Jev action/probe | eval | page | — | yes |
| [`extracted/Ultrafast__L721.js`](extracted/Ultrafast__L721.js) | `Fork/MCP/Ultrafast.swift:721` `Ultrafast.act` | Jev action/probe | eval | page | — | yes |
| [`extracted/Ultrafast__L725.js`](extracted/Ultrafast__L725.js) | `Fork/MCP/Ultrafast.swift:725` `Ultrafast.act` | Jev action/probe | eval | page | — | yes |
| [`extracted/Ultrafast__L731.js`](extracted/Ultrafast__L731.js) | `Fork/MCP/Ultrafast.swift:731` `Ultrafast.act` | Jev action/probe | eval | page | — | yes |
| [`extracted/Ultrafast__L1038.js`](extracted/Ultrafast__L1038.js) | `Fork/MCP/Ultrafast.swift:1038` `Ultrafast.call` | Jev action/probe | eval | page | — | yes |
| [`extracted/Passkeys__L22.js`](extracted/Passkeys__L22.js) | `Fork/Passkeys/Passkeys.swift:22` `Passkeys.script` | passkey shim | documentStart (all frames) | page | `copperPasskeys` {kind:"create"\|"get",origin,topOrigin,crossOrigin,rpId,mediation,requestId,publicKey…} (promise reply) | no |
| [`extracted/StorageImport__L162.js`](extracted/StorageImport__L162.js) | `Fork/StorageImport.swift:162` `StorageImport.put` | import localStorage | eval | page | — | yes |
| [`extracted/StorageImport__L169.js`](extracted/StorageImport__L169.js) | `Fork/StorageImport.swift:169` `StorageImport.put` | import localStorage | eval | page | — | yes |
| [`extracted/Forms__L90.js`](extracted/Forms__L90.js) | `Forms.swift:90` `FormRelay.withoutPasskeys` | forms/password detection & fill | documentStart (all frames) | page | — | no (Apple passkeys workaround) |
| [`extracted/Forms__L102.js`](extracted/Forms__L102.js) | `Forms.swift:102` `FormRelay.script` | forms/password detection & fill | documentEnd (main frame) | page | `officeForms` {kind:"form"\|"submit"\|"settled"\|"focus"\|"fullscreen", user?,password?,typing?,rect?,field?,hint?,on?} | yes |
| [`extracted/Icons__L238.js`](extracted/Icons__L238.js) | `Icons.swift:238` `Favicons.probe` | favicon detection | eval | page | — | yes |
| [`extracted/ImageMenu__L32.js`](extracted/ImageMenu__L32.js) | `ImageMenu.swift:32` `ImageRelay.watch` | image right-click relay | documentStart (all frames) | page | `officeImages` {src:string} | yes |
| [`extracted/Reader__L11.js`](extracted/Reader__L11.js) | `Reader.swift:11` `Reader.script` | reader mode | eval | page | — | yes |
| [`extracted/StoreRelay__L40.js`](extracted/StoreRelay__L40.js) | `StoreRelay.swift:40` `StoreRelay.script` | extension store button | documentEnd (main frame) | page | `officeStore` {placed:pageID} \| {add:true} | no |
| [`extracted/Swipe__L33.js`](extracted/Swipe__L33.js) | `Swipe.swift:33` `Swipe.calm` | horizontal scroll/swipe arbitration | documentStart (main frame) | page | — | yes |
| [`extracted/Swipe__L45.js`](extracted/Swipe__L45.js) | `Swipe.swift:45` `Swipe.watch` | horizontal scroll/swipe arbitration | documentStart (all frames) | page | `officeScroll` {y:number,max:number} \| {side:"taken"\|"free"} | yes |
| [`extracted/Tab__L486.js`](extracted/Tab__L486.js) | `Tab.swift:486` `Tab.startPicking` | curtain/hide picker | eval | page | — | yes |
| [`extracted/Tab__L487.js`](extracted/Tab__L487.js) | `Tab.swift:487` `Tab.stopPicking` | curtain/hide picker | eval | page | — | yes |
| [`extracted/Tab__L546.js`](extracted/Tab__L546.js) | `Tab.swift:546` `Tab.settleSignIn` | forms/password detection & fill | eval | page | — | yes |
| [`extracted/Tab__L566.js`](extracted/Tab__L566.js) | `Tab.swift:566` `Tab.fill` | forms/password detection & fill | eval | page | — | yes |
| [`extracted/Tab__L574.js`](extracted/Tab__L574.js) | `Tab.swift:574` `Tab.submitSignIn` | forms/password detection & fill | eval | page | — | yes |
| [`extracted/Tab__L582.js`](extracted/Tab__L582.js) | `Tab.swift:582` `Tab.fillOTP` | forms/password detection & fill | eval | page | — | yes |
| [`extracted/Tab__L599.js`](extracted/Tab__L599.js) | `Tab.swift:599` `Tab.fillValues` | forms/password detection & fill | eval | page | — | yes |
| [`extracted/Tab__L613.js`](extracted/Tab__L613.js) | `Tab.swift:613` `Tab.fillFocused` | forms/password detection & fill | eval | page | — | yes |
| [`extracted/Tab__L627.js`](extracted/Tab__L627.js) | `Tab.swift:627` `Tab.fillField` | forms/password detection & fill | eval | page | — | yes |
| [`extracted/Tab__L637.js`](extracted/Tab__L637.js) | `Tab.swift:637` `Tab.hasFields` | forms/password detection & fill | eval | page | — | yes |
| [`extracted/Tab__L647.js`](extracted/Tab__L647.js) | `Tab.swift:647` `Tab.fieldsPresent` | forms/password detection & fill | eval | page | — | yes |
| [`extracted/Tab__L658.js`](extracted/Tab__L658.js) | `Tab.swift:658` `Tab.hasOTPField` | forms/password detection & fill | eval | page | — | yes |
| [`extracted/Tab__L668.js`](extracted/Tab__L668.js) | `Tab.swift:668` `Tab.hasPasswordField` | forms/password detection & fill | eval | page | — | yes |
| [`extracted/Tab__L682.js`](extracted/Tab__L682.js) | `Tab.swift:682` `Tab.peek` | curtain/hide picker | eval | page | — | yes |
| [`extracted/Tab__L687.js`](extracted/Tab__L687.js) | `Tab.swift:687` `Tab.unpeek` | curtain/hide picker | eval | page | — | yes |
| [`extracted/Tab__L813.js`](extracted/Tab__L813.js) | `Tab.swift:813` `Tab.unsaved` | forms/password detection & fill | eval | page | — | yes |
| [`extracted/Tab__L901.js`](extracted/Tab__L901.js) | `Tab.swift:901` `Tab.tellStore` | extension store button | eval | page | — | no (extension store) |
| [`extracted/Tab__L966.js`](extracted/Tab__L966.js) | `Tab.swift:966` `Tab.loadAndVerify` | navigation readiness | eval | page | — | yes |
| [`extracted/Tab__L1015.js`](extracted/Tab__L1015.js) | `Tab.swift:1015` `Tab.revive` | navigation readiness | eval | page | — | yes |
| [`extracted/Tab__L1376.js`](extracted/Tab__L1376.js) | `Tab.swift:1376` `PageView.smartMagnify` | smart zoom | eval | page | — | yes |
| [`extracted/Tab__L1395.js`](extracted/Tab__L1395.js) | `Tab.swift:1395` `PageView.smart` | smart zoom | eval | page | — | yes |
| [`extracted/Tab__L1597.js`](extracted/Tab__L1597.js) | `Tab.swift:1597` `ScrollRelay.script` | scroll position relay | documentEnd (main frame) | page | `officeScroll` {y:number,max:number} \| {side:"taken"\|"free"} | yes |

## Script message handlers (every registration)

| Registered name | Swift registration | Constant | World | Accepted page payload | Linux v1? |
| --- | --- | --- | --- | --- | --- |
| `officeScroll` | `Tab.swift:350` | `ScrollRelay.name` | page | `{y,max} or {side:"taken"\|"free"}` | yes |
| `officeVeil` | `Tab.swift:351` | `VeilRelay.name` | page | `{selector,label,note}, {off:true}, {trouble:string}` | yes |
| `officeImages` | `Tab.swift:352` | `ImageRelay.name` | page | `{src:string}` | yes |
| `officeStore` | `Tab.swift:353` | `StoreRelay.name` | page | `{placed:pageID} or {add:true}` | no (extension store) |
| `officeForms` | `Tab.swift:354` | `FormRelay.name` | page | `{kind, user?,password?,typing?,rect?,field?,hint?,on?}` | yes |
| `copperPasskeys` | `Tab.swift:355` | `Passkeys.name` | page | `{kind,origin,rpId,requestId,publicKey…}, reply` | no (passkeys) |
| `canvas` | `Fork/Canvas/CanvasHost.swift:301,320` | `CanvasHost.handler` | page (bundled main frame) | `{type, b64?,ids?,people?,url?,msg?…} or JSON string` | yes |
| `copperFrame` | `Fork/Canvas/CanvasFrames.swift:67` | `CanvasHost.frameHandler` | isolated CopperCanvasFrames | `{kind,name,url?,title?,icon?}` | yes |

Notes: `officeScroll` carries both `ScrollRelay.script` positions and `Swipe.watch` ownership reports. `CanvasLean.typing` posts `officeForms` even though the lean board removes other ordinary page handlers. The canvas `canvas` bridge accepts a dictionary or a JSON string and only accepts messages from the bundled main frame. `copperFrame` is registered in its named isolated content world and reports only direct HTTP(S) subframes. No voice-specific page JS or message handler was found under `Sources/Search/Fork/Voice/`.
