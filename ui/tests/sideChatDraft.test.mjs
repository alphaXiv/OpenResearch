import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import ts from "typescript";

const require = createRequire(import.meta.url);

function loadFunction(file, name, globals) {
  const text = readFileSync(new URL(`../src/components/${file}`, import.meta.url), "utf8");
  const source = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let expression;
  const visit = (node) => {
    if (ts.isFunctionDeclaration(node) && node.name?.text === name) {
      expression = node.getText(source).replace(/^export /, "");
    } else if (ts.isVariableDeclaration(node) && node.name.getText(source) === name) {
      expression = node.initializer.getText(source);
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  assert.ok(expression, name);
  const code = ts.transpileModule(`exports.target = ${expression}`, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
  }).outputText;
  const exports = {};
  new Function("require", "exports", ...Object.keys(globals), code)(require, exports, ...Object.values(globals));
  return exports.target;
}

test("side-chat trigger opens the list on touch and a draft on mouse click", () => {
  const opened = [];
  let created = 0;
  const OptionPicker = loadFunction("ModelPicker.tsx", "OptionPicker", {
    useRef: (value) => ({ current: value }),
    useState: (value) => [value, () => {}],
    useEffect: () => {},
    useLayoutEffect: () => {},
    usePopover: () => ({ open: false, setOpen: (value) => opened.push(typeof value === "function" ? value(false) : value), ref: { current: null } }),
    IconButton: "button",
    REASONING_DEFAULT_ID: "default",
  });
  const tree = OptionPicker({ triggerIcon: "chat", onTriggerClick: () => created++, openOnHover: true, choices: [{ id: "saved", label: "Saved chat" }] });
  const trigger = tree.props.children[0];
  trigger.props.onPointerDown({ pointerType: "touch" });
  trigger.props.onClick();
  assert.equal(created, 0);
  assert.equal(opened.at(-1), true);
  trigger.props.onPointerDown({ pointerType: "mouse" });
  trigger.props.onClick();
  assert.equal(created, 1);
  assert.equal(opened.at(-1), false);
});

test("unsaved side-chat autonomy stays local while root drafts update defaults", () => {
  for (const sideParentSessionId of ["parent", undefined]) {
    const local = [];
    const defaults = [];
    const setAutonomy = loadFunction("ChatPanel.tsx", "setAutonomy", {
      openSession: null, sideParentSessionId,
      setSideDraftAutonomy: (value) => local.push(value),
      onPreferredAutonomyChange: (value) => defaults.push(value),
    });
    setAutonomy("copilot");
    assert.deepEqual(local, sideParentSessionId ? ["copilot"] : []);
    assert.deepEqual(defaults, sideParentSessionId ? [] : ["copilot"]);
  }
});

test("first send applies draft autonomy to the newly created side chat", async () => {
  const changes = [];
  let cached;
  const openNewSession = loadFunction("ChatPanel.tsx", "openNewSession", {
    composerScopeRef: { current: {} }, projectVisitRef: { current: {} },
    sideParentSessionId: "parent", sideDraftAutonomy: "copilot",
    openSideChatMutation: { mutateAsync: async (parent) => ({ id: "child", autonomy: "agentic", sideParentSessionId: parent }) },
    setChatSessionAutonomyMutation: { mutateAsync: async ([id, autonomy]) => { changes.push([id, autonomy]); return { id, autonomy }; } },
    isCurrentScope: () => true, sessionsOptions: { queryKey: [] },
    setSessions: (update) => { cached = update([]); },
    queryClient: { getQueryData: () => [] },
    onActiveSessionChangeRef: { current: () => {} },
  });
  const session = await openNewSession({}, undefined);
  assert.deepEqual(changes, [["child", "copilot"]]);
  assert.equal(session.autonomy, "copilot");
  assert.equal(cached[0].autonomy, "copilot");
});
