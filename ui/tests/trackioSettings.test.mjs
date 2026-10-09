import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

// Evaluate the real EnvVarsSection with deterministic hooks: a render returns
// its element tree, so a test can read the props it hands each EnvRow.
const file = ts.createSourceFile(
  "SettingsPage.tsx",
  readFileSync(new URL("../src/components/SettingsPage.tsx", import.meta.url), "utf8"),
  ts.ScriptTarget.Latest,
  true,
  ts.ScriptKind.TSX,
);
const constants = ["TRACKIO_SERVER_URL_KEY", "TRACKIO_PROJECT_KEY", "TENSORBOARD_LOGDIR_KEY", "RECOMMENDED_ENV_KEYS"];
const pieces = file.statements.filter(
  (node) =>
    (ts.isVariableStatement(node) && node.declarationList.declarations.some((d) => constants.includes(d.name.getText(file)))) ||
    (ts.isFunctionDeclaration(node) && node.name?.text === "EnvVarsSection"),
);
assert.equal(pieces.length, constants.length + 1);
const code = ts.transpileModule(`${pieces.map((node) => node.getText(file)).join("\n")}\nreturn EnvVarsSection;`, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.React, jsxFactory: "h", jsxFragmentFactory: "Fragment" },
}).outputText;

function EnvRow() {}

function deferred() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}

function section({ vars }) {
  const slots = [];
  let cursor = 0;
  let changed = true;
  let effects = [];
  let current = vars;
  const verdicts = [];
  const preflights = [];
  const same = (a, b) => a && a.length === b.length && a.every((v, i) => Object.is(v, b[i]));
  const bindings = {
    h: (type, props, ...children) => ({ type, props: { ...props, children } }),
    Fragment: "Fragment",
    useState(initial) {
      const index = cursor++;
      if (!(index in slots)) slots[index] = initial;
      return [slots[index], (update) => {
        const next = typeof update === "function" ? update(slots[index]) : update;
        if (!Object.is(next, slots[index])) { slots[index] = next; changed = true; }
      }];
    },
    useRef(initial) {
      const index = cursor++;
      slots[index] ??= { current: initial };
      return slots[index];
    },
    useEffect(effect, deps) {
      const index = cursor++;
      if (same(slots[index]?.deps, deps)) return;
      const previous = slots[index];
      slots[index] = { deps };
      effects.push(() => {
        previous?.cleanup?.();
        slots[index].cleanup = effect();
      });
    },
    useQuery: () => ({ data: current, error: null }),
    getEnvVarsQuery: () => ({ queryKey: ["settings", "env"] }),
    setScopedQueryData: (_key, update) => { current = update(current) ?? null; changed = true; },
    getTrackioSettings: () => { const call = deferred(); verdicts.push(call); return call.promise; },
    trackioPreflight: () => { const call = deferred(); preflights.push(call); return call.promise; },
    m: new Proxy({}, { get: (_, name) => () => String(name) }),
    ltr: (value) => value,
    EnvRow,
    AddVarRow: () => null,
    Button: () => null,
    LoadingRow: () => null,
    Plus: () => null,
    Spinner: () => null,
    SETTINGS_CARD_CLASS_NAME: "card",
    SETTINGS_NOTE_CLASS_NAME: "note",
  };
  const EnvVarsSection = new Function(...Object.keys(bindings), code)(...Object.values(bindings));
  let tree;
  const render = () => {
    for (let pass = 0; pass < 10 && changed; pass++) {
      changed = false;
      cursor = 0;
      tree = EnvVarsSection();
      const pending = effects;
      effects = [];
      pending.forEach((effect) => effect());
    }
  };
  function* walk(node) {
    if (Array.isArray(node)) for (const child of node) yield* walk(child);
    else if (node && typeof node === "object") {
      yield node;
      yield* walk(node.props.children);
    } else if (typeof node === "string") yield node;
  }
  const settle = async () => {
    render();
    for (let turn = 0; turn < 5; turn++) {
      await Promise.resolve();
      render();
    }
  };
  return {
    verdicts,
    preflights,
    settle,
    row: (name) => [...walk(tree)].find((node) => node?.type === EnvRow && node.props.name === name).props,
    shows: (text) => [...walk(tree)].includes(text),
  };
}

const url = (value) => ({ key: "TRACKIO_SERVER_URL", value, maskedValue: value, secret: false, inProcessEnv: false });
const token = { key: "TRACKIO_WRITE_TOKEN", value: null, maskedValue: "tok…en-a", secret: true, inProcessEnv: false };
const project = { key: "TRACKIO_PROJECT", value: "demo", maskedValue: "demo", secret: false, inProcessEnv: false };
const hfToken = { key: "HF_TOKEN", value: null, maskedValue: "hf_…abcd", secret: true, inProcessEnv: false };
const verdict = (reason) => ({ configured: true, reachable: true, usable: false, reason, serverUrl: "http://a:7860", project: null, hasToken: true, dashboardUrl: null });
const allowed = { reachable: true, version: "0.35.0", writeAccess: true, error: null };
const LOAD_REASON = "Set TRACKIO_PROJECT before launching a tracked run.";
const NEW_REASON = "The Trackio write token was rejected; runs will not be able to log.";

test("saving or deleting a Trackio setting drops the old Test result and shows the new verdict", async () => {
  const page = section({ vars: [url("http://a:7860"), token] });
  await page.settle();
  page.verdicts[0].resolve(verdict(LOAD_REASON));
  await page.settle();
  assert.equal(page.shows(LOAD_REASON), true);

  page.row("TRACKIO_SERVER_URL").onTest();
  page.preflights[0].resolve(allowed);
  await page.settle();
  assert.deepEqual(page.row("TRACKIO_SERVER_URL").probe, allowed);
  assert.equal(page.shows(LOAD_REASON), false);

  // Control: an unrelated variable is not a connection change.
  page.row("HF_TOKEN").onVars([url("http://a:7860"), token, hfToken]);
  await page.settle();
  assert.deepEqual(page.row("TRACKIO_SERVER_URL").probe, allowed);
  assert.equal(page.verdicts.length, 1);

  page.row("TRACKIO_PROJECT").onVars([url("http://a:7860"), token, hfToken, project]);
  await page.settle();
  // Until the new verdict arrives, neither the old result nor the old reason shows.
  assert.equal(page.row("TRACKIO_SERVER_URL").probe, null);
  assert.equal(page.shows(LOAD_REASON), false);
  page.verdicts[1].resolve(verdict(NEW_REASON));
  await page.settle();
  assert.equal(page.shows(NEW_REASON), true);

  page.row("TRACKIO_SERVER_URL").onTest();
  page.preflights[1].resolve(allowed);
  await page.settle();
  page.row("TRACKIO_WRITE_TOKEN").onVars([url("http://a:7860"), hfToken, project]);
  await page.settle();
  assert.equal(page.row("TRACKIO_SERVER_URL").probe, null);
  assert.equal(page.verdicts.length, 3);
});

test("a Test still running when the connection changes never shows its result", async () => {
  const page = section({ vars: [url("http://a:7860"), token] });
  await page.settle();
  page.verdicts[0].resolve(verdict(LOAD_REASON));
  await page.settle();

  page.row("TRACKIO_SERVER_URL").onTest();
  await page.settle();
  assert.equal(page.row("TRACKIO_SERVER_URL").probe, "checking");
  page.row("TRACKIO_SERVER_URL").onVars([url("http://b:7860"), token]);
  await page.settle();
  page.preflights[0].resolve(allowed);
  page.verdicts[1].resolve(verdict(NEW_REASON));
  await page.settle();
  assert.equal(page.row("TRACKIO_SERVER_URL").probe, null);
  assert.equal(page.shows(NEW_REASON), true);
});
