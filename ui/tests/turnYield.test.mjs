import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { chatNeedsYou, chatStatus } from "../src/chatStatus.ts";
import { groupResponseMessages, hasPendingInput, hideClosedYieldCalls, lastResponseText, transcriptMarkdown } from "../src/chatRendering.ts";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, emit, renderChat, session, userMessage } from "./chat.mjs";
import { find, press, settle, type } from "./render.mjs";
import { composer } from "./chat.mjs";

const NEED = { kind: "need", text: "Share the held-out corpus before I compare accuracy." };
const tool = (name, input = {}, status = "completed") => ({ id: `${name}-${status}`, type: "tool", tool: name, state: { status, input } });
const answer = (id, text, turnYield, extra = {}) => ({ ...assistantMessage(id, [{ id: `${id}-text`, type: "text", text, phase: "final_answer" }]), turnYield, ...extra });
const close = () => document.querySelector("[data-turn-close]");
/** The transcript's live region, which sits just before its feed. */
const liveRegion = () => document.querySelector('[role="feed"]').previousElementSibling;
const announced = () => liveRegion().textContent;

test("the yield call hides behind the closing line it produced, under every agent's name and as a lone command", () => {
  const visible = (part, fields = { turnYield: NEED }) => hideClosedYieldCalls([{ ...assistantMessage("a", [part]), ...fields }])[0].parts.length === 1;
  for (const name of ["mcp__orx__yield", "orx:yield", "orx_yield", "orx/yield"]) {
    assert.equal(visible(tool(name, { arguments: NEED })), false, name);
  }
  assert.equal(visible(tool("bash", { command: `orx yield need "${NEED.text}"` })), false);
  assert.equal(visible(tool("bash", { command: `/bin/zsh -lc 'orx yield done "Finished the sweep."'` })), false);
  assert.equal(visible(tool("bash", { commandArgv: ["/bin/zsh", "-lc", "orx yield running 'Waiting on the sweep.'"] })), false);
  assert.equal(visible(tool("mcp__orx__yield"), { completedAt: null }), false, "a streaming turn's close is still on its way");

  assert.equal(visible(tool("mcp__orx__yield"), {}), true, "a turn that ended without its close keeps the call");
  assert.equal(visible(tool("mcp__orx__yield", { arguments: NEED }, "error")), true, "a refused yield stays visible");
  assert.equal(visible(tool("bash", { command: "git commit -am wip && orx yield done 'ok'" })), true, "a compound command does more than yield");
  assert.equal(visible(tool("bash", { command: "orx exp run --yield-later" })), true);
  assert.equal(visible(tool("mcp__orx__delegate")), true);
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

test("a plan waiting for approval is the user's turn until it is answered or a newer message passes it", () => {
  const plan = (fields = {}) => ({ ...assistantMessage("a", [{ id: "plan", type: "prompt", prompt: { kind: "plan", plan: "Compare on the held-out set.", resolved: false, ...fields } }]), parentId: "u" });
  const ask = userMessage("u", "Plan the comparison.");
  assert.equal(hasPendingInput([ask, plan()], false), true);
  assert.equal(hasPendingInput([ask, plan({ synthesized: true })], false), true);
  assert.equal(hasPendingInput([ask, plan({ resolved: true, approved: true })], false), false);
  assert.equal(hasPendingInput([ask, plan(), userMessage("u2", "Skip the plan and start.")], false), false, "passed by a newer message");
  assert.equal(hasPendingInput([ask, plan({ nativeId: "rpc-1" })], false), false, "a held card needs its live turn");
  assert.equal(hasPendingInput([ask, plan({ nativeId: "rpc-1" })], true), true);
});

test("a turn that ended without its close keeps the yield call in the activity, saying what it asked", async () => {
  const call = tool("mcp__orx__yield", { arguments: NEED });
  const stopped = assistantMessage("a", [{ id: "a-text", type: "text", text: "Reached 3.21% WER.", phase: "final_answer" }, call]);
  await renderChat("/projects/p/tasks/s", { transcripts: { s: [userMessage("u", "Compare."), stopped] } });
  assert.equal(close(), null);
  const row = document.querySelector(`[data-part-id="${call.id}"]`);
  assert.ok(row, "the call is in the activity");
  assert.equal(row.textContent, m.chat_turn_close_with_status({ status: m.chat_turn_close_need(), text: NEED.text }));
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
  assert.equal(announced(), m.chat_turn_close_with_status({ status: m.chat_turn_close_need(), text: NEED.text }));

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

for (const [answer, approve] of [[m.plan_strip_accept_plan, true], [m.plan_strip_reject, false]]) {
  test(`a plan in the open chat shows an input status until the user ${approve ? "accepts" : "rejects"} it`, async () => {
    const current = { ...assistantMessage("a", [{ id: "a-plan", type: "prompt", prompt: { kind: "plan", plan: "Compare on the held-out set.", resolved: false, synthesized: true } }]), parentId: "u" };
    const sessions = [session("plan", { planMode: true, updatedAt: 2 }), session("other", { updatedAt: 1 })];
    localStorage.setItem("sidebar-grouping", "inbox");
    try {
      await renderChat("/projects/p/tasks/plan", {
        sessions,
        transcripts: { plan: [userMessage("u", "Plan the comparison."), current] },
        routes: {
          "GET /api/chat/sessions": (_, url) => ({ sessions: url.includes("archived=true") ? [] : sessions }),
          "POST /api/chat/sessions/plan/respond": (init) => {
            assert.equal(JSON.parse(init.body).approve, approve);
            current.parts[0].prompt = { ...current.parts[0].prompt, resolved: true, approved: approve };
            return { ok: true };
          },
        },
      });
      const row = () => [...document.querySelectorAll(".session-row")].find((element) => !element.closest("[data-rail-ghost]") && element.textContent.includes("Chat plan"));
      assert.ok(row().textContent.includes(m.chat_status_input()));
      assert.ok(row().getAttribute("aria-label").includes(m.chat_panel_waiting_for_your_input()));

      await act(async () => find("button", answer()).click());
      await settle();
      await act(async () => new Promise((resolve) => setTimeout(resolve, 150)));
      await settle();
      assert.ok(!row().textContent.includes(m.chat_status_input()));
      assert.equal(row().getAttribute("aria-label").includes(m.chat_panel_waiting_for_your_input()), false);
    } finally {
      localStorage.removeItem("sidebar-grouping");
      localStorage.removeItem("sidebar-inbox-collapsed");
    }
  });
}

test("the Goal chip opens the goal to edit, keeps a draft through a change elsewhere, and returns focus", async () => {
  let chat = session("s", { goal: "Beat 2.4% WER within four GPU-hours." });
  const patches = [];
  const view = await renderChat("/projects/p/tasks/s", {
    sessions: [chat],
    routes: {
      "GET /api/chat/sessions": (_, url) => ({ sessions: url.includes("archived=true") ? [] : [chat] }),
      "PATCH /api/chat/sessions/s": (init) => {
        const body = JSON.parse(init.body);
        patches.push(body);
        chat = { ...chat, ...body };
        return { session: chat };
      },
    },
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
  assert.ok(dialog().querySelector('[role="status"]'), "the change notice's live region is there before it speaks");
  await press("Escape");
  await settle();
  assert.equal(dialog(), null);
  assert.equal(document.activeElement, chip());
  assert.deepEqual(patches, []);

  await open();
  await type(dialog().querySelector("textarea"), "Beat 2.4% WER within six GPU-hours.");
  await settle();
  chat = { ...chat, goal: "Beat 2.4% WER within two GPU-hours." };
  await emit("chat.session", { session: chat });
  assert.ok(dialog().textContent.includes(m.goal_dialog_changed()));
  assert.equal(dialog().querySelector("textarea").value, "Beat 2.4% WER within six GPU-hours.", "the draft stays");
  const load = find("button", m.goal_dialog_load_saved());
  load.focus();
  await act(async () => load.click());
  await settle();
  assert.equal(dialog().querySelector("textarea").value, "Beat 2.4% WER within two GPU-hours.");
  assert.equal(document.activeElement, dialog().querySelector("textarea"), "focus moves to the loaded goal");
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
  assert.ok(!chip(), "a cleared goal has no chip");
  assert.ok(composer().field().contains(document.activeElement), "focus returns to the composer");
  void view;
});
