// Run: node app/tests/whiteboard.test.mjs
// Geometry only: the drawing half of whiteboard.js needs a webview.
import {
  seededRandom, hashSeed, boxPath, circlePath, fitFont, shapeBounds, edgePoint,
  resolveArrow, arrowGeometry, focusStep, usesBoard,
} from '../src/whiteboard.js';

let fail = 0;
const eq = (name, got, want) => {
  const ok = JSON.stringify(got) === JSON.stringify(want);
  if (!ok) { fail++; console.log('FAIL', name, '\n  got ', JSON.stringify(got), '\n  want', JSON.stringify(want)); }
};
const near = (a, b, tol = 1e-6) => Math.abs(a - b) <= tol;

// Wobble is deterministic, so a redrawn shape doesn't jump.
{
  const a = boxPath({ x: 10, y: 10, w: 100, h: 50 }, seededRandom(hashSeed('k')));
  const b = boxPath({ x: 10, y: 10, w: 100, h: 50 }, seededRandom(hashSeed('k')));
  const c = boxPath({ x: 10, y: 10, w: 100, h: 50 }, seededRandom(hashSeed('other')));
  eq('same seed, same box', a, b);
  eq('different seed, different wobble', a === c, false);
  eq('box is one stroke of four edges', [a.startsWith('M'), (a.match(/Q/g) || []).length], [true, 4]);
}
{
  const r = seededRandom(1);
  const vals = Array.from({ length: 200 }, r);
  eq('random stays in [0, 1)', vals.every((v) => v >= 0 && v < 1), true);
}

// Circle: starts near the radius, ellipse when ry is given.
{
  const d = circlePath(100, 100, 50, seededRandom(3));
  const pts = [...d.matchAll(/(-?\d+\.?\d*) (-?\d+\.?\d*)/g)].map((m) => [Number(m[1]), Number(m[2])]);
  const radii = pts.map(([x, y]) => Math.hypot(x - 100, y - 100));
  eq('circle points stay close to the radius', radii.every((r) => r > 44 && r < 56), true);
  eq('circle overshoots its start (hand-drawn overlap)', pts.length, 41);
  const e = circlePath(0, 0, 100, seededRandom(3), 20);
  const ys = [...e.matchAll(/(-?\d+\.?\d*) (-?\d+\.?\d*)/g)].map((m) => Math.abs(Number(m[2])));
  eq('ellipse is squashed vertically', Math.max(...ys) < 25, true);
}

// Label sizing
eq('short label keeps the preferred size', fitFont('DB', 200), 27);
eq('long label shrinks to fit', fitFont('a very long label for a small box', 120) < 27, true);
eq('never below readable', fitFont('x'.repeat(200), 50), 14);

// Bounds and edges
eq('box bounds', shapeBounds({ type: 'box', x: 1, y: 2, w: 3, h: 4 }), { x: 1, y: 2, w: 3, h: 4 });
eq('circle bounds', shapeBounds({ type: 'circle', x: 50, y: 50, r: 10 }), { x: 40, y: 40, w: 20, h: 20 });
{
  const box = { type: 'box', x: 0, y: 0, w: 100, h: 50 };
  const p = edgePoint(box, { x: 500, y: 25 }, 0);
  eq('edge point on the right side of a box', [p.x, p.y], [100, 25]);
  const q = edgePoint(box, { x: 50, y: -500 }, 0);
  eq('edge point on the top', [near(q.x, 50), near(q.y, 0)], [true, true]);
  const c = edgePoint({ type: 'circle', x: 0, y: 0, r: 10 }, { x: 0, y: 100 }, 5);
  eq('edge of a circle, with the gap', [near(c.x, 0), near(c.y, 15)], [true, true]);
  const inside = edgePoint(box, { x: 52, y: 26 }, 8);
  eq('target inside the shape: stop at the target', [near(inside.x, 52), near(inside.y, 26)], [true, true]);
}

// Arrows between shapes
{
  const byId = new Map([
    ['a', { type: 'box', x: 0, y: 0, w: 100, h: 100 }],
    ['b', { type: 'box', x: 400, y: 0, w: 100, h: 100 }],
  ]);
  const ends = resolveArrow({ from: 'a', to: 'b' }, byId);
  eq('arrow runs edge to edge, short of each outline', [ends.start.x, ends.end.x], [108, 392]);
  eq('unknown id: no arrow', resolveArrow({ from: 'a', to: 'zzz' }, byId), null);
  const spot = resolveArrow({ from: { x: 50, y: 300 }, to: 'a' }, byId);
  eq('spot to shape: starts at the spot', spot.start, { x: 50, y: 300 });
  eq('arrow between overlapping ends: none', resolveArrow({ from: { x: 5, y: 5 }, to: { x: 6, y: 6 } }, byId), null);

  const geo = arrowGeometry({ x: 0, y: 0 }, { x: 300, y: 0 }, seededRandom(9));
  eq('shaft is a curve from start to end', [geo.shaft.startsWith('M 0.0 0.0 Q'), geo.shaft.endsWith('300.0 0.0')], [true, true]);
  eq('arrowhead meets at the end', geo.heads.includes('L 300.0 0.0'), true);
  eq('label sits off the line', Math.abs(geo.label.y) > 10, true);
  const back = arrowGeometry({ x: 300, y: 0 }, { x: 0, y: 0 }, seededRandom(9));
  eq('a reply arrow bends and labels on the other side', Math.sign(geo.label.y) !== Math.sign(back.label.y), true);
}

// Which step leads
{
  const steps = [
    { narration: 'a', board: [{ type: 'box' }] },
    { narration: 'b' },
    { narration: 'c', board: [{ type: 'arrow' }] },
  ];
  eq('a drawing step leads', focusStep(steps, 0), 0);
  eq('a narration-only step keeps the last drawing in focus', focusStep(steps, 1), 0);
  eq('next drawing step takes over', focusStep(steps, 2), 2);
  eq('no board anywhere', focusStep([{ narration: 'x', shape: 'point' }], 0), -1);
  eq('usesBoard follows', [usesBoard(steps, 1), usesBoard([{ narration: 'x' }], 0)], [true, false]);
  eq('empty board list does not count', focusStep([{ narration: 'x', board: [] }], 0), -1);
}

console.log(fail ? `${fail} failure(s)` : 'all whiteboard checks passed');
process.exit(fail ? 1 : 0);
