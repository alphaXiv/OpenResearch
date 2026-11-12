import assert from "node:assert/strict";
import test from "node:test";
import { Activity, act, createElement } from "react";
import { ComposerProjectPicker } from "../src/components/ComposerProjectPicker.tsx";
import { RemoteStopDialog } from "../src/components/RemoteStopDialog.tsx";
import { showSshConfigDialog } from "../src/components/SshConfigDialog.tsx";
import { createDialogs } from "../src/dialog.tsx";
import { m } from "../src/paraglide/messages.js";
import { find, press, render, serve, settle, type } from "./render.mjs";

const dialogs = () => [...document.querySelectorAll('[role="dialog"], [role="alertdialog"]')];
const click = async (element) => {
  await act(async () => element.click());
  await settle();
};

test("the project picker prioritizes the current project, then recent activity, and selects the requested project", async () => {
  const selections = [];
  await render(createElement(ComposerProjectPicker, {
    projects: [{ id: "older", name: "Older", updatedAt: 1 }, { id: "current", name: "Current", updatedAt: 2 }, { id: "newer", name: "Newer", updatedAt: 20 }],
    activity: [{ projectId: "older", lastActivityAt: 30 }],
    projectId: "current",
    projectName: "Current",
    onSelect: (id) => selections.push(id),
    onNewProject() {},
  }));
  await click(find("button", "Current"));
  const choices = [...document.querySelectorAll('[role="menuitemradio"]')];
  assert.deepEqual(choices.map((choice) => choice.textContent), ["Current", "Older", "Newer"]);
  await click(choices[1]);
  assert.deepEqual(selections, ["older"]);
});

test("remote stop cancellation is blocked while stopping, including Escape", async () => {
  let closed = 0;
  let finishStop;
  const { workspace } = await render(null);
  const show = () => act(async () => workspace.dialogs.showCustomDialog({
    component: RemoteStopDialog,
    props: { host: "research", preview: { attachmentCount: 1, activeTurnCount: 0, pendingPermissionCount: 0, queuedMessageCount: 0, activeRunCount: 0 }, currentClientAttached: true },
    onCancel: () => closed++,
    onConfirm: () => new Promise((resolve) => { finishStop = resolve; }),
  }));
  await show();
  await click(find('[role="dialog"] button', m.remote_stop_confirm_action()));
  assert.equal(find('[role="dialog"] button', m.settings_page_cancel()).disabled, true);
  await press("Escape");
  assert.equal(closed, 0);
  assert.equal(dialogs().length, 1);

  finishStop();
  await settle();
  assert.equal(dialogs().length, 0);
  await show();
  await press("Escape");
  await settle();
  assert.equal(closed, 1);
});

async function sshDialog(save = () => ({ ok: true })) {
  const saves = [];
  let closed = 0;
  let content = "Host original";
  serve(async (path, init) => {
    if (path !== "/api/settings/ssh/config") return;
    if (init.method === "PUT") {
      const body = JSON.parse(init.body);
      saves.push(body);
      const reply = await save();
      if (!(reply instanceof Response) || reply.ok) content = body.content;
      return reply;
    }
    return { path: "/tmp/config", content };
  });
  const { workspace } = await render(null);
  await act(async () => showSshConfigDialog(workspace.dialogs, () => closed++));
  await settle();
  return { workspace, saves, closed: () => closed, editor: () => document.querySelector('[role="dialog"] textarea') };
}

test("SSH saves retain optimistic concurrency and block Escape while saving", async () => {
  let finishSave;
  const ssh = await sshDialog(() => new Promise((resolve) => { finishSave = resolve; }));
  assert.equal(find('[role="dialog"] button', m.common_save()).disabled, true);
  await type(ssh.editor(), "Host edited");
  await click(find('[role="dialog"] button', m.common_save()));
  assert.deepEqual(ssh.saves, [{ content: "Host edited", previousContent: "Host original" }]);
  assert.equal(find('[role="dialog"] button', m.settings_page_cancel()).disabled, true);
  await press("Escape");
  await settle();
  assert.equal(dialogs().length, 1);
  assert.equal(ssh.closed(), 0);

  finishSave({ ok: true });
  await settle();
  const { queryClient, queries } = ssh.workspace;
  assert.deepEqual(queryClient.getQueryData(queries.getSshConfigQuery().queryKey), { path: "/tmp/config", content: "Host edited" });
  await press("Escape");
  await settle();
  assert.equal(ssh.closed(), 1);
  assert.equal(dialogs().length, 0);
});

for (const trigger of ["Escape", "outside click", "cancel button"]) {
  test(`SSH ${trigger} waits for discard consent and retains the draft when declined`, async () => {
    const ssh = await sshDialog();
    await type(ssh.editor(), "edited");
    const dismiss = async () => {
      if (trigger === "cancel button") await click(find('[role="dialog"] button', m.settings_page_cancel()));
      else if (trigger === "Escape") await press("Escape");
      else {
        await act(async () => {
          document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
          document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
          document.body.dispatchEvent(new PointerEvent("pointerup", { bubbles: true }));
          document.body.dispatchEvent(new MouseEvent("click", { bubbles: true }));
        });
      }
      await settle();
    };
    await dismiss();
    assert.equal(dialogs().length, 2);
    assert.match(dialogs()[1].textContent, new RegExp(m.ssh_config_discard_changes()));

    const sshCancel = () => [...dialogs()[0].querySelectorAll("button")].find((button) => button.textContent === m.settings_page_cancel());
    assert.equal(sshCancel().disabled, true);

    await click([...dialogs()[1].querySelectorAll("button")].find((button) => button.textContent === m.common_cancel()));
    assert.equal(sshCancel().disabled, false);
    assert.equal(dialogs().length, 1);
    assert.equal(ssh.editor().value, "edited");
    assert.equal(ssh.closed(), 0);

    await dismiss();
    await click(find("button", m.common_confirm()));
    assert.equal(dialogs().length, 0);
    assert.equal(ssh.closed(), 1);
  });
}

test("returning to a hidden workspace removes dialogs dismissed while it was hidden", async () => {
  const manager = createDialogs();
  const page = (mode) => createElement(Activity, { mode }, createElement(manager.DialogManager));
  const { rerender } = await render(page("visible"));
  let finish;
  const show = (title, onConfirm) => act(async () => manager.showConfirmDialog({ title, onConfirm, onCancel: "close" }));
  await show("Connect", () => new Promise((resolve) => { finish = resolve; }));
  await click(find('[role="dialog"] button', m.common_confirm()));

  await rerender(page("hidden"));
  await act(async () => finish());
  await show("Keep open", () => {});
  await rerender(page("visible"));
  await settle();
  assert.deepEqual(dialogs().map((dialog) => dialog.textContent.includes("Keep open")), [true]);
});
