// Run: node app/tests/voice-command.test.mjs
// Transcripts in the first two blocks are real Parakeet output from
// synthesized speech (US, UK and Indian English voices, two speeds).
import { parseVoiceCommand as p, voiceToQuery as q } from '../src/voice-command.js';
let fail = 0;
const check = (input, mode, rest) => {
  const r = p(input);
  const ok = r.mode === mode && (rest === undefined || r.rest === rest);
  if (!ok) { fail++; console.log('FAIL', JSON.stringify(input), '->', JSON.stringify(r), 'want', mode, rest ?? ''); }
};
// Real Parakeet transcripts from the probe (US, UK, Indian voices)
for (const t of ['Agent, open notepad, and type hello.','Uh, agent, open notepad','I\'m agent open Chrome and search for weather.','Hey Agent, save this file.','Okay so agent, close this window.','Hey Pointer, Agent, Open Settings.','Agent opened a calculator.','Armagent open Chrome and search for weather.','Uh Agent. Open notepad.','Uh Agent, Open Notepad','Um agent, open Chrome and search for weather.','Agent, open the calculator.'])
  check(t, 'agent');
for (const t of ['Explain how this chart works.','Uh, explain this diagram.','Um, can you explain what this code does?','Okay explain the error on screen.','Hey, explain this.','So, explain the graph','Ah, explain this diagram.','Um? Can you explain what this code does?','Uh explain this diagram.','Okay, explain the error on screen.'])
  check(t, 'explain');
// Exact rest text
check('Uh, agent, open notepad', 'agent', 'open notepad');
check('Hey Pointer, Agent, Open Settings.', 'agent', 'Open Settings.');
check('Um, can you explain what this code does?', 'explain', 'what this code does?');
check('A gent, open notepad', 'agent', 'open notepad');
check('Agents open notepad', 'agent', 'open notepad');
// Reported from real use: "agent" transcribed as "A J", in its spellings
check('A J open notepad', 'agent', 'open notepad');
check('A. J., open notepad', 'agent', 'open notepad');
check('AJ, open notepad', 'agent', 'open notepad');
check('A.J. open notepad', 'agent', 'open notepad');
check('Uh, A J, open notepad', 'agent', 'open notepad');
check('Explained this graph', 'explain', 'this graph');
check('Ex plain this graph', 'explain', 'this graph');
// Must stay plain questions
for (const t of ['What does the agent config in this file do?','So what is this error?','Can you tell me what this is?','How do I explain this to my manager?','Agent','Agent?','Um, what is this?','The agent is failing, why?','Uh','','Okay so, uh, hmm, well, yeah, now, agent open notepad'])
  check(t, null);
check('So what is this error?', null, 'So what is this error?');
// Query strings
console.log(q('Uh, agent, open notepad.'), '|', q('Um, can you explain this chart?'), '|', q('What is this?'));
console.log(fail ? fail + ' FAILED' : 'all passed');
process.exit(fail ? 1 : 0);
