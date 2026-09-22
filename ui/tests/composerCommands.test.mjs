import assert from "node:assert/strict";
import test from "node:test";
import {
  commandMatchesQuery,
  commandsForHarness,
  effectiveCommandPlanMode,
  parseComposerCommand,
  insertSlashCommand,
  removeSlashCommand,
  slashCommandContext,
  splitCommandTokens,
} from "../src/composerCommands.ts";

test("a selected skill can be removed with its trailing composer spaces", () => {
  const selected = insertSlashCommand("/deb", { start: 0, end: 4, query: "deb" }, "debate", 2);
  const tokenEnd = selected.text.trimEnd().length;
  const context = slashCommandContext(selected.text, tokenEnd);
  assert.ok(context);
  assert.deepEqual(removeSlashCommand(selected.text, { ...context, end: selected.cursor }), {
    text: "",
    cursor: 0,
  });
});

const ALL_COMMANDS = ["compact", "copy", "export", "goal", "model", "new", "plan", "resume"];

test("every harness gets the same commands ahead of its skills", () => {
  const skills = [{ name: "review", description: "Review", source: "user" }];
  for (const activation of ["command", "permission"]) {
    assert.deepEqual(commandsForHarness(skills, activation).map((item) => item.name), [
      ...ALL_COMMANDS,
      "review",
    ]);
  }
});

test("Plan is only offered where the harness can plan", () => {
  assert.deepEqual(
    commandsForHarness([], null).map((item) => item.name),
    ALL_COMMANDS.filter((name) => name !== "plan"),
  );
});

test("built-in commands replace user-skill collisions, aliases included", () => {
  const skills = [
    { name: "PLAN", description: "Legacy collision", source: "user" },
    { name: "export", description: "Collision", source: "builtin" },
    { name: "clear", description: "Alias collision", source: "user" },
    { name: "export", description: "Plugin collision", source: "user", plugin: "acme" },
    { name: "review", description: "Review", source: "user" },
  ];
  const commands = commandsForHarness(skills, "command");
  // A plugin's skill is inserted and resolved by its bare name too, so it
  // shadows just like any other; only names that cannot collide survive.
  assert.deepEqual(commands.map((item) => item.name), [...ALL_COMMANDS, "review"]);
  assert.ok(commands.filter((item) => item.name !== "review").every((item) => item.source === "command"));
});

test("Plan is recognized and removed anywhere in the message", () => {
  assert.deepEqual(parseComposerCommand("/plan", "command"), { name: "plan", prompt: "" });
  assert.deepEqual(parseComposerCommand("investigate /PLAN this", "command"), {
    name: "plan",
    prompt: "investigate this",
  });
  assert.deepEqual(parseComposerCommand("first\n/plan\nsecond /plan", "permission"), {
    name: "plan",
    prompt: "first\nsecond",
  });
  assert.equal(parseComposerCommand("/planner", "command"), null);
  assert.equal(parseComposerCommand("https://example.com/plan", "command"), null);
});

test("aliases run their command without being listed separately", () => {
  const menu = commandsForHarness([], "command");
  assert.deepEqual(menu.map((item) => item.name), ALL_COMMANDS);
  assert.deepEqual(parseComposerCommand("/clear", null), { name: "new", prompt: "" });
  assert.deepEqual(parseComposerCommand("/summarize", null), { name: "compact", prompt: "" });
  assert.deepEqual(parseComposerCommand("  /CLEAR  ", null), { name: "new", prompt: "" });
  assert.equal(parseComposerCommand("/cleared", null), null);
  // The alias still finds its command in the menu.
  assert.ok(commandMatchesQuery(menu.find((item) => item.name === "new"), "cle"));
  assert.ok(!commandMatchesQuery(menu.find((item) => item.name === "copy"), "cle"));
  assert.ok(!commandMatchesQuery({ name: "review", description: "", source: "user" }, "cle"));
});

test("Goal takes the rest of the message, but only when it leads", () => {
  assert.deepEqual(parseComposerCommand("/goal ship the sweep", "command"), {
    name: "goal",
    prompt: "ship the sweep",
  });
  assert.deepEqual(parseComposerCommand("/goal", "command"), { name: "goal", prompt: "" });
  assert.deepEqual(parseComposerCommand("/goal clear", "command"), { name: "goal", prompt: "clear" });
  // Mid-sentence it is prose, like every other non-plan command.
  assert.equal(parseComposerCommand("remind me what the /goal was", "command"), null);
  assert.equal(parseComposerCommand("/goalie", "command"), null);
  // A command named inside the goal is part of the goal, not a command.
  assert.deepEqual(parseComposerCommand("/goal keep the /plan in sync", "command"), {
    name: "goal",
    prompt: "keep the /plan in sync",
  });
});

test("only Plan composes with a prompt; the rest must be the whole message", () => {
  assert.deepEqual(parseComposerCommand("/export", null), { name: "export", prompt: "" });
  assert.equal(parseComposerCommand("/plan", null), null);
  assert.equal(parseComposerCommand("/newer idea", "command"), null);
  // Prose that merely mentions a command still reaches the agent.
  for (const text of [
    "what does /clear do?",
    "the files under /export are stale",
    "/copy this file for me",
  ])
    assert.equal(parseComposerCommand(text, "command"), null);
  // Plan is the exception, and wins over a mention of another command.
  assert.deepEqual(parseComposerCommand("/plan the /export flow", "command"), {
    name: "plan",
    prompt: "the /export flow",
  });
});

test("slash context follows the caret anywhere in the message", () => {
  assert.deepEqual(slashCommandContext("/pl", 3), { query: "pl", start: 0, end: 3 });
  assert.deepEqual(slashCommandContext("investigate /pl now", 15), {
    query: "pl",
    start: 12,
    end: 15,
  });
  assert.deepEqual(slashCommandContext("investigate / now", 13), {
    query: "",
    start: 12,
    end: 13,
  });
  assert.equal(slashCommandContext("investigate/path", 16), null);
  // Where onChange looks once the space that finished a command lands.
  assert.deepEqual(slashCommandContext("investigate /plan now", 17), {
    query: "plan",
    start: 12,
    end: 17,
  });
});

const isWrite = (name) => name === "write";

test("known command tokens split out wherever they were typed", () => {
  assert.deepEqual(splitCommandTokens("use the /write skill", isWrite), [
    { text: "use the ", command: false },
    { text: "/write", command: true },
    { text: " skill", command: false },
  ]);
  assert.deepEqual(splitCommandTokens("/Write now", isWrite), [
    { text: "/Write", command: true },
    { text: " now", command: false },
  ]);
  assert.deepEqual(splitCommandTokens("/write ", isWrite), [
    { text: "/write", command: true },
    { text: " ", command: false },
  ]);
  assert.deepEqual(splitCommandTokens("", isWrite), []);
  assert.deepEqual(splitCommandTokens("no commands here", isWrite), [
    { text: "no commands here", command: false },
  ]);
  // Unknown commands, paths, and URLs stay plain text.
  assert.deepEqual(splitCommandTokens("/unknown /src/write https://x.dev/write", isWrite), [
    { text: "/unknown /src/write https://x.dev/write", command: false },
  ]);
});

test("splitting a message loses nothing — the chips are painted by offset", () => {
  for (const text of [
    "use the /write skill",
    "/write",
    "  /write  two  spaces  ",
    "line one\n/write args\n\nline three",
    "/write/write /write\t/write",
  ])
    assert.equal(
      splitCommandTokens(text, isWrite)
        .map((segment) => segment.text)
        .join(""),
      text,
    );
});

test("picking a command replaces the token in place", () => {
  const text = "look at /wr now";
  // The caret lands in the args, past the space that already followed.
  assert.deepEqual(insertSlashCommand(text, slashCommandContext(text, 11), "write"), {
    text: "look at /write now",
    cursor: 15,
  });
  // A command ending the draft gets the space its args will need.
  const tail = "look at /wr";
  assert.deepEqual(insertSlashCommand(tail, slashCommandContext(tail, 11), "write"), {
    text: "look at /write ",
    cursor: 15,
  });
});

test("skill insertion can reserve its full hover margin", () => {
  const text = "look at /wr now";
  assert.deepEqual(insertSlashCommand(text, slashCommandContext(text, 11), "write", 2), {
    text: "look at /write  now",
    cursor: 16,
  });
  const indented = "\t/wr now";
  assert.deepEqual(insertSlashCommand(indented, slashCommandContext(indented, 4), "write", 2), {
    text: "\t/write  now",
    cursor: 9,
  });
});

test("removing a slash command preserves the surrounding message", () => {
  assert.deepEqual(
    removeSlashCommand("/plan investigate", { query: "plan", start: 0, end: 5 }),
    { text: "investigate", cursor: 0 },
  );
  assert.deepEqual(
    removeSlashCommand("investigate /plan this", { query: "plan", start: 12, end: 17 }),
    { text: "investigate this", cursor: 12 },
  );
  assert.deepEqual(removeSlashCommand("investigate /plan", { query: "plan", start: 12, end: 17 }), {
    text: "investigate",
    cursor: 11,
  });
});

test("a requested toggle overrides pending Plan state for an immediate send", () => {
  assert.equal(effectiveCommandPlanMode("command", undefined, false), false);
  assert.equal(effectiveCommandPlanMode("command", undefined, true), true);
  assert.equal(effectiveCommandPlanMode("command", true, false), true);
  assert.equal(effectiveCommandPlanMode("command", false, true), false);
  assert.equal(effectiveCommandPlanMode("permission", true, true), undefined);
  assert.equal(effectiveCommandPlanMode("command", undefined, null), undefined);
});
