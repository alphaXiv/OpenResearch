import assert from "node:assert/strict";
import test from "node:test";
import {
  activeTurnTaskList,
  isTaskListTool,
  lastTaskList,
  parseTaskList,
  taskAllDone,
  toolBaseName,
} from "../src/taskProgress.ts";

const tool = (id, name, input, status = "completed") => ({ id, type: "tool", tool: name, state: { status, input } });
const message = (id, role, ...parts) => ({ id, role, parts, createdAt: 0 });

test("recognizes each harness's task-list tool by base name", () => {
  assert.equal(toolBaseName("mcp__planner__update_plan"), "update_plan");
  assert.equal(isTaskListTool("TodoWrite"), true);
  assert.equal(isTaskListTool("todowrite"), true);
  assert.equal(isTaskListTool("update_plan"), true);
  assert.equal(isTaskListTool("mcp__planner__update_plan"), true);
  assert.equal(isTaskListTool("todoread"), false);
  assert.equal(isTaskListTool("Bash"), false);
  assert.equal(isTaskListTool(undefined), false);
});

test("parses Claude Code TodoWrite input with active forms", () => {
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
    explanation: "Starting the fix",
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
  assert.equal(list.current, null);
  assert.equal(taskAllDone(list), true);
  const abandoned = parseTaskList(tool("x", "todowrite", { todos: [{ content: "Drop", status: "cancelled" }] }));
  assert.equal(taskAllDone(abandoned), false);
});

test("non-task tools, empty lists, and failed writes parse as null", () => {
  assert.equal(parseTaskList(tool("b", "Bash", { command: "ls" })), null);
  assert.equal(parseTaskList(tool("e", "TodoWrite", { todos: [] })), null);
  assert.equal(parseTaskList(tool("n", "TodoWrite", {})), null);
  assert.equal(parseTaskList(tool("d", "TodoWrite", { todos: [{ content: "a", status: "pending" }] }, "error")), null);
  assert.equal(parseTaskList({ id: "x", type: "text", text: "TodoWrite" }), null);
});

test("the last parsable task-list part in a message is the current one", () => {
  const first = tool("t1", "TodoWrite", { todos: [{ content: "a", status: "pending" }] });
  const second = tool("t2", "TodoWrite", { todos: [{ content: "a", status: "completed" }] });
  const denied = tool("t3", "TodoWrite", { todos: [{ content: "a", status: "pending" }] }, "error");
  const bash = tool("b", "Bash", { command: "ls" });
  assert.equal(lastTaskList([first, bash, second, bash, denied]).id, "t2");
  assert.equal(lastTaskList([first, bash, second, bash, denied]).list.done, 1);
  assert.equal(lastTaskList([bash]), null);
});

test("activeTurnTaskList only reads the tail assistant message", () => {
  const older = tool("t1", "TodoWrite", { todos: [{ content: "old", status: "in_progress" }] });
  const live = tool("t2", "TodoWrite", { todos: [{ content: "new", status: "in_progress" }] });
  assert.equal(activeTurnTaskList([message("a1", "assistant", older), message("u1", "user")]), null);
  assert.equal(activeTurnTaskList([message("a1", "assistant", older), message("u1", "user"), message("a2", "assistant")]), null);
  assert.equal(activeTurnTaskList([message("a1", "assistant", older), message("u1", "user"), message("a2", "assistant", live)]).current.text, "new");
});
