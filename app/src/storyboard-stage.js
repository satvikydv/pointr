// Draws an "explain" walkthrough over the screen: the shape for the step
// being spoken, the shapes of earlier steps as a numbered trail, a spotlight
// that dims everything except the current target, and a ghost cursor that
// travels from one target to the next before each shape is drawn.
//
// main.js only calls show(steps, index) for each step, finish() when the
// walkthrough ends, pause() when a question interrupts it, and clear(). Every
// call starts a new "generation"; animations still running from an older one
// notice and stop, so steps can change mid-animation (skip, pause, dismiss)
// without leftovers.
//
// The geometry helpers at the top have no DOM, so they are tested in Node
// (app/tests/storyboard-stage.test.mjs).

const SVG_NS = 'http://www.w3.org/2000/svg';
const POINT_COLOR = '#ff5470';
const AREA_COLOR = '#5B8CFF';
const BOX_OUTSET = 6;

const clampUnit = (n) => Math.max(0, Math.min(1, Number(n) || 0));
const clamp = (n, lo, hi) => Math.max(lo, Math.min(hi, n));
const lerp = (a, b, t) => a + (b - a) * t;

export const easeInOutCubic = (t) => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);
export const easeOutCubic = (t) => 1 - Math.pow(1 - t, 3);
export const easeOutBack = (t) => {
  const c1 = 1.70158;
  const c3 = c1 + 1;
  return 1 + c3 * Math.pow(t - 1, 3) + c1 * Math.pow(t - 1, 2);
};

/// A step's annotation in screen pixels, or null when it has none. A box or
/// line too small to see is drawn as a point at its centre instead.
export function shapeGeometry(step, w, h) {
  if (!step || !step.shape) return null;
  const x1 = clampUnit(step.x_norm) * w;
  const y1 = clampUnit(step.y_norm) * h;
  if (step.shape === 'point') return { kind: 'point', x: x1, y: y1, anchor: { x: x1, y: y1 } };
  if (step.x2_norm == null || step.y2_norm == null) return null;
  const x2 = clampUnit(step.x2_norm) * w;
  const y2 = clampUnit(step.y2_norm) * h;

  if (step.shape === 'box') {
    const x = Math.min(x1, x2);
    const y = Math.min(y1, y2);
    const bw = Math.abs(x2 - x1);
    const bh = Math.abs(y2 - y1);
    if (bw < 4 || bh < 4) {
      const cx = x + bw / 2, cy = y + bh / 2;
      return { kind: 'point', x: cx, y: cy, anchor: { x: cx, y: cy } };
    }
    // Model boxes hug their content; a few pixels of air keeps the outline
    // off the text it surrounds.
    const ox = Math.max(0, x - BOX_OUTSET), oy = Math.max(0, y - BOX_OUTSET);
    const ow = Math.min(w, x + bw + BOX_OUTSET) - ox, oh = Math.min(h, y + bh + BOX_OUTSET) - oy;
    return { kind: 'box', x: ox, y: oy, w: ow, h: oh, anchor: { x: ox, y: oy } };
  }
  if (step.shape === 'line') {
    if (Math.hypot(x2 - x1, y2 - y1) < 6) return { kind: 'point', x: x1, y: y1, anchor: { x: x1, y: y1 } };
    return { kind: 'line', x1, y1, x2, y2, anchor: { x: x1, y: y1 } };
  }
  return null;
}

/// The area the spotlight leaves undimmed around a shape.
export function focusRect(geo) {
  if (!geo) return null;
  if (geo.kind === 'point') return { x: geo.x - 52, y: geo.y - 52, w: 104, h: 104 };
  if (geo.kind === 'box') return { x: geo.x - 14, y: geo.y - 14, w: geo.w + 28, h: geo.h + 28 };
  const pad = 30;
  const x = Math.min(geo.x1, geo.x2) - pad;
  const y = Math.min(geo.y1, geo.y2) - pad;
  return { x, y, w: Math.abs(geo.x2 - geo.x1) + pad * 2, h: Math.abs(geo.y2 - geo.y1) + pad * 2 };
}

/// Where a shape's step number sits, kept on screen.
export function badgePosition(geo, w, h) {
  let x, y;
  if (geo.kind === 'point') { x = geo.x + 16; y = geo.y - 16; }
  else if (geo.kind === 'box') { x = geo.x; y = geo.y; }
  else { x = (geo.x1 + geo.x2) / 2; y = (geo.y1 + geo.y2) / 2 - 16; }
  return { x: clamp(x, 12, w - 12), y: clamp(y, 12, h - 12) };
}

/// The ghost cursor flies on a gentle arc, not a straight line: a quadratic
/// curve whose control point sits off to the side of the midpoint.
export function travelControl(from, to) {
  const dx = to.x - from.x, dy = to.y - from.y;
  const dist = Math.hypot(dx, dy);
  if (dist < 1) return { x: from.x, y: from.y };
  const bend = Math.min(120, dist * 0.22);
  return { x: (from.x + to.x) / 2 + (-dy / dist) * bend, y: (from.y + to.y) / 2 + (dx / dist) * bend };
}

export function bezierPoint(p0, c, p1, t) {
  const u = 1 - t;
  return { x: u * u * p0.x + 2 * u * t * c.x + t * t * p1.x, y: u * u * p0.y + 2 * u * t * c.y + t * t * p1.y };
}

/// Longer trips take longer, within limits; no trip at all takes no time.
export function travelDuration(from, to) {
  const dist = Math.hypot(to.x - from.x, to.y - from.y);
  if (dist < 2) return 0;
  return clamp(280 + dist * 0.32, 320, 760);
}

export function rectsOverlap(a, b) {
  return a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
}

/// Where the ghost cursor waits once a shape is drawn: just off its tip, so
/// it doesn't sit on the very spot it pointed out.
export function restPosition(p, w, h) {
  return { x: clamp(p.x + 22, 0, w - 16), y: clamp(p.y + 26, 0, h - 22) };
}

export function lerpRect(a, b, t) {
  return { x: lerp(a.x, b.x, t), y: lerp(a.y, b.y, t), w: lerp(a.w, b.w, t), h: lerp(a.h, b.h, t) };
}

// ---------------------------------------------------------------------------
// DOM side

function el(name, attrs = {}, parent = null) {
  const node = document.createElementNS(SVG_NS, name);
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, String(v));
  if (parent) parent.appendChild(node);
  return node;
}

function setRect(node, r) {
  node.setAttribute('x', r.x);
  node.setAttribute('y', r.y);
  node.setAttribute('width', Math.max(0, r.w));
  node.setAttribute('height', Math.max(0, r.h));
}

function arrowHead(x1, y1, x2, y2) {
  const len = Math.hypot(x2 - x1, y2 - y1) || 1;
  const ux = (x2 - x1) / len, uy = (y2 - y1) / len;
  const px = -uy, py = ux;
  const headLen = 12, headWidth = 5;
  const bx = x2 - ux * headLen, by = y2 - uy * headLen;
  return `${x2},${y2} ${(bx + px * headWidth).toFixed(1)},${(by + py * headWidth).toFixed(1)} ${(bx - px * headWidth).toFixed(1)},${(by - py * headWidth).toFixed(1)}`;
}

/// A shape in its finished state. Animations start from this and set their
/// own starting values.
function buildShape(geo, parent) {
  const g = el('g', { class: `sb-shape sb-${geo.kind}` }, parent);
  if (geo.kind === 'point') {
    g.setAttribute('transform', `translate(${geo.x} ${geo.y})`);
    el('circle', { class: 'sb-point-ring', r: 10, fill: 'none', stroke: POINT_COLOR, 'stroke-width': 2 }, g);
    el('circle', { class: 'sb-point-dot', r: 5, fill: POINT_COLOR }, g);
  } else if (geo.kind === 'box') {
    const rect = el('rect', { rx: 6, fill: AREA_COLOR, 'fill-opacity': 0.12, stroke: AREA_COLOR, 'stroke-width': 2 }, g);
    setRect(rect, geo);
  } else {
    el('path', { d: `M ${geo.x1} ${geo.y1} L ${geo.x2} ${geo.y2}`, stroke: AREA_COLOR, 'stroke-width': 2.5, 'stroke-linecap': 'round', fill: 'none' }, g);
    el('polygon', { points: arrowHead(geo.x1, geo.y1, geo.x2, geo.y2), fill: AREA_COLOR }, g);
  }
  return g;
}

function buildBadge(n, geo, w, h, parent) {
  const pos = badgePosition(geo, w, h);
  const g = el('g', { class: 'sb-badge', transform: `translate(${pos.x} ${pos.y})` }, parent);
  el('circle', { r: 9.5, fill: geo.kind === 'point' ? POINT_COLOR : AREA_COLOR }, g);
  const text = el('text', { 'text-anchor': 'middle', 'dominant-baseline': 'central', y: 0.5 }, g);
  text.textContent = String(n);
  return { g, pos };
}

const CURSOR_PATH = 'M0 0 L0 18 L4.6 13.4 L7.8 20.4 L10.8 19.1 L7.7 12.2 L13.6 12.2 Z';

export function createStoryboardStage(svg) {
  const reducedMotion = () => window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  const dur = (ms) => (reducedMotion() ? 0 : ms);

  let gen = 0;
  let parts = null; // the scaffold, built on first use
  let state = { index: -1, geo: null, hole: null, cursor: null, current: null, steps: [] };

  function scaffold() {
    if (parts) return parts;
    svg.innerHTML = '';
    const defs = el('defs', {}, svg);
    const feather = el('filter', { id: 'sb-feather', filterUnits: 'userSpaceOnUse', x: 0, y: 0, width: '100%', height: '100%' }, defs);
    el('feGaussianBlur', { stdDeviation: 7 }, feather);
    const mask = el('mask', { id: 'sb-spot-mask', maskUnits: 'userSpaceOnUse', x: 0, y: 0, width: '100%', height: '100%' }, defs);
    el('rect', { x: 0, y: 0, width: '100%', height: '100%', fill: 'white' }, mask);
    const hole = el('rect', { rx: 14, fill: 'black', filter: 'url(#sb-feather)' }, mask);
    const dim = el('rect', { class: 'sb-dim', x: 0, y: 0, width: '100%', height: '100%', mask: 'url(#sb-spot-mask)' }, svg);
    const trail = el('g', { id: 'sb-trail' }, svg);
    const current = el('g', { id: 'sb-current' }, svg);
    const cursor = el('g', { class: 'sb-cursor' }, svg);
    el('circle', { class: 'sb-cursor-halo', r: 9 }, cursor);
    el('path', { d: CURSOR_PATH, class: 'sb-cursor-arrow' }, cursor);
    parts = { hole, dim, trail, current, cursor };
    return parts;
  }

  /// Runs onUpdate(eased t) every frame for `ms`. Resolves early, without
  /// further updates, once a newer generation has started.
  function tween(myGen, ms, onUpdate, ease = easeInOutCubic) {
    return new Promise((resolve) => {
      if (ms <= 0) {
        if (myGen === gen) onUpdate(1);
        resolve();
        return;
      }
      const start = performance.now();
      const frame = (now) => {
        if (myGen !== gen) return resolve();
        // A frame's timestamp can predate `start`, so clamp at 0 too.
        const t = clamp((now - start) / ms, 0, 1);
        onUpdate(ease(t));
        if (t < 1) requestAnimationFrame(frame);
        else resolve();
      };
      requestAnimationFrame(frame);
    });
  }

  function placeCursor(p) {
    parts.cursor.setAttribute('transform', `translate(${p.x} ${p.y})`);
    state.cursor = { x: p.x, y: p.y };
  }

  function ripple(p) {
    if (reducedMotion()) return;
    const ring = el('circle', { class: 'sb-ripple', cx: p.x, cy: p.y, r: 4 }, svg);
    const myGen = gen;
    tween(myGen, 460, (t) => {
      ring.setAttribute('r', 4 + 22 * t);
      ring.setAttribute('opacity', 0.75 * (1 - t));
    }, easeOutCubic).then(() => ring.remove());
  }

  // Earlier steps stay on screen, faded and numbered, so the walkthrough
  // builds up a map of what it covered. `fresh` is the one that just
  // became part of the trail: it fades down from full strength instead of
  // appearing already faded.
  function renderTrail(steps, upTo, fresh, w, h) {
    parts.trail.innerHTML = '';
    for (let i = 0; i < upTo; i++) {
      const geo = shapeGeometry(steps[i], w, h);
      if (!geo) continue;
      const item = el('g', { class: 'sb-trail-item' }, parts.trail);
      buildShape(geo, item);
      buildBadge(i + 1, geo, w, h, item);
      if (i === fresh && !reducedMotion()) {
        item.style.opacity = '1';
        item.getBoundingClientRect();
        requestAnimationFrame(() => { item.style.opacity = ''; });
      }
    }
  }

  function setSpotlight(myGen, geo, ms) {
    const target = focusRect(geo);
    if (!target) {
      parts.dim.classList.remove('on');
      state.hole = null;
      return Promise.resolve();
    }
    // First target: the hole closes in from a much larger area, like an iris.
    const from = state.hole || { x: target.x - 600, y: target.y - 600, w: target.w + 1200, h: target.h + 1200 };
    parts.dim.classList.add('on');
    state.hole = target;
    return tween(myGen, Math.max(ms, dur(420)), (t) => setRect(parts.hole, lerpRect(from, target, t)));
  }

  async function travel(myGen, to, ms) {
    const from = state.cursor || to;
    const c = travelControl(from, to);
    parts.cursor.classList.add('on');
    parts.cursor.classList.remove('resting');
    await tween(myGen, ms, (t) => placeCursor(bezierPoint(from, c, to, t)));
  }

  async function rest(myGen, w, h) {
    const from = state.cursor;
    const to = restPosition(from, w, h);
    parts.cursor.classList.add('resting');
    await tween(myGen, dur(280), (t) => placeCursor({ x: lerp(from.x, to.x, t), y: lerp(from.y, to.y, t) }), easeOutCubic);
  }

  async function drawIn(myGen, geo, g) {
    if (geo.kind === 'point') {
      g.setAttribute('transform', `translate(${geo.x} ${geo.y}) scale(0)`);
      await tween(myGen, dur(380), (t) => g.setAttribute('transform', `translate(${geo.x} ${geo.y}) scale(${t})`), easeOutBack);
    } else if (geo.kind === 'box') {
      const rect = g.firstElementChild;
      const perimeter = 2 * (geo.w + geo.h);
      rect.setAttribute('stroke-dasharray', perimeter);
      await tween(myGen, dur(560), (t) => {
        rect.setAttribute('stroke-dashoffset', perimeter * (1 - t));
        rect.setAttribute('fill-opacity', 0.12 * t);
      });
      rect.removeAttribute('stroke-dasharray');
      rect.removeAttribute('stroke-dashoffset');
    } else {
      // The cursor rides the tip of the arrow as it is drawn.
      const [path, head] = g.children;
      head.setAttribute('opacity', 0);
      await tween(myGen, dur(520), (t) => {
        const x = lerp(geo.x1, geo.x2, t), y = lerp(geo.y1, geo.y2, t);
        path.setAttribute('d', `M ${geo.x1} ${geo.y1} L ${x} ${y}`);
        placeCursor({ x, y });
      });
      if (myGen === gen) head.setAttribute('opacity', 1);
    }
  }

  async function morphBox(myGen, rect, from, to, ms) {
    await tween(myGen, ms, (t) => setRect(rect, lerpRect(from, to, t)));
  }

  async function popBadge(myGen, g, pos) {
    await tween(myGen, dur(300), (t) => g.setAttribute('transform', `translate(${pos.x} ${pos.y}) scale(${t})`), easeOutBack);
  }

  return {
    /// Shows step `index` of `steps`: earlier steps become the trail, the
    /// spotlight and cursor move to this step's target and its shape is drawn.
    /// Showing the same step again (resuming after a pause) redraws it in place.
    show(steps, index) {
      const myGen = ++gen;
      scaffold();
      const w = window.innerWidth, h = window.innerHeight;
      const prev = state;
      const geo = shapeGeometry(steps[index], w, h);
      const sameStep = index === prev.index;

      renderTrail(steps, index, index === prev.index + 1 ? index - 1 : -1, w, h);
      state = { ...prev, index, geo, steps };

      const canMorph = !sameStep && prev.geo && prev.geo.kind === 'box' && geo && geo.kind === 'box'
        && prev.current && prev.current.isConnected;
      if (!canMorph) {
        parts.current.innerHTML = '';
        state.current = null;
      }

      if (!geo) {
        parts.cursor.classList.remove('on');
        setSpotlight(myGen, null, 0);
        return;
      }

      // The cursor's first trip starts just above the caption bar.
      if (!state.cursor) placeCursor({ x: w / 2, y: h - 170 });
      const trip = dur(travelDuration(state.cursor, geo.anchor));
      setSpotlight(myGen, geo, trip);

      (async () => {
        let g;
        if (canMorph) {
          g = prev.current;
          // The old step's number goes with it (the trail has its own copy),
          // and a box cut off mid draw-on is completed before it moves.
          for (const b of parts.current.querySelectorAll('.sb-badge')) b.remove();
          const rect = g.firstElementChild;
          rect.removeAttribute('stroke-dasharray');
          rect.removeAttribute('stroke-dashoffset');
          rect.setAttribute('fill-opacity', 0.12);
          await Promise.all([
            travel(myGen, geo.anchor, trip),
            morphBox(myGen, g.firstElementChild, prev.geo, geo, Math.max(trip, dur(380))),
          ]);
        } else {
          await travel(myGen, geo.anchor, trip);
          if (myGen !== gen) return;
          ripple(geo.anchor);
          g = buildShape(geo, parts.current);
          state.current = g;
          await drawIn(myGen, geo, g);
        }
        if (myGen !== gen) return;
        state.current = g;
        const badge = buildBadge(index + 1, geo, w, h, parts.current);
        await Promise.all([popBadge(myGen, badge.g, badge.pos), rest(myGen, w, h)]);
      })();
    },

    /// The walkthrough ended on its own: the spotlight and cursor go, and
    /// every step's shape stays up, numbered, as a summary of what was covered.
    finish() {
      if (!parts) return;
      gen++;
      const w = window.innerWidth, h = window.innerHeight;
      parts.dim.classList.remove('on');
      parts.cursor.classList.remove('on');
      parts.current.innerHTML = '';
      renderTrail(state.steps, state.steps.length, -1, w, h);
      parts.trail.classList.add('final');
      state = { index: -1, geo: null, hole: null, cursor: null, current: null, steps: [] };
    },

    /// A question interrupted the walkthrough: lift the dim so the answer is
    /// readable, and freeze any animation where it is.
    pause() {
      if (!parts) return;
      gen++;
      parts.dim.classList.remove('on');
      parts.cursor.classList.remove('on');
      // The question is probably about what this step points at, so its shape
      // stays, completed in place if it was still being drawn. Resuming
      // replays the step and draws it again.
      parts.current.innerHTML = '';
      let current = null;
      if (state.geo) {
        const w = window.innerWidth, h = window.innerHeight;
        current = buildShape(state.geo, parts.current);
        buildBadge(state.index + 1, state.geo, w, h, parts.current);
      }
      state = { ...state, index: -1, hole: null, current };
    },

    clear() {
      gen++;
      svg.innerHTML = '';
      parts = null;
      state = { index: -1, geo: null, hole: null, cursor: null, current: null, steps: [] };
    },
  };
}
