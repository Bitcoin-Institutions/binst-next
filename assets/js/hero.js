// reused-from: diegobianqui-site@28cf1ae:src/lab/constellation/viewer.js (2026-11-16) — adapted: kept the renderer guard, the pixel-ratio cap, the canvasTex/radial/sprite helpers and the comet idea; dropped the agent-session model, panels, controls and post-processing; nodes are laid out as a three-level tree
//
// The home page's living tree. Rust renders a static SVG of the featured
// institution plus `<div id="hero-scene" data-tree="…">`. When WebGL works and
// the visitor allows motion, this module draws the same tree in 3D inside that
// div: the institution anchored to a Bitcoin block, its process definitions
// below it, their instances on the lowest level, and comets for executed steps.
// If anything fails, the SVG simply stays.

import * as THREE from 'three';

const ORANGE = new THREE.Color('#f7931a');
const VIOLET = new THREE.Color('#8b9cff');
const GREEN = new THREE.Color('#62c46a');
const AMBER = new THREE.Color('#e6b34a');
const WHITE = new THREE.Color('#ffffff');

const ROOT_Y = 58;
const DEF_Y = -6;
const LEAF_Y = -62;
const CHAIN_Y = 122;
const MAX_PIXEL_RATIO = 1.5;

let renderer = null;
let scene = null;
let camera = null;
let rig = null;
// The chain of blocks does not turn with the tree, so it always reads as a chain.
let fixed = null;
let canvas = null;
let host = null;
let dataKey = null;
let rafId = 0;
let last = 0;
let onScreen = true;
let failed = false;
let resizeObserver = null;
let viewObserver = null;

let stars = [];
let edges = [];
let comets = [];
let spinners = [];
const pointer = { x: 0, y: 0 };
const tilt = { x: 0, y: 0 };

// ── textures ──────────────────────────────────────────────────────

function canvasTex(size, draw) {
  const c = document.createElement('canvas');
  c.width = c.height = size;
  draw(c.getContext('2d'), size);
  const t = new THREE.CanvasTexture(c);
  t.needsUpdate = true;
  return t;
}

function radial(stops, size = 128) {
  return canvasTex(size, (g, s) => {
    const gr = g.createRadialGradient(s / 2, s / 2, 0, s / 2, s / 2, s / 2);
    for (const [o, a] of stops) gr.addColorStop(o, `rgba(255,255,255,${a})`);
    g.fillStyle = gr;
    g.fillRect(0, 0, s, s);
  });
}

let TEX = null;

function sprite(parent, tex, col, scale, opacity = 1) {
  const m = new THREE.SpriteMaterial({
    map: tex, color: col.clone(), transparent: true, opacity,
    blending: THREE.AdditiveBlending, depthWrite: false, depthTest: false,
  });
  const s = new THREE.Sprite(m);
  s.scale.setScalar(scale);
  parent.add(s);
  return s;
}

// A text label that always faces the camera. The texture is owned by the
// label and freed when the tree is rebuilt.
function label(parent, text, cssColor, height, weight = 600) {
  const c = document.createElement('canvas');
  const g = c.getContext('2d');
  const font = `${weight} 46px -apple-system, "Segoe UI", Inter, sans-serif`;
  g.font = font;
  const width = Math.min(1024, Math.ceil(g.measureText(text).width) + 24);
  c.width = width;
  c.height = 64;
  const g2 = c.getContext('2d');
  g2.font = font;
  g2.fillStyle = cssColor;
  g2.textBaseline = 'middle';
  g2.fillText(text, 12, 34, width - 24);
  const tex = new THREE.CanvasTexture(c);
  tex.colorSpace = THREE.SRGBColorSpace;
  const m = new THREE.SpriteMaterial({ map: tex, transparent: true, depthWrite: false, depthTest: false });
  m.userData.ownsMap = true;
  const s = new THREE.Sprite(m);
  s.scale.set(height * (width / 64), height, 1);
  parent.add(s);
  return s;
}

function clip(text, max) {
  return text.length > max ? `${text.slice(0, max - 1).trimEnd()}…` : text;
}

// ── scene objects ─────────────────────────────────────────────────

function star(pos, col, size) {
  const group = new THREE.Group();
  group.position.copy(pos);
  rig.add(group);
  const glow = sprite(group, TEX.glow, col, size * 4.2, 0.55);
  const core = sprite(group, TEX.core, WHITE.clone().lerp(col, 0.35), size, 1);
  const s = { group, glow, core, size, phase: Math.random() * Math.PI * 2, pos };
  stars.push(s);
  return s;
}

function wire(parent, geometry, col, opacity) {
  const m = new THREE.LineBasicMaterial({ color: col, transparent: true, opacity, blending: THREE.AdditiveBlending, depthWrite: false });
  const mesh = new THREE.LineSegments(new THREE.WireframeGeometry(geometry), m);
  geometry.dispose();
  parent.add(mesh);
  spinners.push(mesh);
  return mesh;
}

function edge(parent, from, to, col, opacity, rate, cometColor) {
  const mid = from.clone().lerp(to, 0.5);
  mid.y += (from.y - to.y) * 0.18;
  const curve = new THREE.QuadraticBezierCurve3(from, mid, to);
  const geometry = new THREE.BufferGeometry().setFromPoints(curve.getPoints(24));
  const m = new THREE.LineBasicMaterial({ color: col, transparent: true, opacity, blending: THREE.AdditiveBlending, depthWrite: false });
  parent.add(new THREE.Line(geometry, m));
  edges.push({ parent, curve, rate, color: cometColor, due: Math.random() / Math.max(rate, 0.01) });
}

function emit(e) {
  const group = new THREE.Group();
  e.parent.add(group);
  sprite(group, TEX.glow, e.color, 15, 0.9);
  sprite(group, TEX.core, WHITE, 4, 1);
  comets.push({ group, curve: e.curve, t: 0, speed: 0.55 + Math.random() * 0.25 });
}

function disposeRig() {
  for (const group of [rig, fixed]) {
    group.traverse((o) => {
      if (o.geometry) o.geometry.dispose();
      if (o.material) {
        if (o.material.userData.ownsMap && o.material.map) o.material.map.dispose();
        o.material.dispose();
      }
    });
    group.clear();
  }
  stars = [];
  edges = [];
  comets = [];
  spinners = [];
}

// data: { name, height, tip, defs: [{ name, verified, instances: [{ progress, done }] }] }
function build(data) {
  disposeRig();
  const rootPos = new THREE.Vector3(0, ROOT_Y, 0);

  // The chain: a row of blocks with the inscription's block lit, and a beam
  // from that block down to the institution.
  const blocks = 9;
  const anchor = Math.floor(blocks / 2);
  for (let i = 0; i < blocks; i++) {
    const x = (i - anchor) * 30;
    const holder = new THREE.Group();
    holder.position.set(x, CHAIN_Y, -24);
    fixed.add(holder);
    const lit = i === anchor;
    wire(holder, new THREE.BoxGeometry(15, 15, 15), lit ? ORANGE : VIOLET, lit ? 0.95 : 0.22);
    if (lit) sprite(holder, TEX.glow, ORANGE, 46, 0.5);
    if (i > 0) {
      const link = new THREE.BufferGeometry().setFromPoints([new THREE.Vector3(x - 22.5, CHAIN_Y, -24), new THREE.Vector3(x - 7.5, CHAIN_Y, -24)]);
      fixed.add(new THREE.Line(link, new THREE.LineBasicMaterial({ color: VIOLET, transparent: true, opacity: 0.2, blending: THREE.AdditiveBlending, depthWrite: false })));
    }
  }
  const anchorPos = new THREE.Vector3(0, CHAIN_Y - 9, -24);
  edge(fixed, anchorPos, rootPos, ORANGE, 0.5, 0.22, ORANGE);
  if (data.height) {
    const l = label(fixed, `Bitcoin testnet4 · block ${Number(data.height).toLocaleString('en-US')}`, '#f7b35c', 7.5, 500);
    l.position.set(0, CHAIN_Y + 17, -24);
  }

  // Institution
  const root = star(rootPos, ORANGE, 26);
  wire(root.group, new THREE.IcosahedronGeometry(13, 0), ORANGE, 0.55);
  const rootLabel = label(rig, clip(data.name || 'Institution', 34), '#ffffff', 10.5);
  rootLabel.position.set(0, ROOT_Y + 23, 0);

  // Process definitions on an arc below the institution
  const defs = data.defs || [];
  const spread = Math.min(1.05, 2.6 / Math.max(defs.length, 1));
  const radius = defs.length > 1 ? 84 : 0;
  defs.forEach((def, i) => {
    const a = (i - (defs.length - 1) / 2) * spread;
    const pos = new THREE.Vector3(Math.sin(a) * radius, DEF_Y, Math.cos(a) * radius - radius * 0.55);
    const col = def.verified ? VIOLET : AMBER;
    const node = star(pos, col, 15);
    wire(node.group, new THREE.OctahedronGeometry(7.5, 0), col, 0.5);
    edge(rig, rootPos, pos, def.verified ? ORANGE : AMBER, def.verified ? 0.42 : 0.2, def.verified ? 0.3 : 0, ORANGE);
    const l = label(rig, clip(def.name || 'Process', 26), '#d9dcff', 8);
    l.position.set(pos.x, pos.y + 14.5, pos.z);

    const instances = def.instances || [];
    const ring = 13 + Math.min(instances.length, 8) * 3.2;
    instances.forEach((inst, j) => {
      const b = instances.length === 1 ? 0 : (j / instances.length) * Math.PI * 2 + i;
      const leaf = new THREE.Vector3(pos.x + Math.sin(b) * ring, LEAF_Y, pos.z + Math.cos(b) * ring);
      const leafCol = inst.done ? GREEN : VIOLET;
      const s = star(leaf, leafCol, 7 + 5 * (inst.progress || 0));
      s.glow.material.opacity = 0.3 + 0.4 * (inst.progress || 0);
      // A step in flight: busier while the instance is running, quiet when done.
      edge(rig, pos, leaf, leafCol, 0.36, inst.done ? 0.07 : 0.16 + 0.3 * (inst.progress || 0), leafCol);
    });
  });
}

// ── renderer ──────────────────────────────────────────────────────

function init() {
  if (failed) return false;
  try {
    renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true, powerPreference: 'high-performance' });
  } catch (err) {
    failed = true;
    return false;
  }
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, MAX_PIXEL_RATIO));
  renderer.setClearColor(0x000000, 0);
  canvas = renderer.domElement;
  canvas.setAttribute('role', 'img');
  canvas.setAttribute('aria-label', 'Animated view of the featured institution tree');
  // A lost context cannot be drawn to; fall back to the static tree.
  canvas.addEventListener('webglcontextlost', (event) => {
    event.preventDefault();
    stop();
    failed = true;
    if (host) host.closest('.hero-stage')?.classList.remove('is-3d');
  });

  TEX = {
    glow: radial([[0, 1], [0.05, 0.85], [0.16, 0.36], [0.38, 0.1], [0.65, 0.025], [1, 0]]),
    core: radial([[0, 1], [0.22, 0.95], [0.45, 0.4], [0.7, 0.07], [1, 0]], 64),
  };
  scene = new THREE.Scene();
  camera = new THREE.PerspectiveCamera(40, 1, 1, 3000);
  rig = new THREE.Group();
  fixed = new THREE.Group();
  scene.add(rig, fixed);

  // A sparse field of distant points for depth.
  const count = 320;
  const positions = new Float32Array(count * 3);
  for (let i = 0; i < count; i++) {
    const v = new THREE.Vector3().randomDirection().multiplyScalar(360 + Math.random() * 520);
    positions.set([v.x, v.y * 0.7, v.z], i * 3);
  }
  const field = new THREE.BufferGeometry();
  field.setAttribute('position', new THREE.BufferAttribute(positions, 3));
  scene.add(new THREE.Points(field, new THREE.PointsMaterial({
    color: 0x9aa3d8, size: 2.2, sizeAttenuation: true, transparent: true, opacity: 0.55, depthWrite: false,
  })));

  window.addEventListener('pointermove', (event) => {
    pointer.x = (event.clientX / window.innerWidth) * 2 - 1;
    pointer.y = (event.clientY / window.innerHeight) * 2 - 1;
  }, { passive: true });
  document.addEventListener('visibilitychange', () => (document.hidden ? stop() : play()));
  resizeObserver = new ResizeObserver(resize);
  viewObserver = new IntersectionObserver((entries) => {
    onScreen = entries.some((entry) => entry.isIntersecting);
    if (onScreen) play(); else stop();
  });
  return true;
}

function resize() {
  if (!host || !renderer) return;
  const w = Math.max(host.clientWidth, 1);
  const h = Math.max(host.clientHeight, 1);
  renderer.setSize(w, h, false);
  canvas.style.width = '100%';
  canvas.style.height = '100%';
  camera.aspect = w / h;
  // Keep the whole tree in frame on narrow stages by stepping back.
  camera.position.set(0, 26, 330 / Math.min(1, Math.max(camera.aspect, 0.55) / 1.05));
  camera.lookAt(0, 28, 0);
  camera.updateProjectionMatrix();
}

function frame(time) {
  rafId = requestAnimationFrame(frame);
  const dt = Math.min((time - last) / 1000, 0.05);
  last = time;

  tilt.x += (pointer.y * 0.12 - tilt.x) * 0.04;
  tilt.y += (pointer.x * 0.35 - tilt.y) * 0.04;
  rig.rotation.y += dt * 0.11;
  rig.rotation.x = tilt.x;
  fixed.rotation.x = tilt.x;
  scene.rotation.y = tilt.y;

  const t = time / 1000;
  for (const s of stars) {
    const pulse = 1 + 0.07 * Math.sin(t * 1.6 + s.phase);
    s.core.scale.setScalar(s.size * pulse);
    s.glow.scale.setScalar(s.size * 4.2 * pulse);
  }
  for (const m of spinners) {
    m.rotation.x += dt * 0.25;
    m.rotation.y += dt * 0.4;
  }
  for (const e of edges) {
    if (e.rate <= 0) continue;
    e.due -= dt;
    if (e.due <= 0) {
      emit(e);
      e.due = (0.6 + Math.random() * 0.8) / e.rate;
    }
  }
  for (let k = comets.length - 1; k >= 0; k--) {
    const c = comets[k];
    c.t += dt * c.speed;
    if (c.t >= 1) {
      c.group.traverse((o) => o.material && o.material.dispose());
      c.group.removeFromParent();
      comets.splice(k, 1);
      continue;
    }
    c.curve.getPoint(c.t, c.group.position);
    const fade = Math.sin(Math.PI * c.t);
    c.group.children[0].material.opacity = 0.9 * fade;
    c.group.children[1].material.opacity = fade;
  }
  renderer.render(scene, camera);
}

function play() {
  if (rafId || !host || !renderer || failed || !onScreen || document.hidden) return;
  last = performance.now();
  rafId = requestAnimationFrame(frame);
}

function stop() {
  if (rafId) cancelAnimationFrame(rafId);
  rafId = 0;
}

// ── keeping up with the page ──────────────────────────────────────

// Rust replaces the view's HTML when its content changes, so the stage
// element comes and goes. The canvas is kept and moved into the new one.
function sync() {
  const el = document.getElementById('hero-scene');
  if (!el) {
    stop();
    host = null;
    return;
  }
  if (!renderer && !init()) return;
  if (failed) return;
  if (el !== host) {
    if (host) {
      resizeObserver.unobserve(host);
      viewObserver.unobserve(host);
    }
    host = el;
    host.appendChild(canvas);
    resizeObserver.observe(host);
    viewObserver.observe(host);
    resize();
  }
  const key = el.getAttribute('data-tree') || '';
  if (key !== dataKey) {
    let data = null;
    try {
      data = JSON.parse(key);
    } catch (err) {
      data = null;
    }
    if (!data || !data.name) {
      // Nothing to draw yet: leave the static placeholder in view.
      el.closest('.hero-stage')?.classList.remove('is-3d');
      dataKey = null;
      stop();
      return;
    }
    dataKey = key;
    build(data);
  }
  el.closest('.hero-stage')?.classList.add('is-3d');
  play();
}

export function start() {
  const view = document.getElementById('view');
  if (!view) return;
  new MutationObserver(sync).observe(view, { childList: true, subtree: true });
  sync();
}
