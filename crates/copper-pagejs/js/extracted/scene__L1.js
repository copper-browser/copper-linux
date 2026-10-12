// from Sources/Search/Fork/Backdrop/scene.js:1 resource module
// The living backdrop behind a space column: a finely subdivided plane,
// displaced by simplex noise in the vertex shader and seen in perspective
// (Stripe's hero technique), coloured from the space's own colours and lit
// softly along the folds. Four scenes share one program shape; the style is
// a compile-time constant so no shader branches on it.
//
// Swift drives everything through `window.backdrop`:
//   backdrop.set({ style, colors: ['#rrggbb', …], speed, blur, dark, key, epoch }, fade)
//   backdrop.toward({ … the same … }, x)
//   backdrop.mode('run' | 'still' | 'pause')
// Nothing is drawn until the first `set`, so the still gradient under the
// (transparent) web view carries the load.
//
// One page serves every space the column visits. A space switch is a `set`
// with a `fade`: the colours, the softness and the clock slide from what is
// on screen to the new space's over that many seconds, in the shader, with
// no reload. A swipe is `toward`: the arriving space's look mixed in by the
// fingers' `x`, 0…1, until the swipe lands (a `set` of the new space) or
// springs back (a `set` of the old one).
//
// The scenes' clock is the wall's, counted from an `epoch` Swift fixes once
// per launch, not the page's own: a space's scene at any moment is where it
// "would be" had it been drawing all along, so going away and coming back,
// a pause behind another window, or a second window, all land on the same
// frame of the same continuous motion. Each space's time is that clock times
// its speed, plus an offset kept per space (`key`) so that dragging the
// speed slider bends the motion rather than jumping it.
//
// Performance is the point. The canvas is rendered at half device
// resolution (less as blur rises) and upscaled by the compositor — the
// picture is soft by design, so nothing is lost. Frames are capped at 30 a
// second, all the noise is evaluated per vertex (a few thousand of them)
// rather than per pixel, and `mode` stops requestAnimationFrame outright
// whenever Swift says nobody is looking.

import { WebGLRenderer, Scene, PerspectiveCamera, PlaneGeometry, ShaderMaterial, Mesh, Vector3, Color }
  from './three.module.min.js';

// Frames a second while running. The scenes drift slowly enough that
// twenty-four look continuous, and every frame costs the same whatever its
// size (a draw, a surface swap, a compositor commit in three processes), so
// the rate is the one real lever on power. Thirty is what the theme may
// ask for; the bench can push `set({ fps })` to sixty to weigh a ProMotion
// option.
const FPS = 24, MAX_FPS = 60;
const STYLES = { ribbons: 0, silk: 1, aurora: 2, waves: 3 };

// The clock folds over every 2^16 seconds (about eighteen hours): the
// shaders read time as a 32-bit float, and past that its steps grow coarse
// enough to show as judder. The fold is one jump, once in that span, at the
// same instant for every space.
const WRAP = 65536;
let epoch = null;
function wall() {
  const now = performance.timeOrigin + performance.now();
  const since = (now - (epoch ?? performance.timeOrigin)) / 1000;
  return ((since % WRAP) + WRAP) % WRAP;
}

// Ashima's 3D simplex noise — the standard GLSL port.
const NOISE = /* glsl */`
vec3 mod289(vec3 x){return x-floor(x*(1.0/289.0))*289.0;}
vec4 mod289(vec4 x){return x-floor(x*(1.0/289.0))*289.0;}
vec4 permute(vec4 x){return mod289(((x*34.0)+1.0)*x);}
vec4 taylorInvSqrt(vec4 r){return 1.79284291400159-0.85373472095314*r;}
float snoise(vec3 v){
  const vec2 C=vec2(1.0/6.0,1.0/3.0); const vec4 D=vec4(0.0,0.5,1.0,2.0);
  vec3 i=floor(v+dot(v,C.yyy)); vec3 x0=v-i+dot(i,C.xxx);
  vec3 g=step(x0.yzx,x0.xyz); vec3 l=1.0-g; vec3 i1=min(g.xyz,l.zxy); vec3 i2=max(g.xyz,l.zxy);
  vec3 x1=x0-i1+C.xxx; vec3 x2=x0-i2+C.yyy; vec3 x3=x0-D.yyy;
  i=mod289(i);
  vec4 p=permute(permute(permute(i.z+vec4(0.0,i1.z,i2.z,1.0))+i.y+vec4(0.0,i1.y,i2.y,1.0))+i.x+vec4(0.0,i1.x,i2.x,1.0));
  float n_=0.142857142857; vec3 ns=n_*D.wyz-D.xzx;
  vec4 j=p-49.0*floor(p*ns.z*ns.z); vec4 x_=floor(j*ns.z); vec4 y_=floor(j-7.0*x_);
  vec4 x=x_*ns.x+ns.yyyy; vec4 y=y_*ns.x+ns.yyyy; vec4 h=1.0-abs(x)-abs(y);
  vec4 b0=vec4(x.xy,y.xy); vec4 b1=vec4(x.zw,y.zw);
  vec4 s0=floor(b0)*2.0+1.0; vec4 s1=floor(b1)*2.0+1.0; vec4 sh=-step(h,vec4(0.0));
  vec4 a0=b0.xzyw+s0.xzyw*sh.xxyy; vec4 a1=b1.xzyw+s1.xzyw*sh.zzww;
  vec3 p0=vec3(a0.xy,h.x); vec3 p1=vec3(a0.zw,h.y); vec3 p2=vec3(a1.xy,h.z); vec3 p3=vec3(a1.zw,h.w);
  vec4 norm=taylorInvSqrt(vec4(dot(p0,p0),dot(p1,p1),dot(p2,p2),dot(p3,p3)));
  p0*=norm.x; p1*=norm.y; p2*=norm.z; p3*=norm.w;
  vec4 m=max(0.6-vec4(dot(x0,x0),dot(x1,x1),dot(x2,x2),dot(x3,x3)),0.0); m=m*m;
  return 42.0*dot(m*m,vec4(dot(p0,x0),dot(p1,x1),dot(p2,x2),dot(p3,x3)));
}`;

// The height field for each style, in plane units (the visible column is
// about 2 units tall). `vF` carries whatever the fragment shader needs to
// colour the point — the ribbon index, the noise values — so the pixel
// side never evaluates noise itself.
const VERTEX = /* glsl */`
uniform float uTime, uAmp, uBlur;
varying vec3 vN;
varying vec2 vP;
varying vec4 vF;
${NOISE}

vec4 field(vec2 p, float t) {
  // The blur setting also slows the pattern down in space: fewer, wider
  // features read as softer without any post-processing.
  float k = mix(1.0, 0.65, uBlur);
  #if STYLE == 0
    // Ribbons: bands stacked on a diagonal, each edge a slow wave. Each
    // ribbon is a smooth bump — quicker up than down — so every edge is a
    // fold the light can catch. (A true step there is finer than the mesh
    // and shows as a sawtooth.)
    float ang = -0.62;
    mat2 R = mat2(cos(ang), -sin(ang), sin(ang), cos(ang));
    vec2 d = R * p;
    float u = d.y, v = d.x;
    float wob = 0.10*sin(v*2.1*k + t*0.35) + 0.05*sin(v*4.3*k - t*0.5 + 1.0)
              + 0.06*snoise(vec3(p*0.9*k, t*0.15));
    float band = (u + wob) * 4.2 * k;
    float f = fract(band);
    float ridge = smoothstep(0.0, 0.35, f) * (1.0 - smoothstep(0.55, 1.0, f));
    float swell = snoise(vec3(p*0.7*k + 5.0, t*0.12));
    return vec4(0.55*ridge + 0.5*swell, band, v, swell);
  #elif STYLE == 1
    // Silk: domain-warped noise, two octaves, drifting on the diagonal.
    vec2 q = p + 0.35*vec2(snoise(vec3(p*0.8*k, t*0.10)), snoise(vec3(p*0.8*k + 7.3, t*0.10 + 3.0)));
    q += vec2(t*0.04, -t*0.03);
    float a = snoise(vec3(q*1.1*k, t*0.18));
    float b = snoise(vec3(q*2.3*k + 11.0, t*0.25));
    return vec4(0.8*a + 0.35*b, a, b, 0.0);
  #elif STYLE == 2
    // Aurora: curtains stretched up the column, a slow mass behind them and
    // a finer weave in front.
    float c = snoise(vec3(p.x*3.2*k, p.y*0.55*k, t*0.22));
    float m = snoise(vec3(p*0.7*k + 3.0, t*0.08));
    float r = snoise(vec3(p.x*6.0*k + 9.0, p.y*1.1*k, t*0.35));
    return vec4(1.0*c*(0.6 + 0.4*m) + 0.3*r, c, m, r);
  #else
    // Waves: two rollers crossing at a slight angle, over a slow swell.
    float w1 = sin(p.y*5.5*k + p.x*0.9 + t*0.7 + 0.8*snoise(vec3(p*0.6*k, t*0.1)));
    float w2 = sin(p.y*9.0*k - p.x*1.6 - t*1.05 + 1.3);
    float n = snoise(vec3(p*1.0*k, t*0.15));
    return vec4(0.5*w1 + 0.22*w2 + 0.35*n, w1, w2, n);
  #endif
}

void main() {
  vec2 p = position.xy;
  float t = uTime;
  vec4 f = field(p, t);
  // Two more taps give the surface a normal; the step is a little wider
  // than a vertex so the lighting stays smooth across the mesh.
  float e = 0.025;
  float hx = field(p + vec2(e, 0.0), t).x;
  float hy = field(p + vec2(0.0, e), t).x;
  float h = f.x * uAmp;
  vN = normalize(vec3(-(hx*uAmp - h) / e, -(hy*uAmp - h) / e, 1.0));
  vP = p;
  vF = f;
  gl_Position = projectionMatrix * modelViewMatrix * vec4(p, h, 1.0);
}`;

// Colour is a five-stop ramp built on the JS side from the space's one to
// three colours; every style reads it differently. Lighting is a half-
// Lambert wash and a small sheen — enough to show the folds, never enough
// to push a patch out of the band the titles were tuned against.
const FRAGMENT = /* glsl */`
uniform vec3 uC[5];
uniform vec3 uLight;
uniform float uBlur, uDark;
varying vec3 vN;
varying vec2 vP;
varying vec4 vF;

vec3 pick(int k) {
  k -= 5 * int(floor(float(k) / 5.0));
  if (k == 0) return uC[0];
  if (k == 1) return uC[1];
  if (k == 2) return uC[2];
  if (k == 3) return uC[3];
  return uC[4];
}

// 0…1 across the five stops, mirrored so the ends never seam.
vec3 ramp(float x) {
  x = abs(fract(x * 0.5) * 2.0 - 1.0) * 4.0;
  int i = int(floor(x));
  float f = smoothstep(0.0, 1.0, fract(x));
  return mix(pick(i), pick(i + 1), f);
}

void main() {
  vec3 col;
  #if STYLE == 0
    float band = vF.y;
    float f = fract(band);
    int k = int(floor(band));
    vec3 cur = pick(k);
    // Each ribbon lightens along its width, like light on silk, and the
    // one beneath darkens just under the edge above it.
    cur = mix(cur, uLight, 0.16 * f);
    float edge = mix(0.02, 0.14, uBlur);
    vec3 prev = pick(k - 1);
    prev = mix(prev, uLight, 0.16);
    col = mix(prev, cur, smoothstep(0.0, edge, f));
    col *= 1.0 - 0.07 * smoothstep(mix(0.7, 0.4, uBlur), 1.0, f);
  #elif STYLE == 1
    col = ramp(0.5 + 0.35 * vF.y + 0.15 * vF.z);
  #elif STYLE == 2
    col = ramp(0.5 + 0.4 * vF.y + 0.25 * vF.z + 0.08 * vF.w);
    // The curtains' crests glow a little, as an aurora does.
    col = mix(col, uLight, 0.14 * smoothstep(0.2, 0.9, vF.y));
  #else
    col = ramp(0.5 + 0.38 * vF.y + 0.12 * vF.z + 0.14 * vF.w);
  #endif

  vec3 N = normalize(vN);
  vec3 L = normalize(vec3(-0.35, 0.55, 0.75));
  float diff = 0.5 + 0.5 * dot(N, L);
  vec3 H = normalize(L + vec3(0.0, 0.0, 1.0));
  float spec = pow(max(dot(N, H), 0.0), 28.0);
  // Folds show less as the blur rises, and the sheen is dimmer in the dark
  // where a bright patch would glare against light titles.
  float relief = mix(1.0, 0.45, uBlur);
  col *= mix(1.0 - 0.10 * relief, 1.0 + 0.08 * relief, diff);
  col += uLight * spec * mix(0.10, 0.05, uDark) * relief;
  gl_FragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}`;

// ---- colours -------------------------------------------------------------

function hexToRGB(hex) {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  const n = m ? parseInt(m[1], 16) : 0x888888;
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
}
const mix = (a, b, t) => a.map((v, i) => v + (b[i] - v) * t);
const lighten = (c, t) => mix(c, [1, 1, 1], t);
// Deeper: towards black, but pulled back towards the hue so the shade stays
// coloured rather than turning grey.
function deepen(c, t) {
  const mx = Math.max(...c), mn = Math.min(...c);
  const sat = mx > 0 ? (mx - mn) / mx : 0;
  const richer = c.map(v => mx - (mx - v) * Math.min(1, 1 + (sat < 0.6 ? 0.6 : 0.2)));
  return mix(mix(c, richer, 0.5), [0, 0, 0], t);
}

/// One to three column colours become the five stops the shaders read: the
/// given colours in order with in-between mixes, or shades of the one colour.
/// In the dark the lifts are smaller — a pale patch on a dark column glares.
function palette(hexes, dark) {
  const cs = hexes.map(hexToRGB);
  const lift = dark ? 0.10 : 0.30;
  let stops;
  if (cs.length >= 3) {
    stops = [cs[0], mix(cs[0], cs[1], 0.5), cs[1], mix(cs[1], cs[2], 0.5), cs[2]];
  } else if (cs.length === 2) {
    stops = [lighten(cs[0], lift * 0.6), cs[0], mix(cs[0], cs[1], 0.5), cs[1], lighten(cs[1], lift * 0.6)];
  } else {
    const a = cs[0] || [0.6, 0.6, 0.6];
    stops = [lighten(a, lift), lighten(a, lift * 0.4), a, deepen(a, dark ? 0.12 : 0.08), lighten(a, lift * 0.7)];
  }
  const base = cs[0] || stops[2];
  const light = dark ? lighten(base, 0.25) : lighten(base, 0.7);
  return { stops: stops.map(s => new Color(...s)), light: new Color(...light) };
}

// ---- the scene -----------------------------------------------------------

class Backdrop {
  constructor() {
    this.params = null;
    // What is drawn: `to`, the look Swift last set; `from`, while a fade
    // is on, the look it is fading out of; `scrub`, while a swipe is on, the
    // arriving space's look and how far it is mixed in.
    this.to = null;
    this.from = null;
    this.fadeAt = 0;
    this.fadeFor = 0;
    this.scrub = null;
    // Per space, what its clock is ahead of `wall × speed` — moved only
    // when its speed changes, so the change is a bend and not a jump.
    this.offsets = {};
    this.mode = 'pause';
    this.running = false;
    this.time = 0;
    this.last = 0;
    this.frame = 0;
    this.resizeDue = true;
    this.segments = [0, 0];
    this.lost = false;
    // Frame accounting for the bench: frames drawn, and over the last second
    // how many landed and how long `draw` took on the page's own thread.
    this.frames = 0;
    this.window = { since: 0, frames: 0, drawMs: 0, fps: 0, avgDrawMs: 0 };
    new ResizeObserver(() => { this.resizeDue = true; this.poke(); }).observe(document.documentElement);
    document.addEventListener('visibilitychange', () => this.reconsider());
  }

  /// A look as the shaders want it: plain numbers, so two can be mixed.
  resolve(p) {
    const { stops, light } = palette(p.colors || [], !!p.dark);
    return {
      style: STYLES[p.style] ?? 0, stops: stops.map(c => [c.r, c.g, c.b]), light: [light.r, light.g, light.b],
      blur: Math.min(1, Math.max(0, +p.blur || 0)), dark: p.dark ? 1 : 0,
      speed: Math.max(0, +p.speed || 0), key: String(p.key || ''),
    };
  }

  /// Where a look's scene is at wall time `w`.
  timeOf(look, w) {
    return look.clock ? look.clock(w) : w * look.speed + (this.offsets[look.key] || 0);
  }

  /// Part way from look `a` to look `b`. The shape (the style) cannot be
  /// mixed — it is which program runs — so it changes half way; the clock
  /// is mixed like the colours, so the motion never jumps either.
  blend(a, b, k) {
    if (k <= 0) return a;
    if (k >= 1) return b;
    const m = (x, y) => x + (y - x) * k;
    return {
      style: k < 0.5 ? a.style : b.style,
      stops: a.stops.map((s, i) => s.map((v, j) => m(v, b.stops[i][j]))),
      light: a.light.map((v, j) => m(v, b.light[j])),
      blur: m(a.blur, b.blur), dark: m(a.dark, b.dark), speed: Math.max(a.speed, b.speed), key: '',
      clock: w => m(this.timeOf(a, w), this.timeOf(b, w)),
    };
  }

  /// What is on screen at wall time `w`.
  current(w) {
    let look = this.to;
    if (this.from) {
      const x = this.fadeFor > 0 ? (performance.now() - this.fadeAt) / this.fadeFor : 1;
      if (x >= 1) this.from = null;
      else look = this.blend(this.from, this.to, x * x * (3 - 2 * x));
    }
    if (this.scrub) look = this.blend(look, this.scrub.look, this.scrub.x);
    return look;
  }

  set(params, fade) {
    if (params.epoch) epoch = +params.epoch;
    const look = this.resolve(params);
    const first = !this.to;
    this.params = params;
    if (first) {
      this.to = look;
      this.build();
    } else if (!this.scrub && look.key && look.key === this.to.key) {
      // The same space, retuned — its own page's colours or sliders. Its
      // clock carries on from where it is at the new speed.
      const w = wall();
      if (look.speed !== this.to.speed) this.offsets[look.key] = this.timeOf(this.to, w) - w * look.speed;
      this.to = look;
    } else {
      // Another space, or the end of a swipe either way: from whatever is
      // on screen now, mixed and all, to this one.
      const shown = this.current(wall());
      this.scrub = null;
      this.to = look;
      this.from = shown === look ? null : shown;
      this.fadeAt = performance.now();
      this.fadeFor = this.mode === 'run' && +fade > 0 ? +fade * 1000 : 0;
      if (!this.fadeFor) this.from = null;
    }
    this.fps = Math.min(MAX_FPS, Math.max(5, +params.fps || FPS));
    // Blur is mostly resolution: a softer picture is a smaller one, upscaled.
    this.resizeDue = true;
    this.reconsider();
    this.poke();
  }

  /// A swipe in flight: the arriving space mixed in by `x`, 0…1.
  toward(params, x) {
    if (!this.to) return;
    const look = this.resolve(params);
    if (!this.scrub || this.scrub.look.key !== look.key || look.key === '') this.scrub = { look, x: 0 };
    else this.scrub.look = look;
    this.scrub.x = Math.min(1, Math.max(0, +x || 0));
    this.reconsider();
    this.poke();
  }

  setMode(mode) {
    this.mode = mode;
    // A fade is motion: with Reduce Motion (or nothing to see) it lands at once.
    if (mode !== 'run') this.from = null;
    this.reconsider();
    if (mode === 'still') this.poke();
  }

  build() {
    const canvas = document.createElement('canvas');
    document.body.appendChild(canvas);
    canvas.addEventListener('webglcontextlost', e => { e.preventDefault(); this.lost = true; this.stop(); });
    canvas.addEventListener('webglcontextrestored', () => { this.lost = false; this.resizeDue = true; this.reconsider(); this.poke(); });
    this.renderer = new WebGLRenderer({
      canvas, antialias: false, alpha: false, depth: false, stencil: false,
      powerPreference: 'low-power', preserveDrawingBuffer: false,
    });
    this.scene = new Scene();
    // Fov and distance chosen so the visible column is two plane units
    // tall, whatever its width; the mesh is oversized to cover the tilt.
    this.camera = new PerspectiveCamera(36, 1, 0.1, 10);
    this.camera.position.set(0, 0, 1 / Math.tan(18 * Math.PI / 180));
    this.camera.lookAt(new Vector3(0, 0, 0));
    this.material = new ShaderMaterial({
      defines: { STYLE: this.to.style },
      uniforms: {
        uTime: { value: 0 }, uAmp: { value: 0.16 }, uBlur: { value: 0 }, uDark: { value: 0 },
        uC: { value: [new Color(), new Color(), new Color(), new Color(), new Color()] },
        uLight: { value: new Color(1, 1, 1) },
      },
      vertexShader: VERTEX, fragmentShader: FRAGMENT, depthTest: false, depthWrite: false,
    });
    this.mesh = new Mesh(new PlaneGeometry(1, 1, 1, 1), this.material);
    // Tilted away at the top, so the pattern compresses into the distance.
    this.mesh.rotation.x = -0.32;
    this.scene.add(this.mesh);
  }

  resize() {
    const w = Math.max(1, document.documentElement.clientWidth);
    const h = Math.max(1, document.documentElement.clientHeight);
    const blur = this.to.blur;
    const scale = 0.5 * (1 - 0.55 * blur);
    // Capped in pixels too: a backdrop the size of a window still costs the
    // same as a column.
    const budget = Math.sqrt(1.2e6 / Math.max(1, w * h * (devicePixelRatio * scale) ** 2));
    this.renderer.setPixelRatio(devicePixelRatio * scale * Math.min(1, budget));
    this.renderer.setSize(w, h, false);
    this.camera.aspect = w / h;
    this.camera.updateProjectionMatrix();
    const aspect = w / h;
    // About one vertex per six points on screen, within bounds; rebuilt
    // only when the count moves enough to matter.
    const sx = Math.min(160, Math.max(24, Math.round(w / 6)));
    const sy = Math.min(400, Math.max(24, Math.round(h / 6)));
    const [ox, oy] = this.segments;
    if (Math.abs(sx - ox) > ox * 0.2 || Math.abs(sy - oy) > oy * 0.2) {
      this.mesh.geometry.dispose();
      this.mesh.geometry = new PlaneGeometry(2 * aspect * 1.6 + 0.6, 2 * 1.7, sx, sy);
      this.segments = [sx, sy];
    }
    this.resizeDue = false;
  }

  // Whether frames should be flowing: Swift's word, the page's own
  // visibility, and a speed above zero. WebKit stops requestAnimationFrame
  // itself for an occluded page, which is why the bench (whose windows
  // never reach the screen) can `force` a timer-driven loop instead.
  // A fade keeps frames flowing even for a scene that stands still.
  reconsider() {
    const speed = this.to ? Math.max(this.to.speed, this.scrub ? this.scrub.look.speed : 0) : 0;
    const visible = !document.hidden || this.forced;
    const should = this.mode === 'run' && visible && (speed > 0.01 || !!this.from) && !this.lost && !!this.to;
    if (should && !this.running) this.start();
    if (!should && this.running) this.stop();
  }

  start() {
    this.running = true;
    this.last = performance.now();
    const step = now => {
      this.last = now;
      this.draw();
    };
    if (this.forced) {
      // Each tick is due a fixed period after the one before, not after
      // this one finished, so the rate holds instead of drifting under it.
      let due = performance.now();
      const tick = () => {
        if (!this.running) return;
        const now = performance.now();
        due = Math.max(due + 1000 / this.fps, now - 1000 / this.fps);
        this.timer = setTimeout(tick, Math.max(0, due - now));
        step(now);
      };
      tick();
      return;
    }
    const tick = now => {
      if (!this.running) return;
      this.frame = requestAnimationFrame(tick);
      // Cap the rate: a 120 Hz display asks four times a frame, and the
      // scene moves slowly enough that thirty a second look continuous.
      if (now - this.last < 1000 / this.fps - 2) return;
      step(now);
    };
    this.frame = requestAnimationFrame(tick);
  }

  stop() {
    this.running = false;
    cancelAnimationFrame(this.frame);
    clearTimeout(this.timer);
  }

  // One frame outside the loop — for a still mode, a resize while paused,
  // or new colours arriving while nothing is moving.
  poke() {
    if (this.running || !this.to || this.lost || this.mode === 'pause') return;
    this.draw();
  }

  draw() {
    const began = performance.now();
    if (this.resizeDue) this.resize();
    const now = wall();
    const fading = !!this.from;
    const look = this.current(now);
    if (look.style !== this.material.defines.STYLE) {
      // three keeps each style's program once compiled, so going back and
      // forth costs a compile only the first time.
      this.material.defines.STYLE = look.style;
      this.material.needsUpdate = true;
    }
    const u = this.material.uniforms;
    look.stops.forEach((c, i) => u.uC.value[i].setRGB(c[0], c[1], c[2]));
    u.uLight.value.setRGB(look.light[0], look.light[1], look.light[2]);
    u.uBlur.value = look.blur;
    u.uDark.value = look.dark;
    this.time = this.timeOf(look, now);
    u.uTime.value = this.time;
    this.renderer.render(this.scene, this.camera);
    const w = this.window;
    this.frames++;
    w.frames++;
    w.drawMs += performance.now() - began;
    if (began - w.since >= 1000) {
      w.fps = w.frames * 1000 / (began - w.since);
      w.avgDrawMs = w.drawMs / w.frames;
      w.since = began;
      w.frames = 0;
      w.drawMs = 0;
    }
    // A fade that just landed on a scene with no motion of its own: stop.
    if (fading && !this.from) this.reconsider();
  }
}

const backdrop = new Backdrop();
window.backdrop = {
  set: (p, fade) => backdrop.set(p, fade),
  toward: (p, x) => backdrop.toward(p, x),
  mode: m => backdrop.setMode(m),
  // Bench only: keep drawing on a timer even while WebKit calls the page
  // hidden (its windows never reach the screen), so motion can be measured.
  force: f => { backdrop.forced = !!f; backdrop.stop(); backdrop.reconsider(); },
  // For the bench: whether the loop runs and how far the clock has gone.
  status: () => ({ running: backdrop.running, mode: backdrop.mode, hidden: document.hidden, time: +backdrop.time.toFixed(3),
                   wall: +wall().toFixed(3), epoch, key: backdrop.to ? backdrop.to.key : null, style: backdrop.material ? backdrop.material.defines.STYLE : null,
                   fading: !!backdrop.from, scrub: backdrop.scrub ? +backdrop.scrub.x.toFixed(3) : null, segments: backdrop.segments,
                   fpsCap: backdrop.fps, frames: backdrop.frames, fps: +backdrop.window.fps.toFixed(1), drawMs: +backdrop.window.avgDrawMs.toFixed(2) }),
};
// A queued call from Swift that arrived before this module ran.
if (window.__backdropPending) { for (const [f, a] of window.__backdropPending) window.backdrop[f](...a); delete window.__backdropPending; }
