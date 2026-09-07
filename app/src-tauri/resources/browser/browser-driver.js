// Spawned by the Rust app (commands/browser.rs) as a persistent local
// subprocess for the duration of one browser-based agent task. Talks
// newline-delimited JSON over stdin/stdout: {"id","cmd","args"} in,
// {"id","ok","result"} or {"id","ok":false,"error"} out. One command
// in flight at a time — the Rust side sends and blocks for the matching
// reply, so no request multiplexing is needed here.
//
// Elements are referenced by role+accessible-name (ARIA), not raw CSS
// selectors or serialized element handles — `snapshot` walks the
// accessibility tree, assigns each interactive node a short numeric ref,
// and `click`/`type` re-resolve that ref via page.getByRole(role, {name})
// at call time. This is exactly the mechanism a real user's screen reader
// would use, and it's what lets the model pick a REAL target instead of
// guessing a pixel coordinate — the actual point of this feature.

const readline = require("readline");

let browser = null;
let page = null;
// ref (string) -> { role, name } — rebuilt fresh on every `snapshot` call,
// so a ref is only valid until the next snapshot.
let refMap = new Map();

const INTERACTIVE_ROLES = new Set([
  "button", "link", "textbox", "searchbox", "combobox", "checkbox",
  "radio", "menuitem", "tab", "switch", "slider",
]);

// Flattens the accessibility tree into a numbered list of interactive
// nodes only — a full tree (every div/span) would be too large and too
// noisy for the model to reason about.
function flatten(node, out) {
  if (!node) return;
  if (INTERACTIVE_ROLES.has(node.role) && node.name) {
    out.push({ role: node.role, name: node.name });
  }
  for (const child of node.children || []) flatten(child, out);
}

async function cmdOpen() {
  if (browser) return { alreadyOpen: true };
  browser = await require("playwright").chromium.launch({ headless: false });
  const context = await browser.newContext();
  page = await context.newPage();
  await page.goto("about:blank");
  return { opened: true };
}

async function cmdNavigate(args) {
  if (!page) throw new Error("No browser session open — call 'open' first.");
  await page.goto(args.url, { waitUntil: "domcontentloaded", timeout: 20000 });
  return { url: page.url() };
}

async function cmdSnapshot() {
  if (!page) throw new Error("No browser session open — call 'open' first.");
  const tree = await page.accessibility.snapshot({ interestingOnly: true });
  const flat = [];
  flatten(tree, flat);

  refMap = new Map();
  const elements = flat.map((el, i) => {
    const ref = String(i + 1);
    refMap.set(ref, el);
    return { ref, role: el.role, name: el.name };
  });

  return { url: page.url(), title: await page.title(), elements };
}

function locatorFor(ref) {
  const el = refMap.get(ref);
  if (!el) throw new Error(`Unknown ref "${ref}" — call 'snapshot' again, refs don't survive a navigation or page change.`);
  return page.getByRole(el.role, { name: el.name, exact: true }).first();
}

async function cmdClick(args) {
  await locatorFor(args.ref).click({ timeout: 10000 });
  return { clicked: args.ref };
}

async function cmdType(args) {
  await locatorFor(args.ref).fill(args.text, { timeout: 10000 });
  return { typed: args.ref };
}

async function cmdClose() {
  if (browser) {
    await browser.close();
    browser = null;
    page = null;
    refMap = new Map();
  }
  return { closed: true };
}

const HANDLERS = {
  open: cmdOpen,
  navigate: cmdNavigate,
  snapshot: cmdSnapshot,
  click: cmdClick,
  type: cmdType,
  close: cmdClose,
};

const rl = readline.createInterface({ input: process.stdin, terminal: false });

rl.on("line", async (line) => {
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    return; // ignore unparseable lines rather than crashing the whole process
  }

  const handler = HANDLERS[msg.cmd];
  if (!handler) {
    process.stdout.write(JSON.stringify({ id: msg.id, ok: false, error: `Unknown command: ${msg.cmd}` }) + "\n");
    return;
  }

  try {
    const result = await handler(msg.args || {});
    process.stdout.write(JSON.stringify({ id: msg.id, ok: true, result }) + "\n");
  } catch (e) {
    process.stdout.write(JSON.stringify({ id: msg.id, ok: false, error: String(e && e.message || e) }) + "\n");
  }
});

process.on("SIGTERM", async () => { await cmdClose(); process.exit(0); });
