// Run: node app/tests/storyboard-stage.test.mjs
// Geometry only: the DOM half of storyboard-stage.js needs a webview.
import {
  shapeGeometry, focusRect, badgePosition, travelControl, bezierPoint,
  travelDuration, lerpRect, easeInOutCubic, easeOutBack, rectsOverlap, restPosition,
} from '../src/storyboard-stage.js';

let fail = 0;
const eq = (name, got, want) => {
  const ok = JSON.stringify(got) === JSON.stringify(want);
  if (!ok) { fail++; console.log('FAIL', name, '\n  got ', JSON.stringify(got), '\n  want', JSON.stringify(want)); }
};
const near = (a, b) => Math.abs(a - b) < 1e-6;

const W = 1000, H = 500;

// shapeGeometry
eq('no shape', shapeGeometry({ narration: 'x' }, W, H), null);
eq('point scales to the screen',
  shapeGeometry({ shape: 'point', x_norm: 0.5, y_norm: 0.2 }, W, H),
  { kind: 'point', x: 500, y: 100, anchor: { x: 500, y: 100 } });
eq('box normalised whichever corners come first, with a little air around it',
  shapeGeometry({ shape: 'box', x_norm: 0.6, y_norm: 0.8, x2_norm: 0.2, y2_norm: 0.2 }, W, H),
  { kind: 'box', x: 194, y: 94, w: 412, h: 312, anchor: { x: 194, y: 94 } });
eq('box air stops at the screen edge',
  shapeGeometry({ shape: 'box', x_norm: 0, y_norm: 0, x2_norm: 1, y2_norm: 1 }, W, H),
  { kind: 'box', x: 0, y: 0, w: 1000, h: 500, anchor: { x: 0, y: 0 } });
eq('a box too thin to see becomes a point at its centre',
  shapeGeometry({ shape: 'box', x_norm: 0.2, y_norm: 0.2, x2_norm: 0.201, y2_norm: 0.6 }, W, H).kind, 'point');
eq('line keeps its direction',
  shapeGeometry({ shape: 'line', x_norm: 0.9, y_norm: 0.1, x2_norm: 0.1, y2_norm: 0.9 }, W, H),
  { kind: 'line', x1: 900, y1: 50, x2: 100, y2: 450, anchor: { x: 900, y: 50 } });
eq('a zero-length line becomes a point',
  shapeGeometry({ shape: 'line', x_norm: 0.5, y_norm: 0.5, x2_norm: 0.5, y2_norm: 0.5 }, W, H).kind, 'point');
eq('box without a second corner draws nothing',
  shapeGeometry({ shape: 'box', x_norm: 0.5, y_norm: 0.5 }, W, H), null);
eq('out-of-range coordinates are clamped to the screen',
  shapeGeometry({ shape: 'point', x_norm: 1.7, y_norm: -3 }, W, H).anchor, { x: 1000, y: 0 });
eq('unknown shape draws nothing', shapeGeometry({ shape: 'star', x_norm: 0.5, y_norm: 0.5 }, W, H), null);

// focusRect
eq('no target, no hole', focusRect(null), null);
eq('point gets a square hole around it', focusRect({ kind: 'point', x: 100, y: 100 }), { x: 48, y: 48, w: 104, h: 104 });
eq('box hole is padded', focusRect({ kind: 'box', x: 10, y: 20, w: 100, h: 50 }), { x: -4, y: 6, w: 128, h: 78 });
eq('line hole covers both ends', focusRect({ kind: 'line', x1: 200, y1: 100, x2: 100, y2: 300 }), { x: 70, y: 70, w: 160, h: 260 });

// badgePosition stays on screen
eq('badge next to a point', badgePosition({ kind: 'point', x: 100, y: 100 }, W, H), { x: 116, y: 84 });
eq('badge in the top-left corner is pulled on screen', badgePosition({ kind: 'box', x: 0, y: 0, w: 10, h: 10 }, W, H), { x: 12, y: 12 });

// cursor travel
{
  const from = { x: 0, y: 0 }, to = { x: 400, y: 0 };
  const c = travelControl(from, to);
  eq('arc bends off the straight line', [near(c.x, 200), c.y !== 0], [true, true]);
  const start = bezierPoint(from, c, to, 0), end = bezierPoint(from, c, to, 1);
  eq('curve starts and ends at the targets', [start, end], [from, to]);
  eq('no trip, no time', travelDuration(from, from), 0);
  eq('short trips have a floor', travelDuration(from, { x: 10, y: 0 }), 320);
  eq('long trips have a ceiling', travelDuration(from, { x: 5000, y: 0 }), 760);
  eq('same spot needs no arc', travelControl(from, from), from);
}

// caption placement and cursor resting spot
eq('overlapping rects', rectsOverlap({ x: 0, y: 0, w: 10, h: 10 }, { x: 5, y: 5, w: 10, h: 10 }), true);
eq('touching edges do not overlap', rectsOverlap({ x: 0, y: 0, w: 10, h: 10 }, { x: 10, y: 0, w: 10, h: 10 }), false);
eq('cursor rests just off the target', restPosition({ x: 100, y: 100 }, W, H), { x: 122, y: 126 });
eq('cursor rest stays on screen', restPosition({ x: 995, y: 495 }, W, H), { x: 984, y: 478 });

// tweening helpers
eq('lerpRect halfway', lerpRect({ x: 0, y: 0, w: 10, h: 10 }, { x: 10, y: 20, w: 30, h: 50 }, 0.5), { x: 5, y: 10, w: 20, h: 30 });
eq('easing endpoints', [easeInOutCubic(0), easeInOutCubic(1), near(easeOutBack(1), 1)], [0, 1, true]);
eq('easeOutBack overshoots on the way', easeOutBack(0.7) > 1, true);

console.log(fail ? `${fail} failure(s)` : 'all storyboard-stage checks passed');
process.exit(fail ? 1 : 0);
