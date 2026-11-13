// Playback state for an "explain" walkthrough: steps that arrive while it is
// already playing, and pause / resume / stop around the step being spoken.
// No DOM and no Tauri here, so the timing logic can be tested in Node
// (app/tests/explain-session.test.mjs); main.js supplies the real hooks.
//
// The loop only ever advances past a step whose narration finished while the
// walkthrough was neither paused nor stopped. Pausing mid-sentence cancels the
// speech and keeps the index, so resuming replays that step from its start.

export function createExplainSession({ topic = '', requestId = 0 } = {}) {
  const waiters = [];
  const wakeAll = () => {
    const pending = waiters.splice(0);
    for (const resolve of pending) resolve();
  };
  const changed = () => new Promise((resolve) => waiters.push(resolve));

  const session = {
    topic,
    requestId,
    steps: [],
    index: 0, // the step being played, or about to be
    streamDone: false,
    streamError: null,
    paused: false,
    stopped: false,
    ended: false,
    _speech: null,

    push(step) {
      session.steps.push(step);
      wakeAll();
    },

    // The model finished writing steps (error is set if the stream broke).
    finish(error = null) {
      session.streamDone = true;
      if (error) session.streamError = error;
      wakeAll();
    },

    // Resolves once there is a first step, or the stream ended without one.
    async firstStep() {
      while (session.steps.length === 0 && !session.streamDone && !session.stopped) await changed();
    },

    pause() {
      if (session.paused || session.stopped) return;
      session.paused = true;
      if (session._speech) session._speech.cancel();
      wakeAll();
    },

    resume() {
      if (!session.paused) return;
      session.paused = false;
      wakeAll();
    },

    // Jump to another step (Left / Right arrows). The step being spoken is
    // cut off and the loop picks up at the new index. Returns false when
    // there is nowhere to go: before the first step, or past the last one
    // the model has written so far. Skipping past the last step once the
    // stream is done ends the walkthrough as finished.
    goTo(target) {
      if (session.stopped || session.paused) return false;
      if (target < 0) return false;
      if (target >= session.steps.length) {
        if (!session.streamDone || target !== session.steps.length) return false;
      }
      if (target === session.index) return false;
      session.index = target;
      if (session._speech) session._speech.cancel();
      wakeAll();
      return true;
    },

    next() {
      return session.goTo(session.index + 1);
    },

    back() {
      return session.goTo(session.index - 1);
    },

    stop() {
      if (session.stopped) return;
      session.stopped = true;
      if (session._speech) session._speech.cancel();
      wakeAll();
    },

    // hooks.onStep(step, index): show the step (caption, annotation).
    // hooks.speak(step): returns { done: Promise<'ended' | 'cancelled'>, cancel() }.
    // hooks.onWaiting(): the walkthrough is paused or waiting for the model.
    // Resolves with 'finished' or 'stopped' when the walkthrough is over.
    async run(hooks) {
      for (;;) {
        if (session.stopped) break;
        if (session.paused) {
          await changed();
          continue;
        }
        if (session.index >= session.steps.length) {
          if (session.streamDone) {
            session.ended = true;
            return 'finished';
          }
          if (hooks.onWaiting) hooks.onWaiting();
          await changed();
          continue;
        }

        const step = session.steps[session.index];
        hooks.onStep(step, session.index);
        const speech = hooks.speak(step);
        session._speech = speech;
        const outcome = await speech.done;
        session._speech = null;
        if (outcome === 'ended' && !session.paused && !session.stopped) session.index++;
      }
      session.ended = true;
      return 'stopped';
    },
  };
  return session;
}
