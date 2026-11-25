import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { Message } from "../src/components/ChatPanel.tsx";
import { m } from "../src/paraglide/messages.js";
import { activityInProgress, toolActivity } from "../src/worklog/activity.ts";
import { render, settle } from "./render.mjs";

/** Text as the reader sees it, without the bidi isolates around interpolated values. */
const plain = (text) => text.replace(/[⁦-⁩]/g, "");

let ids = 0;
const call = (tool, input, extra = {}) => ({ id: `tool-${ids++}`, type: "tool", tool, state: { status: "completed", input, ...extra } });
const bash = (command, input = {}) => call("Bash", { command, ...input });
const codex = (script, cwd = "/ws") => call("bash", { command: `/bin/zsh -lc '${script.replace(/'/g, `'"'"'`)}'`, cwd });
const row = (part) => {
  const activity = toolActivity(part);
  return { label: plain(activity.label), progress: plain(activity.progressLabel ?? ""), icon: activity.icon, kind: activity.kind, files: (activity.files ?? []).map((file) => file.path), more: (activity.more ?? []).map(plain) };
};
const words = (message) => plain(message);

test("a sed -i edit in a compound call reads as the edit, with the rest of the call counted", () => {
  const edit = row(bash([
    "git checkout orx/600-steps",
    `sed -i '' 's/p.add_argument("--steps", type=int, default=200)/p.add_argument("--steps", type=int, default=600)/' train.py`,
    "git diff --stat",
    "git commit -am \"Train for 600 steps\"",
  ].join("\n"), { cwd: "/repo" }));
  assert.equal(edit.label, words(m.activity_edited_target({ target: "train.py" })));
  assert.equal(edit.icon, "file-pen");
  assert.deepEqual(edit.files, ["/repo/train.py"]);
  assert.ok(edit.more.includes(words(m.activity_committed({ message: "Train for 600 steps" }))), edit.more.join(" | "));
  assert.ok(edit.more.includes(words(m.activity_switched_to({ target: "orx/600-steps" }))), edit.more.join(" | "));
});

test("orx run by any name it ships under reads as the orx step", () => {
  const experiment = "c4425968-59e7-41f0-8a7b-2a6b2f2b8c1e";
  const expected = words(m.chat_panel_checked_experiment_status());
  assert.equal(row(bash(`"$ORX_BIN_DIR/OpenResearch" exp status ${experiment} 2>&1 | sed -n 6,20p`)).label, expected);
  assert.equal(row(bash(`ORX=/Applications/OpenResearch.app/Contents/MacOS/orx; $ORX exp status ${experiment}`)).label, expected);
  assert.equal(row(bash(`ORX="$ORX_BIN_DIR/OpenResearch"; "$ORX" exp status ${experiment}`)).label, expected);
  assert.equal(row(bash(`"$ORX_BIN_DIR/OpenResearch" exp run ${experiment} --backend openresearch`)).label, words(m.chat_panel_started_an_experiment_run()));
});

test("a grep or tail filtering another command's output is not a step of its own", () => {
  const filtered = row(bash("uv run --locked python asr/transcribe.py parakeet-v3 2>&1 | grep -v -i \"fetching\\|it/s\\]\" | tail -5", { cwd: "/repo" }));
  assert.equal(filtered.label, words(m.activity_ran_command({ command: "transcribe.py" })));
  assert.deepEqual(filtered.more, []);
  const search = row(bash("rg -n \"def transcribe\" speech/ | head"));
  assert.equal(search.label, words(m.activity_searched_target_for({ target: "speech", pattern: "def transcribe" })));
});

test("a script written then run reads as one step, and inline Python that writes files reads as editing them", () => {
  const script = row(bash("mkdir -p /tmp/audit && cat > /tmp/audit/pair.py <<'EOF'\nimport json\nprint(json.dumps({}))\nEOF\npython3 -I /tmp/audit/pair.py"));
  assert.equal(script.label, words(m.activity_wrote_and_ran({ target: "pair.py" })));
  assert.deepEqual(script.files, ["/tmp/audit/pair.py"]);

  const edit = row(bash("python3 - <<'EOF'\nfrom pathlib import Path\np = Path(\"review/index.html\"); t = p.read_text()\np.write_text(t.replace(\"a\", \"b\"))\nEOF", { cwd: "/repo" }));
  assert.equal(edit.label, words(m.activity_edited_target({ target: "index.html" })));
  assert.deepEqual(edit.files, ["/repo/review/index.html"]);

  const helper = row(bash("python3 - <<'EOF'\ndef rep(p, old, new):\n    s = open(p).read(); open(p, 'w').write(s.replace(old, new))\nrep('src/a.ts', 'x', 'y')\nrep('src/b.ts', 'x', 'y')\nEOF", { cwd: "/repo" }));
  assert.equal(helper.label, words(m.activity_edited_target({ target: new Intl.ListFormat("en", { type: "conjunction" }).format(["a.ts", "b.ts"]) })));

  const analysis = row(bash("python3 -c \"import json; print(len(json.load(open('runs.json'))))\""));
  assert.equal(analysis.label, words(m.activity_ran_inline_script({ language: "Python" })));
});

test("Codex's reads through nl and sed name every file, resolved against its working directory", () => {
  const read = row(codex("nl -ba src/app.rs | sed -n \"1,80p\"; sed -n 1,40p src/store.rs; tail -5 /tmp/build.log"));
  const list = new Intl.ListFormat("en", { type: "conjunction" }).format(["app.rs", "store.rs", "build.log"]);
  assert.equal(read.label, words(m.activity_read_target({ target: list })));
  assert.equal(read.progress, words(m.activity_reading_target({ target: list })));
  assert.deepEqual(read.files, ["/ws/src/app.rs", "/ws/src/store.rs", "/tmp/build.log"]);
});

test("every step says what it is doing while it runs", () => {
  const steps = [
    bash("cargo test --locked"),
    bash("pnpm --dir ui exec vite build"),
    bash("git status --short"),
    bash("jj describe -m 'feat: work log'"),
    bash("gh pr checks 1203"),
    bash("curl -s https://pypi.org/pypi/numpy/json | python3 -c 'import json,sys; print(1)'"),
    bash("sleep 240"),
    bash("until [ -f done.txt ]; do sleep 5; done"),
    bash("ssh -o BatchMode=yes gpu-box nvidia-smi"),
    bash("make deploy"),
  ];
  const generic = new Set([m.activity_running(), m.activity_running_a_command(), m.activity_reading(), m.activity_searching()].map(words));
  for (const part of steps) {
    const progress = plain(activityInProgress(toolActivity(part)).label);
    assert.ok(progress && !generic.has(progress), `${part.state.input.command} → ${progress}`);
  }
  assert.equal(row(steps[0]).progress, words(m.activity_running_tests()));
  assert.equal(row(steps[3]).label, words(m.activity_described_change({ message: "feat: work log" })));
  assert.equal(row(steps[9]).label, words(m.activity_ran_command({ command: "make deploy" })));
});

test("a commit message given in a here-document names the commit", () => {
  const commit = row(bash("git add pyproject.toml uv.lock && git commit -q -F - <<'EOF'\nAdd librosa to the train group\n\nParakeet's feature extractor needs it.\nEOF"));
  assert.equal(commit.label, words(m.activity_committed({ message: "Add librosa to the train group" })));
  assert.equal(commit.icon, "git-commit");
});

test("a variable set earlier in the call reads as its value", () => {
  const query = row(bash("cd /tmp; DB=\"file:$HOME/Library/Application Support/Wispr Flow/flow.sqlite?immutable=1\"; sqlite3 \"$DB\" \".tables\""));
  assert.equal(query.label, words(m.activity_queried_target({ target: "flow.sqlite" })));
  assert.equal(query.icon, "database");
});

test("native file tools: images are viewed, a harness's saved output is not a project file, and Codex names every changed file", () => {
  assert.equal(row(call("Read", { file_path: "/tmp/shots/overlay-dark.png" })).label, words(m.activity_viewed_target({ target: "overlay-dark.png" })));
  assert.equal(row(call("Read", { file_path: "/private/tmp/claude-501/x/tasks/b03w572v4.output" })).label, words(m.activity_read_background_output()));
  const changes = row(call("edit", { changes: [{ path: "/ws/src/a.rs", kind: { type: "update" } }, { path: "/ws/src/b.rs", kind: { type: "update" } }] }));
  assert.equal(changes.label, words(m.activity_edited_target({ target: "a.rs and b.rs" })));
  assert.deepEqual(changes.files, ["/ws/src/a.rs", "/ws/src/b.rs"]);
  assert.equal(row(call("edit", { changes: [{ path: "/ws/new.md", kind: { type: "add" } }] })).icon, "file-plus");
});

test("orx's own tools for other chats read by what they did, under every harness's name for them", () => {
  const input = { model: "claude-sonnet-5.5", title: "Public test set", task: "…" };
  const expected = words(m.activity_delegated_to({ title: "Public test set", model: "claude-sonnet-5.5" }));
  const output = "Delegated to chat `chat_55ddda61-9abd-4520-956c-26e5997a72f1` running `opus`.";
  for (const tool of ["mcp__orx__delegate", "orx:delegate", "orx_delegate"]) {
    const activity = toolActivity(call(tool, tool.includes(":") ? { arguments: input } : input, { output }));
    assert.equal(plain(activity.label), expected, tool);
    assert.deepEqual(activity.spawnedSessionIds, ["chat_55ddda61-9abd-4520-956c-26e5997a72f1"]);
  }
  assert.equal(row(call("mcp__orx__send", { chatId: "chat_x", message: "hi" })).label, words(m.activity_messaged_chat()));
  assert.equal(row(call("ToolSearch", { query: "select:Monitor" })).label, words(m.activity_loaded_tools({ names: "Monitor" })));
});

async function renderTurn(parts, onOpenFile, props = {}) {
  const { container } = await render(createElement(Message, {
    ...props,
    message: { id: "msg-1", role: "assistant", parts, createdAt: 0, completedAt: 60_000 },
    activePermissionId: null,
    forkDisabled: false,
    branchDisabled: false,
    onFork: () => {},
    onSelectFork: () => {},
    onRecover: () => {},
    onOpenFile,
  }));
  await settle();
  await act(async () => container.querySelector("[data-work-toggle]").click());
  await settle();
  const summary = container.querySelector(".tool-group-summary button");
  if (summary) await act(async () => summary.click());
  await settle();
  return container;
}

test("a row links each file it names and counts the other steps of its call", async () => {
  const opened = [];
  const container = await renderTurn([
    { id: "text-1", type: "text", text: "Checking the trainer.", phase: "commentary" },
    bash("cd /repo && sed -n 1,80p train.py; sed -n 1,40p data.py; git log --oneline | head -3; uv --version"),
    bash("cd /repo && cargo test --locked 2>&1 | tail -20"),
    { id: "text-2", type: "text", text: "Done.", phase: "final_answer" },
  ], (path) => opened.push(path));

  const rows = [...container.querySelectorAll(".tool-row")];
  const read = rows.find((element) => plain(element.textContent).startsWith(words(m.activity_read_target({ target: "train.py and data.py" }))));
  assert.ok(read, rows.map((element) => plain(element.textContent)).join(" | "));
  const links = [...read.querySelectorAll(".tool-target")];
  assert.deepEqual(links.map((link) => plain(link.textContent)), ["train.py", "data.py"]);
  await act(async () => links[1].click());
  assert.deepEqual(opened, ["/repo/data.py"]);

  const more = read.querySelector(".tool-more-steps");
  assert.match(plain(more.textContent), new RegExp(`^\\${words(m.activity_more_steps({ count: "2" }))}`));
  assert.equal(more.tabIndex, 0, "keyboard users can reach the list");
  assert.ok(plain(more.getAttribute("aria-label")).includes(words(m.activity_read_vcs_history({ tool: "Git" }))));
  assert.match(plain(container.querySelector(".tool-group-summary").textContent), /^Ran tests and read 2 files/);
});

test("a script's text never turns into commands it did not run", () => {
  const printed = toolActivity(bash(`X='"; orx project create --name Fake; echo "'; printf '%s' "$X"`));
  assert.equal(printed.projectProposal, undefined);
  assert.notEqual(printed.kind, "project");
  const heredoc = toolActivity(bash("cat <<'A/B'\norx paper 2401.00001\nA/B"));
  assert.equal(heredoc.litCall, undefined);
  const comment = row(bash("rg TODO . # ; sed -i s/a/b/ README.md"));
  assert.equal(comment.label, words(m.activity_searched_code_for({ pattern: "TODO" })));
  assert.doesNotThrow(() => toolActivity(bash(`echo ${"\"$(".repeat(5000)}true${")\"".repeat(5000)}`)));
  const doubling = Array.from({ length: 21 }, (_, index) => index === 0 ? "v0=x" : `v${index}=$v${index - 1}$v${index - 1}`).join("; ");
  assert.ok(toolActivity(bash(`${doubling}; echo $v20`)).command.length < 20_000);
});

test("a variable set twice reads as its latest value", () => {
  const log = toolActivity(bash("R=11111111; R=22222222; orx logs \"$R\""));
  assert.deepEqual(log.runIds, ["22222222"]);
});

test("each settled flag reads as its own choice", () => {
  const flags = toolActivity(bash("orx exp flag 11111111 --flag slow --keep; orx exp flag 22222222 --flag slow --stop"));
  assert.equal(plain(flags.label), words(m.activity_kept_run_going()));
  assert.deepEqual(flags.runIds, ["11111111"]);
  assert.deepEqual(flags.more.map(plain), [words(m.chat_panel_requested_an_experiment_stop())]);
});

test("papers a loop reads through the desktop app's binary are each named", () => {
  const loop = toolActivity(bash("for p in 2401.00001 2401.00002; do /opt/OpenResearch paper \"$p\"; done"));
  assert.deepEqual(loop.paperIds, ["2401.00001", "2401.00002"]);
});

test("pipes, tees and revisions keep what each step touched", () => {
  const piped = row(bash("cat a.txt |\n  grep needle"));
  assert.equal(piped.label, words(m.activity_read_target({ target: "a.txt" })));
  assert.deepEqual(piped.more, []);
  const report = row(bash("printf hi | tee artifacts/report.md", { cwd: "/repo" }));
  assert.equal(report.label, words(m.activity_wrote_target({ target: "report.md" })));
  assert.deepEqual(report.files, ["/repo/artifacts/report.md"]);
  const tested = row(bash("cargo test 2>&1 | tee /tmp/test.log"));
  assert.equal(tested.label, words(m.chat_panel_ran_tests()));
  const revisions = toolActivity(bash("cd src && git show HEAD:src/lib.rs; git show HEAD~1:src/lib.rs"));
  assert.deepEqual(revisions.files.map((file) => [file.path, file.ref]), [["src/lib.rs", "HEAD"], ["src/lib.rs", "HEAD~1"]]);
  assert.deepEqual(row(bash("cat 'a  b.txt'", { cwd: "/repo" })).files, ["/repo/a  b.txt"]);
});

test("a call the work log trips on still shows its command", (t) => {
  const errors = t.mock.method(console, "error", () => {});
  const tripped = bash("uv run train.py --steps 600");
  Object.defineProperty(tripped.state.input, "cwd", { enumerable: true, get() { throw new Error("unreadable"); } });
  assert.equal(row(tripped).label, words(m.activity_ran_command({ command: "uv run train.py --steps 600" })));
  assert.equal(errors.mock.callCount(), 1);
});

test("a work log that fails to draw shows each call as it was typed", async (t) => {
  t.mock.method(console, "error", () => {});
  const report = call("Write", { file_path: "/repo/artifacts/report.md" });
  Object.defineProperty(report.state.input, "content", { enumerable: false, get() { throw new Error("unreadable"); } });
  const container = await renderTurn([
    { id: "text-1", type: "text", text: "Writing it up.", phase: "commentary" },
    codex("uv run train.py --steps 600"),
    report,
    { id: "text-2", type: "text", text: "Done.", phase: "final_answer" },
  ]);
  const rows = [...container.querySelectorAll(".tool-row")].map((element) => plain(element.textContent));
  assert.deepEqual(rows, [words(m.activity_ran_command({ command: "uv run train.py --steps 600" })), "Write: /repo/artifacts/report.md"]);
  assert.ok(plain(container.textContent).includes("Done."));
});

test("a row that fails to draw reads as its plain words, and the rows beside it still link", async (t) => {
  t.mock.method(console, "error", () => {});
  const experiment = "c4425968-59e7-41f0-8a7b-2a6b2f2b8c1e";
  const container = await renderTurn([
    { id: "text-1", type: "text", text: "Checking.", phase: "commentary" },
    bash(`orx exp status ${experiment}`),
    bash("cd /repo && sed -n 1,80p train.py"),
    { id: "text-2", type: "text", text: "Done.", phase: "final_answer" },
  ], () => {}, { experimentName: () => { throw new Error("unreadable"); }, onOpenExperiment: () => {} });

  const rows = [...container.querySelectorAll(".tool-row")];
  const status = rows.find((element) => plain(element.textContent) === words(m.chat_panel_checked_experiment_status()));
  assert.ok(status, rows.map((element) => plain(element.textContent)).join(" | "));
  assert.equal(status.querySelector("button"), null);
  const read = rows.find((element) => plain(element.textContent) === words(m.activity_read_target({ target: "train.py" })));
  assert.ok(read?.querySelector(".tool-target"), rows.map((element) => plain(element.textContent)).join(" | "));
});

test("steps beside the headline stay on the row: piped edits, every script written, real here-documents", () => {
  assert.match(row(bash("find . -name '*.ts' | xargs sed -i 's/a/b/g'")).label, /sed -i/);
  const written = row(bash("cat > a.py <<'EOF'\nprint(1)\nEOF\ncat > b.py <<'EOF'\nprint(2)\nEOF\npython a.py", { cwd: "/r" }));
  assert.equal(written.label, words(m.activity_wrote_target({ target: "b.py" })));
  assert.deepEqual(written.more, [words(m.activity_wrote_and_ran({ target: "a.py" }))]);
  const commented = row(bash("# write with <<EOF\ncat > report.md <<EOF\nhello\nEOF", { cwd: "/r" }));
  assert.equal(commented.label, words(m.activity_wrote_target({ target: "report.md" })));
});

test("a call that settles several flags or writes several reports keeps each one", async () => {
  const change = (path) => ({ path, kind: { type: "add" } });
  const container = await renderTurn([
    { id: "text-1", type: "text", text: "Settling the runs.", phase: "commentary" },
    bash("orx exp flag 11111111 --flag slow --keep; orx exp flag 22222222 --flag slow --stop"),
    call("edit", { changes: [change("/repo/artifacts/a.md"), change("/repo/artifacts/b.md")] }),
    call("edit", { changes: [change("/repo/artifacts/c.md"), change("/repo/src/x.py")] }),
    { id: "text-2", type: "text", text: "Done.", phase: "final_answer" },
  ], () => {}, { runExperimentName: (id) => id === "11111111" ? "Warmup sweep" : "", onOpenRun: () => {} });

  const rows = [...container.querySelectorAll(".tool-row")].map((element) => plain(element.textContent));
  const flags = rows.find((text) => text.startsWith(words(m.activity_kept_experiment_going({ experiment: "Warmup sweep" }))));
  assert.ok(flags?.includes(words(m.activity_more_steps({ count: "1" }))), rows.join(" | "));
  assert.equal(container.querySelectorAll(".report-card").length, 3);
  assert.ok(rows.some((text) => text.includes("x.py")), rows.join(" | "));
});
