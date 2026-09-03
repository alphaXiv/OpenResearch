import assert from "node:assert/strict";
import test from "node:test";
import {
  activeTurnTaskList,
  foldTaskList,
  isTaskListTool,
  lastTaskList,
  parseTaskList,
  priorTaskLists,
  taskAllDone,
  toolBaseName,
} from "../src/taskProgress.ts";

const tool = (id, name, input, status = "completed", output) => ({ id, type: "tool", tool: name, state: { status, input, output } });
const message = (id, role, ...parts) => ({ id, role, parts, createdAt: 0 });

test("recognizes each harness's task-list tools by base name", () => {
  assert.equal(toolBaseName("mcp__planner__update_plan"), "update_plan");
  for (const name of ["TodoWrite", "todowrite", "update_plan", "TaskCreate", "TaskUpdate", "TaskList", "TaskGet"]) {
    assert.equal(isTaskListTool(name), true, name);
  }
  assert.equal(isTaskListTool("todoread"), false);
  assert.equal(isTaskListTool("Task"), false);
  assert.equal(isTaskListTool(undefined), false);
});

test("parses a whole-list TodoWrite with active forms", () => {
  const list = parseTaskList(tool("t1", "TodoWrite", {
    todos: [
      { content: "Read the config", status: "completed", activeForm: "Reading the config" },
      { content: "Run the tests", status: "in_progress", activeForm: "Running the tests" },
      { content: "Write the report", status: "pending", activeForm: "Writing the report" },
    ],
  }));
  assert.deepEqual(list, {
    items: [
      { text: "Read the config", status: "completed", activeText: "Reading the config" },
      { text: "Run the tests", status: "in_progress", activeText: "Running the tests" },
      { text: "Write the report", status: "pending", activeText: "Writing the report" },
    ],
    done: 1,
    total: 3,
    current: { text: "Run the tests", status: "in_progress", activeText: "Running the tests" },
  });
});

test("parses Codex update_plan steps with camel-case statuses", () => {
  const plan = parseTaskList(tool("p", "update_plan", {
    plan: [
      { step: "Inspect the repo", status: "completed" },
      { step: "Patch the loader", status: "inProgress" },
      { step: "Verify", status: "pending" },
    ],
  }));
  assert.equal(plan.done, 1);
  assert.equal(plan.total, 3);
  assert.equal(plan.current.text, "Patch the loader");
});

test("cancelled OpenCode todos are listed but leave the count", () => {
  const list = parseTaskList(tool("o", "todowrite", {
    todos: [
      { content: "Keep", status: "completed", priority: "high" },
      { content: "Drop", status: "cancelled", priority: "low" },
      { content: "", status: "pending" },
    ],
  }));
  assert.deepEqual(list.items.map((item) => item.status), ["completed", "cancelled"]);
  assert.equal(list.done, 1);
  assert.equal(list.total, 1);
  assert.equal(taskAllDone(list), true);
  assert.equal(taskAllDone(parseTaskList(tool("x", "todowrite", { todos: [{ content: "Drop", status: "cancelled" }] }))), false);
});

test("non-task tools, empty lists, and failed writes parse as null", () => {
  assert.equal(parseTaskList(tool("b", "Bash", { command: "ls" })), null);
  assert.equal(parseTaskList(tool("e", "TodoWrite", { todos: [] })), null);
  assert.equal(parseTaskList(tool("n", "TodoWrite", {})), null);
  assert.equal(parseTaskList(tool("d", "TodoWrite", { todos: [{ content: "a", status: "pending" }] }, "error")), null);
  assert.equal(parseTaskList({ id: "x", type: "text", text: "TodoWrite" }), null);
});

test("folds Claude Code TaskCreate / TaskUpdate / TaskList calls in order", () => {
  const parts = [
    tool("c1", "TaskCreate", { subject: "Inspect loader", description: "…", activeForm: "Inspecting loader" }, "completed", "Task #1 created successfully: Inspect loader"),
    tool("c2", "TaskCreate", { subject: "Run tests", activeForm: "Running tests" }, "running"),
    tool("u1", "TaskUpdate", { taskId: "1", status: "in_progress" }, "completed", "Updated task #1 status"),
    tool("bash", "Bash", { command: "ls" }),
    tool("u2", "TaskUpdate", { taskId: 1, status: "completed" }, "completed", "Updated task #1 status"),
    tool("u3", "TaskUpdate", { taskId: "9", status: "completed" }, "completed"),
    tool("denied", "TaskUpdate", { taskId: "2", status: "completed" }, "error"),
    tool("g", "TaskGet", { taskId: "1" }, "completed", "#1 ..."),
  ];
  const list = foldTaskList(parts, null);
  assert.deepEqual(list.items, [
    { id: "1", text: "Inspect loader", status: "completed", activeText: "Inspecting loader" },
    { id: "2", text: "Run tests", status: "pending", activeText: "Running tests" },
  ]);
  assert.equal(list.done, 1);
  assert.equal(list.total, 2);

  const listed = foldTaskList([
    ...parts,
    tool("l", "TaskList", {}, "completed", "#1 [completed] Inspect loader (worker) [blocked by #2]\n#2 [in_progress] Run tests"),
  ], null);
  assert.deepEqual(listed.items.map((item) => [item.id, item.text, item.status, item.activeText]), [
    ["1", "Inspect loader", "completed", "Inspecting loader"],
    ["2", "Run tests", "in_progress", "Running tests"],
  ]);
  assert.equal(foldTaskList([...parts, tool("l", "TaskList", {}, "completed", "No tasks found")], null), null);

  const deleted = foldTaskList([...parts, tool("d", "TaskUpdate", { taskId: "2", status: "deleted" })], null);
  assert.deepEqual(deleted.items.map((item) => item.id), ["1"]);
  assert.equal(taskAllDone(deleted), true);

  // A provisional id follows the harness's highest-id-plus-one rule.
  const gap = foldTaskList([
    tool("l", "TaskList", {}, "completed", "#1 [pending] A\n#3 [pending] C"),
    tool("c", "TaskCreate", { subject: "D" }, "running"),
  ], null);
  assert.deepEqual(gap.items.map((item) => [item.id, item.text]), [["1", "A"], ["3", "C"], ["4", "D"]]);
  assert.equal(lastTaskList(parts, null).id, "g");
  assert.equal(lastTaskList([tool("bash", "Bash", { command: "ls" })], null), null);
});

test("incremental calls build on the list from earlier turns", () => {
  const first = message("a1", "assistant", tool("c1", "TaskCreate", { subject: "Step one" }, "completed", "Task #1 created successfully: Step one"));
  const second = message("a2", "assistant", tool("u1", "TaskUpdate", { taskId: "1", status: "completed" }));
  const messages = [first, message("u", "user"), second];
  const priors = priorTaskLists(messages);
  assert.equal(priors.get("a1"), null);
  assert.equal(priors.get("a2").items[0].status, "pending");
  // Messages without task calls keep the prior list by identity and get no entry.
  const untouched = message("a4", "assistant", tool("b", "Bash", {}));
  const before = priors.get("a2");
  assert.equal(foldTaskList(untouched.parts, before), before);
  assert.equal(priorTaskLists([...messages, message("u2", "user"), untouched]).has("a4"), false);
  assert.equal(activeTurnTaskList(messages).items[0].status, "completed");
  assert.equal(activeTurnTaskList([first, message("u", "user")]), null);
  assert.equal(activeTurnTaskList([first, message("u", "user"), message("a3", "assistant", tool("b", "Bash", {}))]), null);
});

test("a whole-list write replaces whatever was folded before it", () => {
  const list = foldTaskList([
    tool("c1", "TaskCreate", { subject: "Old" }, "completed", "Task #1 created successfully: Old"),
    tool("w", "TodoWrite", { todos: [{ content: "New", status: "in_progress" }] }),
  ], null);
  assert.deepEqual(list.items.map((item) => item.text), ["New"]);
});
