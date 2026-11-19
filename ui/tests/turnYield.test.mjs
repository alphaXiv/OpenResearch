import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { chatNeedsYou, chatStatus } from "../src/chatStatus.ts";
import { groupResponseMessages, lastResponseText, partIsVisible, transcriptMarkdown } from "../src/chatRendering.ts";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, emit, renderChat, session, userMessage } from "./chat.mjs";
import { find, press, settle, type } from "./render.mjs";

const NEED = { kind: "need", text: "Share the held-out corpus before I compare accuracy." };
const tool = (name, input = {}, status = "completed") => ({ id: `${name}-${status}`, type: "tool", tool: name, state: { status, input } });
const answer = (id, text, turnYield, extra = {}) => ({ ...assistantMessage(id, [{ id: `${id}-text`, type: "text", text, phase: "final_answer" }]), turnYield, ...extra });
const close = () => document.querySelector("[data-turn-close]");
/** The transcript's live region, which sits just before its feed. */
const liveRegion = () => document.querySelector('[role="feed"]').previousElementSibling;
const announced = () => liveRegion().textContent;

test("the yield call hides behind the closing line it produced, under every agent's name and as a lone command", () => {
  for (const name of ["mcp__orx__yield", "orx:yield", "orx_yield", "orx/yield"]) {
    assert.equal(partIsVisible(tool(name, { arguments: NEED })), false, name);
  }
  assert.equal(partIsVisible(tool("bash", { command: `orx yield need "${NEED.text}"` })), false);
  assert.equal(partIsVisible(tool("bash", { command: `/bin/zsh -lc 'orx yield done "Finished the sweep."'` })), false);
  assert.equal(partIsVisible(tool("bash", { commandArgv: ["/bin/zsh", "-lc", "orx yield running 'Waiting on the sweep.'"] })), false);

  assert.equal(partIsVisible(tool("mcp__orx__yield", { arguments: NEED }, "error")), true, "a refused yield stays visible");
  assert.equal(partIsVisible(tool("bash", { command: "git commit -am wip && orx yield done 'ok'" })), true, "a compound command does more than yield");
  assert.equal(partIsVisible(tool("bash", { command: "orx exp run --yield-later" })), true);
  assert.equal(partIsVisible(tool("mcp__orx__delegate")), true);
});

test("a yield closes its response group and rides along in copy and export", () => {
  const first = { ...assistantMessage("a1", [tool("mcp__orx__yield")]), turnYield: { kind: "running", text: "Waiting on the sweep." } };
  const wake = answer("a2", "The sweep finished at 2.4% WER.", { kind: "done", text: "Finished the sweep." });
  assert.deepEqual(groupResponseMessages([userMessage("u", "go"), first, wake]).map((group) => group.map((row) => row.id)), [["u"], ["a1"], ["a2"]]);

  assert.equal(lastResponseText([first]), "Waiting on the sweep.");
  assert.equal(lastResponseText([first, wake]), "The sweep finished at 2.4% WER.\n\nFinished the sweep.");
  assert.equal(
    transcriptMarkdown("Sweep", [userMessage("u", "go"), first], { user: "You", assistant: "Codex" }),
    "# Sweep\n\n## You\n\ngo\n\n## Codex\n\nWaiting on the sweep.\n",
  );
});

test("an idle chat whose agent needs something is the user's turn, until a turn runs or the need is gone", () => {
  const chat = session("s", { yieldKind: "need" });
  assert.equal(chatNeedsYou(chat, false, false), true);
  assert.equal(chatNeedsYou(chat, true, false), false);
  assert.equal(chatNeedsYou({ ...chat, yieldKind: "running" }, false, false), false);
  assert.equal(chatNeedsYou({ ...chat, yieldKind: null }, false, true), true);
  assert.equal(chatNeedsYou({ ...chat, yieldKind: undefined }, false, false), false);
  assert.deepEqual(chatStatus(chat, false, chatNeedsYou(chat, false, false), []), { kind: "input" });
});

test("a turn ends with its closing line after the answer, labelled by why it stopped", async () => {
  await renderChat("/projects/p/tasks/s", {
    transcripts: { s: [userMessage("u", "Compare the fine-tune with the baseline."), answer("a", "The fine-tune reaches 3.21% WER.", NEED)] },
  });
  const line = close();
  assert.ok(line, "a closing line");
  assert.equal(line.textContent, NEED.text);
  assert.equal(line.querySelector('[role="img"]').getAttribute("aria-label"), m.chat_turn_close_need());
  const reply = find(".msg-assistant .md", "3.21% WER");
  assert.ok(reply.compareDocumentPosition(line) & Node.DOCUMENT_POSITION_FOLLOWING, "after the answer");
  assert.equal(document.querySelectorAll("[data-turn-close]").length, 1);
});

test("a reply that is only a yield stays visible, and its call shows no second copy", async () => {
  const yieldOnly = { ...assistantMessage("a", [tool("mcp__orx__yield", { arguments: { kind: "running", text: "Waiting on two runs." } })]), turnYield: { kind: "running", text: "Waiting on two runs." } };
  await renderChat("/projects/p/tasks/s", { transcripts: { s: [userMessage("u", "Start the sweep."), yieldOnly] } });
  assert.equal(close()?.textContent, "Waiting on two runs.");
  assert.equal(document.body.textContent.split("Waiting on two runs.").length, 2, "shown once");
  assert.equal(close().querySelector('[role="img"]').getAttribute("aria-label"), m.chat_turn_close_running());
});

test("a failed turn shows its recovery, never a closing line", async () => {
  const failed = {
    ...assistantMessage("a", [
      { id: "a-text", type: "text", text: "Starting the comparison.", phase: "final_answer" },
      { id: "turn-recovery", type: "tool", tool: "turn_recovery", state: { status: "error", error: "The agent stopped.", input: { turnId: "t", recoveryAction: "continue" } } },
    ]),
    turnYield: NEED,
  };
  await renderChat("/projects/p/tasks/s", { transcripts: { s: [userMessage("u", "Compare."), failed] } });
  assert.equal(close(), null);
});

test("a closing line that arrives with the finished turn is announced once", async () => {
  const running = { ...assistantMessage("a", [{ id: "a-text", type: "text", text: "Comparing now.", phase: "final_answer" }]), completedAt: null };
  await renderChat("/projects/p/tasks/s", { transcripts: { s: [userMessage("u", "Compare."), running] } });
  assert.equal(close(), null);

  await emit("chat.message", { sessionId: "s", message: { ...running, completedAt: 3, turnYield: NEED } });
  assert.equal(close()?.textContent, NEED.text);
  assert.equal(announced(), m.announcement_turn_close({ status: m.chat_turn_close_need(), text: NEED.text }));

  const spoken = liveRegion().firstElementChild;
  await emit("chat.message", { sessionId: "s", message: { ...running, completedAt: 3, turnYield: NEED, parts: [...running.parts] } });
  assert.equal(liveRegion().firstElementChild, spoken, "not announced again");
});

test("chats waiting on the user stay in Inbox while active turns go in Working", async () => {
  const sessions = [
    session("need", { yieldKind: "need", updatedAt: 5 }),
    session("asking", { awaitingInput: true, updatedAt: 4 }),
    session("busy", { yieldKind: "need", busy: true, updatedAt: 3 }),
    session("done", { yieldKind: "done", updatedAt: 2 }),
  ];
  localStorage.setItem("sidebar-grouping", "inbox");
  try {
    await renderChat("/projects/p/tasks/done", {
      sessions,
      routes: {
        "GET /api/chat/sessions": (_, url) => ({
          sessions: url.includes("scope=sidebar") ? url.includes("archived=true") ? [] : sessions : sessions.filter((row) => url.includes(`projectId=${row.projectId}`)),
        }),
      },
    });
    const heading = (label) => [...document.querySelectorAll("button[aria-expanded]")].find((button) => button.textContent.startsWith(label));
    // A row that just left stays drawn, inert, while it fades.
    const row = (title) => [...document.querySelectorAll(".session-row")].find((element) => !element.closest("[data-rail-ghost]") && element.textContent.includes(title));
    const order = (a, b) => Boolean(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
    const inbox = heading(m.sidebar_inbox());
    const working = heading(m.chat_inbox_working());
    assert.ok(inbox && working);
    assert.ok(order(inbox, row("Chat need")) && order(inbox, row("Chat asking")));
    assert.ok(order(row("Chat asking"), row("Chat done")) && order(working, inbox), "an open chat stays in Inbox");
    assert.ok(order(working, row("Chat busy")) && order(row("Chat busy"), inbox), "a running turn is work, whatever the last one asked");
    assert.ok(row("Chat need").getAttribute("aria-label").includes(m.chat_panel_waiting_for_your_input()));

    await act(async () => inbox.click());
    await settle();
    assert.equal(heading(m.sidebar_inbox()).getAttribute("aria-expanded"), "false");
    assert.ok(heading(m.sidebar_inbox()).textContent.includes("3"));
    assert.equal(row("Chat need"), undefined);
    await act(async () => heading(m.sidebar_inbox()).click());
    await settle();

    sessions[0] = { ...sessions[0], busy: true };
    await emit("chat.session", { session: sessions[0] });
    await act(async () => new Promise((resolve) => setTimeout(resolve, 150)));
    await settle();
    assert.ok(order(working, row("Chat need")) && order(row("Chat need"), inbox), "a new turn takes the chat to Working");
  } finally {
    localStorage.removeItem("sidebar-grouping");
    localStorage.removeItem("sidebar-inbox-collapsed");
  }
});

test("the Goal chip opens the goal to edit, keeps a draft through a change elsewhere, and returns focus", async () => {
  const chat = session("s", { goal: "Beat 2.4% WER within four GPU-hours." });
  const patches = [];
  const view = await renderChat("/projects/p/tasks/s", {
    sessions: [chat],
    routes: { "PATCH /api/chat/sessions/s": (init) => {
      const body = JSON.parse(init.body);
      patches.push(body);
      return { session: { ...chat, ...body } };
    } },
  });
  const chip = () => [...document.querySelectorAll('button[aria-haspopup="dialog"]')]
    .find((button) => button.getAttribute("aria-label")?.startsWith(m.chat_panel_edit_goal({ goal: "" })));
  const dialog = () => document.querySelector('[role="dialog"]');
  const open = async () => {
    chip().focus();
    await act(async () => chip().click());
    await act(async () => new Promise((resolve) => requestAnimationFrame(resolve)));
    await settle();
  };

  await open();
  assert.ok(dialog(), "a dialog");
  assert.equal(dialog().querySelector("textarea").value, chat.goal);
  await press("Escape");
  await settle();
  assert.equal(dialog(), null);
  assert.equal(document.activeElement, chip());
  assert.deepEqual(patches, []);

  await open();
  await type(dialog().querySelector("textarea"), "Beat 2.4% WER within six GPU-hours.");
  await settle();
  await emit("chat.session", { session: { ...chat, goal: "Beat 2.4% WER within two GPU-hours." } });
  assert.ok(dialog().textContent.includes(m.goal_dialog_changed()));
  assert.equal(dialog().querySelector("textarea").value, "Beat 2.4% WER within six GPU-hours.", "the draft stays");
  await act(async () => find("button", m.goal_dialog_load_saved()).click());
  await settle();
  assert.equal(dialog().querySelector("textarea").value, "Beat 2.4% WER within two GPU-hours.");
  assert.equal(dialog().textContent.includes(m.goal_dialog_changed()), false);

  await type(dialog().querySelector("textarea"), "  Beat 2.4% WER within three GPU-hours.  ");
  await act(async () => find("button", m.common_save()).click());
  await settle();
  assert.deepEqual(patches, [{ goal: "Beat 2.4% WER within three GPU-hours." }]);
  assert.equal(dialog(), null);

  await open();
  await act(async () => find("button", m.goal_dialog_clear()).click());
  await settle();
  assert.deepEqual(patches.at(-1), { goal: null });
  assert.equal(dialog(), null);
  void view;
});
