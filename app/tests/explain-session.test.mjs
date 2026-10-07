// Run: node app/tests/explain-session.test.mjs
import { createExplainSession } from '../src/explain-session.js';

let fail = 0;
const eq = (name, got, want) => {
  const ok = JSON.stringify(got) === JSON.stringify(want);
  if (!ok) { fail++; console.log('FAIL', name, '\n  got ', JSON.stringify(got), '\n  want', JSON.stringify(want)); }
};
const tick = () => new Promise((r) => setTimeout(r, 0));

// A controllable stand-in for narration: end() finishes it naturally,
// cancel() is what pause/stop call.
function fakeSpeech(log, label) {
  let resolve;
  const done = new Promise((r) => { resolve = r; });
  return {
    label,
    done,
    end: () => resolve('ended'),
    cancel: () => { log.push(`cancel:${label}`); resolve('cancelled'); },
  };
}

function harness(session) {
  const log = [];
  const speeches = [];
  const hooks = {
    onStep: (step, i) => log.push(`show:${step.narration}#${i}`),
    speak: (step) => {
      const s = fakeSpeech(log, step.narration);
      speeches.push(s);
      return s;
    },
    onWaiting: () => log.push('waiting'),
  };
  const result = session.run(hooks);
  return { log, speeches, result };
}

// 1. steps that arrive while playing, in order, then finishing
{
  const s = createExplainSession();
  s.push({ narration: 'a' });
  const h = harness(s);
  await tick();
  eq('plays first step right away', h.log, ['show:a#0']);
  h.speeches[0].end();
  await tick();
  eq('waits for the model when caught up', h.log, ['show:a#0', 'waiting']);
  s.push({ narration: 'b' });
  await tick();
  eq('starts the step the moment it arrives', h.log.at(-1), 'show:b#1');
  h.speeches[1].end();
  s.finish();
  await tick();
  eq('ends as finished', await h.result, 'finished');
  eq('index advanced past every step', s.index, 2);
}

// 2. steps already waiting when the stream ends are all still played
{
  const s = createExplainSession();
  s.push({ narration: 'a' });
  s.push({ narration: 'b' });
  s.finish();
  const h = harness(s);
  await tick();
  h.speeches[0].end();
  await tick();
  h.speeches[1].end();
  eq('plays buffered steps then finishes', await h.result, 'finished');
  eq('no waiting when the stream is already done', h.log.includes('waiting'), false);
}

// 3. a stream that breaks after some steps still plays them out
{
  const s = createExplainSession();
  s.push({ narration: 'a' });
  s.finish(new Error('network'));
  const h = harness(s);
  await tick();
  h.speeches[0].end();
  eq('finishes after a broken stream', await h.result, 'finished');
  eq('keeps the error for the caller', String(s.streamError), 'Error: network');
}

// 4. pause mid-step cancels speech, keeps the index, resume replays the step
{
  const s = createExplainSession();
  s.push({ narration: 'a' });
  s.push({ narration: 'b' });
  s.finish();
  const h = harness(s);
  await tick();
  s.pause();
  await tick();
  eq('pausing cancels the narration', h.log, ['show:a#0', 'cancel:a']);
  eq('index stays on the interrupted step', s.index, 0);
  await tick();
  eq('nothing plays while paused', h.speeches.length, 1);
  s.resume();
  await tick();
  eq('resume replays the interrupted step from the start', h.log.slice(2), ['show:a#0']);
  h.speeches[1].end();
  await tick();
  h.speeches[2].end();
  eq('then carries on to the end', await h.result, 'finished');
}

// 5. pause while waiting for the model: nothing is shown until resume
{
  const s = createExplainSession();
  s.push({ narration: 'a' });
  const h = harness(s);
  await tick();
  h.speeches[0].end();
  await tick();
  s.pause();
  s.push({ narration: 'b' });
  await tick();
  eq('a step arriving during a pause is held', h.log.filter((l) => l.startsWith('show:')), ['show:a#0']);
  s.resume();
  await tick();
  eq('shown after resume', h.log.at(-1), 'show:b#1');
  s.stop();
  eq('stop ends it', await h.result, 'stopped');
}

// 6. speech ending naturally in the same moment as a pause does not skip ahead
{
  const s = createExplainSession();
  s.push({ narration: 'a' });
  s.push({ narration: 'b' });
  s.finish();
  const h = harness(s);
  await tick();
  s.paused = true; // pause lands first, speech then reports 'ended'
  h.speeches[0].end();
  await tick();
  eq('index not advanced while paused', s.index, 0);
  s.paused = false;
  s.stop();
  await h.result;
}

// 7. stop mid-speech
{
  const s = createExplainSession();
  s.push({ narration: 'a' });
  const h = harness(s);
  await tick();
  s.stop();
  eq('stop cancels speech and ends', [await h.result, h.log.at(-1)], ['stopped', 'cancel:a']);
}

// 8. firstStep: resolves on first step, on a failed stream, and on stop
{
  const s = createExplainSession();
  let got = false;
  s.firstStep().then(() => { got = true; });
  await tick();
  eq('still waiting with nothing yet', got, false);
  s.push({ narration: 'a' });
  await tick();
  eq('resolves on the first step', got, true);

  const failed = createExplainSession();
  let failedDone = false;
  failed.firstStep().then(() => { failedDone = true; });
  failed.finish(new Error('boom'));
  await tick();
  eq('resolves when the stream fails before any step', [failedDone, failed.steps.length], [true, 0]);

  const stopped = createExplainSession();
  let stoppedDone = false;
  stopped.firstStep().then(() => { stoppedDone = true; });
  stopped.stop();
  await tick();
  eq('resolves when stopped while waiting', stoppedDone, true);
}

// 9. resume/pause are idempotent
{
  const s = createExplainSession();
  s.resume();
  eq('resume when not paused is a no-op', s.paused, false);
  s.pause();
  s.pause();
  eq('double pause stays paused', s.paused, true);
  s.stop();
  s.pause();
  eq('pause after stop is ignored', [s.stopped, s.paused], [true, true]);
}

console.log(fail ? `${fail} failure(s)` : 'all explain-session checks passed');
process.exit(fail ? 1 : 0);
