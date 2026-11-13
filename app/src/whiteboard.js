// Whiteboard mode for "explain" walkthroughs: when nothing on screen relates
// to the topic, the model sends shapes for a blank board instead (see
// backend/app/services/whiteboard.py for the contract), and they are drawn
// here, hand-sketched, while each step is spoken. The board builds up: shapes
// from earlier steps stay, faded, and the current step's shapes are drawn
// stroke by stroke with a glowing pen tip, labels written in after.
//
// Same calling pattern as storyboard-stage.js: show(steps, index, title) per
// step, finish(), pause(), clear(). Each call starts a new generation so an
// animation from an older call stops where it is.
//
// The geometry helpers at the top have no DOM and are tested in Node
// (app/tests/whiteboard.test.mjs).

const SVG_NS = 'http://www.w3.org/2000/svg';
export const BOARD_W = 1000;
export const BOARD_H = 600;

export const COLORS = {
  white: '#eef1f7',
  blue: '#7aa2ff',
  red: '#ff6b81',
  green: '#5fd39a',
  yellow: '#ffd166',
};

const FONT_SIZES = { s: 20, m: 27, l: 40 };

const clamp = (n, lo, hi) => Math.max(lo, Math.min(hi, n));
const lerp = (a, b, t) => a + (b - a) * t;
const easeInOut = (t) => (t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2);

// ---------------------------------------------------------------------------
// Geometry (no DOM)

/// Small deterministic random source, so a shape wobbles the same way every
/// time it is redrawn (replaying a step, or the board rebuilding) instead of
/// visibly jumping.
export function seededRandom(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export function hashSeed(text) {
  let h = 2166136261;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

const jitter = (rand, amount) => (rand() * 2 - 1) * amount;

/// One hand-drawn edge: a slight bow in the middle and a small overshoot at
/// the end, written as a quadratic segment continuing the current path.
function roughEdge(x1, y1, x2, y2, rand) {
  const len = Math.hypot(x2 - x1, y2 - y1) || 1;
  const ux = (x2 - x1) / len, uy = (y2 - y1) / len;
  const bow = jitter(rand, Math.min(5, len * 0.02));
  const over = 2 + rand() * 4;
  const cx = (x1 + x2) / 2 - uy * bow;
  const cy = (y1 + y2) / 2 + ux * bow;
  const ex = x2 + ux * over + jitter(rand, 1.2);
  const ey = y2 + uy * over + jitter(rand, 1.2);
  return `Q ${cx.toFixed(1)} ${cy.toFixed(1)} ${ex.toFixed(1)} ${ey.toFixed(1)}`;
}

/// A box outline as one continuous stroke, clockwise from the top-left, so it
/// can be drawn on in a single pass.
export function boxPath(s, rand) {
  const { x, y, w, h } = s;
  const corners = [[x + w, y], [x + w, y + h], [x, y + h], [x, y]];
  let px = x + jitter(rand, 1.5), py = y + jitter(rand, 1.5);
  let d = `M ${px.toFixed(1)} ${py.toFixed(1)}`;
  for (const [cx, cy] of corners) {
    d += ' ' + roughEdge(px, py, cx, cy, rand);
    px = cx; py = cy;
  }
  return d;
}

/// A circle drawn the way a hand draws one: starting up and to the left,
/// slightly uneven, and running a little past where it started.
export function circlePath(cx, cy, r, rand, ry = r) {
  const start = -Math.PI * 0.62 + jitter(rand, 0.2);
  const sweep = Math.PI * 2 + 0.28 + rand() * 0.2;
  const steps = 40;
  const phase = rand() * Math.PI * 2;
  let d = '';
  for (let i = 0; i <= steps; i++) {
    const a = start + (sweep * i) / steps;
    // A slow, gentle drift in the radius (a hand, not a compass), growing a
    // touch over the stroke so the overlap at the end shows.
    const k = 1 + Math.sin(a * 2 + phase) * 0.018 + (i / steps) * 0.035;
    const px = cx + Math.cos(a) * r * k, py = cy + Math.sin(a) * ry * k;
    d += `${i ? ' L' : 'M'} ${px.toFixed(1)} ${py.toFixed(1)}`;
  }
  return d;
}

export function fontSize(size) {
  return FONT_SIZES[size] || FONT_SIZES.m;
}

/// A label's font size so it fits inside `width` (handwriting is ~0.5em per
/// character), never below a readable minimum.
export function fitFont(text, width, preferred = FONT_SIZES.m) {
  const needed = (text || '').length * 0.52;
  if (!needed) return preferred;
  return Math.round(clamp(width / needed, 14, preferred));
}

/// The area a shape covers, for arrows and highlights to aim at.
export function shapeBounds(s) {
  if (s.type === 'box') return { x: s.x, y: s.y, w: s.w, h: s.h };
  if (s.type === 'circle') return { x: s.x - s.r, y: s.y - s.r, w: s.r * 2, h: s.r * 2 };
  if (s.type === 'text') {
    const fs = fontSize(s.size);
    const w = s.text.length * fs * 0.52;
    return { x: s.x - w / 2, y: s.y - fs * 0.6, w, h: fs * 1.2 };
  }
  return null;
}

/// Where a line from `toward` meets the edge of shape `s`, pulled back a few
/// units so the arrow stops just short of the outline.
export function edgePoint(s, toward, gap = 8) {
  const b = shapeBounds(s);
  const cx = b.x + b.w / 2, cy = b.y + b.h / 2;
  const dx = toward.x - cx, dy = toward.y - cy;
  const len = Math.hypot(dx, dy);
  if (len < 1e-6) return { x: cx, y: cy };
  const ux = dx / len, uy = dy / len;
  let dist;
  if (s.type === 'circle') {
    dist = s.r;
  } else {
    const tx = ux ? (b.w / 2) / Math.abs(ux) : Infinity;
    const ty = uy ? (b.h / 2) / Math.abs(uy) : Infinity;
    dist = Math.min(tx, ty);
  }
  dist = Math.min(len, dist + gap);
  return { x: cx + ux * dist, y: cy + uy * dist };
}

function centre(s) {
  const b = shapeBounds(s);
  return { x: b.x + b.w / 2, y: b.y + b.h / 2 };
}

/// The two ends of an arrow, given the shapes drawn so far (by id). null if
/// an end names a shape that doesn't exist.
export function resolveArrow(arrow, byId) {
  const look = (ref) => (typeof ref === 'string' ? byId.get(ref) || null : ref);
  const a = look(arrow.from), b = look(arrow.to);
  if (!a || !b) return null;
  const aIsShape = typeof arrow.from === 'string', bIsShape = typeof arrow.to === 'string';
  const aCentre = aIsShape ? centre(a) : a;
  const bCentre = bIsShape ? centre(b) : b;
  const start = aIsShape ? edgePoint(a, bCentre) : aCentre;
  const end = bIsShape ? edgePoint(b, aCentre) : bCentre;
  if (Math.hypot(end.x - start.x, end.y - start.y) < 4) return null;
  return { start, end };
}

/// A gently curved arrow shaft, its arrowhead strokes, and where its label
/// sits (beside the middle of the curve, on the outside of the bend).
export function arrowGeometry(start, end, rand) {
  const dx = end.x - start.x, dy = end.y - start.y;
  const len = Math.hypot(dx, dy) || 1;
  const px = -dy / len, py = dx / len;
  // Always bend to the same side of the direction of travel, so an arrow
  // back the other way (a reply) takes the other side instead of overlapping.
  const bend = clamp(len * 0.1, 6, 50) * (0.85 + rand() * 0.3);
  const c = { x: (start.x + end.x) / 2 + px * bend, y: (start.y + end.y) / 2 + py * bend };
  const shaft = `M ${start.x.toFixed(1)} ${start.y.toFixed(1)} Q ${c.x.toFixed(1)} ${c.y.toFixed(1)} ${end.x.toFixed(1)} ${end.y.toFixed(1)}`;

  // Arrowhead along the curve's final direction.
  const tx = end.x - c.x, ty = end.y - c.y;
  const tl = Math.hypot(tx, ty) || 1;
  const ux = tx / tl, uy = ty / tl;
  const head = 16;
  const wing = (sign) => ({
    x: end.x - ux * head + -uy * head * 0.55 * sign,
    y: end.y - uy * head + ux * head * 0.55 * sign,
  });
  const w1 = wing(1), w2 = wing(-1);
  const heads = `M ${w1.x.toFixed(1)} ${w1.y.toFixed(1)} L ${end.x.toFixed(1)} ${end.y.toFixed(1)} L ${w2.x.toFixed(1)} ${w2.y.toFixed(1)}`;

  // Midpoint of the quadratic curve, pushed out past the bend.
  const mid = { x: 0.25 * start.x + 0.5 * c.x + 0.25 * end.x, y: 0.25 * start.y + 0.5 * c.y + 0.25 * end.y };
  const label = { x: mid.x + px * 22, y: mid.y + py * 22 };
  return { shaft, heads, label };
}

/// Which step's shapes are "current": the step being spoken if it draws
/// anything, otherwise the most recent one that did, so a narration-only
/// step leaves the last drawing in focus instead of fading everything.
export function focusStep(steps, index) {
  for (let i = index; i >= 0; i--) {
    if (steps[i] && Array.isArray(steps[i].board) && steps[i].board.length) return i;
  }
  return -1;
}

export function usesBoard(steps, index) {
  return focusStep(steps, index) !== -1;
}

// ---------------------------------------------------------------------------
// DOM side

function el(name, attrs = {}, parent = null) {
  const node = document.createElementNS(SVG_NS, name);
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, String(v));
  if (parent) parent.appendChild(node);
  return node;
}

export function createWhiteboard({ panel, backdrop, svg, title }) {
  const reducedMotion = () => window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  const dur = (ms) => (reducedMotion() ? 0 : ms);

  let gen = 0;
  let clipCount = 0;
  let layers = null;
  let shown = -1; // focus step last drawn, for the fade of the step before it

  function scaffold() {
    if (layers) return layers;
    svg.innerHTML = '';
    const defs = el('defs', {}, svg);
    const grid = el('pattern', { id: 'wb-grid', width: 40, height: 40, patternUnits: 'userSpaceOnUse' }, defs);
    el('circle', { cx: 20, cy: 20, r: 1.1, fill: 'rgba(255,255,255,0.08)' }, grid);
    const glow = el('filter', { id: 'wb-glow', x: '-50%', y: '-50%', width: '200%', height: '200%' }, defs);
    el('feGaussianBlur', { stdDeviation: 2.2, result: 'b' }, glow);
    const merge = el('feMerge', {}, glow);
    el('feMergeNode', { in: 'b' }, merge);
    el('feMergeNode', { in: 'SourceGraphic' }, merge);
    el('rect', { x: 0, y: 0, width: BOARD_W, height: BOARD_H, fill: 'url(#wb-grid)' }, svg);
    const old = el('g', { class: 'wb-old' }, svg);
    const now = el('g', { class: 'wb-now' }, svg);
    const pen = el('circle', { class: 'wb-pen', r: 5.5, cx: -50, cy: -50, filter: 'url(#wb-glow)' }, svg);
    layers = { defs, old, now, pen };
    return layers;
  }

  function tween(myGen, ms, onUpdate, ease = easeInOut) {
    return new Promise((resolve) => {
      if (ms <= 0) {
        if (myGen === gen) onUpdate(1);
        resolve();
        return;
      }
      const start = performance.now();
      const frame = (now) => {
        if (myGen !== gen) return resolve();
        // A frame's timestamp can predate `start` (it marks the frame's
        // beginning), so the first progress value can be slightly negative.
        const t = clamp((now - start) / ms, 0, 1);
        onUpdate(ease(t));
        if (t < 1) requestAnimationFrame(frame);
        else resolve();
      };
      requestAnimationFrame(frame);
    });
  }

  const wait = (myGen, ms) => tween(myGen, ms, () => {});

  function stroke(d, color, parent, width = 3) {
    return el('path', {
      d, fill: 'none', stroke: color, 'stroke-width': width,
      'stroke-linecap': 'round', 'stroke-linejoin': 'round', class: 'wb-stroke',
    }, parent);
  }

  // Draws a path on from its start, the pen riding the tip.
  async function drawOn(myGen, path, ms, { pen = true } = {}) {
    if (ms <= 0) return;
    const total = path.getTotalLength();
    path.setAttribute('stroke-dasharray', `${total} ${total}`);
    path.setAttribute('stroke-dashoffset', total);
    if (pen) layers.pen.classList.add('on');
    await tween(myGen, ms, (t) => {
      path.setAttribute('stroke-dashoffset', total * (1 - t));
      if (pen) {
        const p = path.getPointAtLength(total * t);
        layers.pen.setAttribute('cx', p.x);
        layers.pen.setAttribute('cy', p.y);
      }
    });
    if (myGen === gen) {
      path.removeAttribute('stroke-dasharray');
      path.removeAttribute('stroke-dashoffset');
    }
  }

  function addText(x, y, text, color, size, parent, weight = 'normal') {
    const t = el('text', {
      x, y, fill: color, 'font-size': size, 'text-anchor': 'middle',
      'dominant-baseline': 'central', class: 'wb-text', 'font-weight': weight,
    }, parent);
    t.textContent = text;
    return t;
  }

  // Writes text in from left to right behind a widening clip.
  async function writeOn(myGen, textEl, ms) {
    if (ms <= 0) return;
    const box = textEl.getBBox();
    const id = `wb-clip-${++clipCount}`;
    const clip = el('clipPath', { id }, layers.defs);
    const rect = el('rect', { x: box.x - 4, y: box.y - 6, width: 0, height: box.height + 12 }, clip);
    textEl.setAttribute('clip-path', `url(#${id})`);
    layers.pen.classList.add('on');
    await tween(myGen, ms, (t) => {
      rect.setAttribute('width', (box.width + 8) * t);
      layers.pen.setAttribute('cx', box.x + box.width * t);
      layers.pen.setAttribute('cy', box.y + box.height * 0.7);
    }, (t) => t);
    if (myGen === gen) {
      textEl.removeAttribute('clip-path');
      clip.remove();
    }
  }

  const writeTime = (text) => dur(clamp((text || '').length * 32, 240, 1000));

  // Builds one shape into `parent` straight away (registering boxes, circles
  // and text by id, so arrows and highlights after it can find them) and
  // returns how to draw it on. Without `animate` it is built complete and
  // the returned player does nothing.
  function buildShape(s, byId, parent, animate, seedKey) {
    const rand = seededRandom(hashSeed(seedKey));
    const color = COLORS[s.color] || (s.type === 'highlight' ? COLORS.yellow : COLORS.white);
    const g = el('g', { class: `wb-shape wb-${s.type}` }, parent);
    const none = async () => {};

    if (s.type === 'box' || s.type === 'circle') {
      if (s.id) byId.set(s.id, s);
      const isBox = s.type === 'box';
      const fill = isBox
        ? el('rect', { x: s.x, y: s.y, width: s.w, height: s.h, rx: 6, fill: color, 'fill-opacity': animate ? 0 : 0.1 }, g)
        : el('circle', { cx: s.x, cy: s.y, r: s.r, fill: color, 'fill-opacity': animate ? 0 : 0.1 }, g);
      const main = stroke(isBox ? boxPath(s, rand) : circlePath(s.x, s.y, s.r, rand), color, g);
      // A second, fainter pass with its own wobble: the sketchy double line.
      const ghost = stroke(isBox ? boxPath(s, rand) : circlePath(s.x, s.y, s.r * 1.015, rand), color, g, 1.4);
      ghost.setAttribute('opacity', animate ? 0 : 0.35);
      const labelSize = isBox ? fitFont(s.label, s.w - 16) : fitFont(s.label, s.r * 1.7);
      const label = s.label
        ? addText(isBox ? s.x + s.w / 2 : s.x, isBox ? s.y + s.h / 2 : s.y, s.label, color, labelSize, g)
        : null;
      if (!animate) return none;
      main.setAttribute('opacity', 0);
      if (label) label.setAttribute('opacity', 0);
      return async (myGen) => {
        main.setAttribute('opacity', 1);
        await drawOn(myGen, main, dur(isBox ? 700 : 620));
        if (myGen !== gen) return;
        ghost.setAttribute('opacity', 0.35);
        await Promise.all([
          drawOn(myGen, ghost, dur(260), { pen: false }),
          tween(myGen, dur(320), (t) => fill.setAttribute('fill-opacity', 0.1 * t)),
        ]);
        if (label && myGen === gen) {
          label.setAttribute('opacity', 1);
          await writeOn(myGen, label, writeTime(s.label));
        }
      };
    }

    if (s.type === 'text') {
      if (s.id) byId.set(s.id, s);
      const t = addText(s.x, s.y, s.text, color, fontSize(s.size), g, s.size === 'l' ? 'bold' : 'normal');
      if (!animate) return none;
      t.setAttribute('opacity', 0);
      return async (myGen) => {
        t.setAttribute('opacity', 1);
        await writeOn(myGen, t, writeTime(s.text));
      };
    }

    if (s.type === 'arrow') {
      const ends = resolveArrow(s, byId);
      if (!ends) { g.remove(); return none; }
      const geo = arrowGeometry(ends.start, ends.end, rand);
      const shaft = stroke(geo.shaft, color, g, 2.6);
      const heads = stroke(geo.heads, color, g, 2.6);
      const label = s.label ? addText(geo.label.x, geo.label.y, s.label, color, FONT_SIZES.s, g) : null;
      if (!animate) return none;
      for (const part of [shaft, heads, label]) if (part) part.setAttribute('opacity', 0);
      return async (myGen) => {
        shaft.setAttribute('opacity', 1);
        await drawOn(myGen, shaft, dur(480));
        if (myGen !== gen) return;
        heads.setAttribute('opacity', 1);
        await drawOn(myGen, heads, dur(160));
        if (label && myGen === gen) {
          label.setAttribute('opacity', 1);
          await writeOn(myGen, label, writeTime(s.label));
        }
      };
    }

    if (s.type === 'highlight') {
      const target = byId.get(s.target);
      if (!target) { g.remove(); return none; }
      const b = shapeBounds(target);
      // A loose ring around the target, stretched to its proportions.
      const ring = stroke(circlePath(b.x + b.w / 2, b.y + b.h / 2, b.w / 2 + 20, rand, b.h / 2 + 16), color, g, 3.2);
      g.classList.add('wb-highlight');
      if (!animate) return none;
      ring.setAttribute('opacity', 0);
      return async (myGen) => {
        ring.setAttribute('opacity', 1);
        await drawOn(myGen, ring, dur(560), { pen: false });
      };
    }

    g.remove();
    return none;
  }

  function buildStatic(step, stepIndex, byId, parent) {
    const shapes = (step && step.board) || [];
    shapes.forEach((s, i) => buildShape(s, byId, parent, false, `${stepIndex}:${i}:${JSON.stringify(s)}`));
  }

  // Each shape is built only when its turn comes, so the board never shows
  // a shape before the pen gets there.
  async function playStep(myGen, step, stepIndex, byId, parent) {
    const shapes = (step && step.board) || [];
    for (let i = 0; i < shapes.length; i++) {
      if (myGen !== gen) return;
      const play = buildShape(shapes[i], byId, parent, true, `${stepIndex}:${i}:${JSON.stringify(shapes[i])}`);
      await play(myGen);
      await wait(myGen, dur(90));
    }
  }

  function open(titleText) {
    if (!panel.classList.contains('hidden')) return;
    title.textContent = titleText || '';
    backdrop.classList.remove('hidden');
    panel.classList.remove('hidden');
    panel.classList.remove('enter');
    void panel.offsetWidth; // restart the entrance animation
    panel.classList.add('enter');
  }

  // Rebuilds the board for `index`: everything before the focus step static
  // (faded unless `final`), the focus step drawn on if `animate`.
  function render(myGen, steps, index, { animate, final }) {
    scaffold();
    const focus = focusStep(steps, index);
    const byId = new Map();
    layers.old.innerHTML = '';
    layers.now.innerHTML = '';
    layers.old.classList.toggle('final', !!final);
    for (let i = 0; i < focus; i++) buildStatic(steps[i], i, byId, layers.old);
    // The shapes that were current a moment ago fade down instead of
    // appearing already faded.
    if (!final && animate && focus === shown + 1 && !reducedMotion()) {
      layers.old.style.opacity = '1';
      void layers.old.getBoundingClientRect();
      requestAnimationFrame(() => { layers.old.style.opacity = ''; });
    }
    shown = focus;
    if (focus === -1) return Promise.resolve();
    if (animate && focus === index) return playStep(myGen, steps[focus], focus, byId, layers.now);
    buildStatic(steps[focus], focus, byId, layers.now);
    return Promise.resolve();
  }

  return {
    get active() {
      return !panel.classList.contains('hidden');
    },

    /// Shows step `index`: the board opens if needed, earlier drawings stay,
    /// and this step's shapes are drawn on while it is spoken.
    show(steps, index, titleText) {
      const myGen = ++gen;
      open(titleText);
      layers && layers.pen.classList.remove('on');
      render(myGen, steps, index, { animate: true, final: false }).then(() => {
        if (myGen === gen && layers) layers.pen.classList.remove('on');
      });
    },

    /// The walkthrough ended: the whole diagram at full strength, until the
    /// overlay closes.
    finish(steps) {
      if (!this.active) return;
      const myGen = ++gen;
      render(myGen, steps, steps.length - 1, { animate: false, final: true });
      layers.pen.classList.remove('on');
    },

    /// A question interrupted the walkthrough: finish drawing the current
    /// step at once so the board is complete while the answer shows.
    pause(steps, index) {
      if (!this.active) return;
      const myGen = ++gen;
      render(myGen, steps, index, { animate: false, final: false });
      layers.pen.classList.remove('on');
      shown = -1;
    },

    clear() {
      gen++;
      shown = -1;
      layers = null;
      svg.innerHTML = '';
      title.textContent = '';
      panel.classList.add('hidden');
      panel.classList.remove('enter');
      backdrop.classList.add('hidden');
    },
  };
}
