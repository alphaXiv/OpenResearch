---
name: orx-flows
description: Decide how a path through the OpenResearch app behaves before building it. Covers the onboarding and every other setup window, asking a user to sign in, connect, install or grant something, setup for agents, models and compute backends, dialogs and confirmations, settings, menus, opening things, empty, loading and error states, and messages from agents and the system in a chat. Use whenever you add or change a screen, dialog, setting, menu item, status, banner or any step a user takes in `ui/`, even a single button, and before redesigning one. Pairs with orx-copy for the words and orx-testing to prove it. In this repo, use it in place of any general UI skill, including when reviewing someone else's change.
---

# Flows in OpenResearch

## Before you build

1. **Name the user and the job.** OpenResearch's users are researchers who already use a coding agent. Most have never heard of a harness, ACP, `orx-host`, a worktree or the run engine. Write the job in their words ("run my training script on the lab's cluster"), never as the mechanism.
2. **Find the sibling and copy it.** Nearly every new surface already has one. Copy its shape, and give any difference a reason the user would notice. A copy is never worse than its sibling, even where the sibling is flawed, and a new item joins every list that holds its siblings, last unless the list sorts.

   | Adding | Copy |
   | --- | --- |
   | a compute backend | another backend's `TargetTile`, `QuickSetupDialog` and `BackendDetailPage` in `ui/src/components/SettingsPage.tsx` |
   | an agent or model provider | `HarnessSetupDialog`, `AcpAuthDialog`, `LocalModelSetup` |
   | a question, a name, a confirmation | the dialog system (below) |
   | a setup step | the nearest setup of the same kind: `ui/src/components/Onboarding.tsx` for the first run, the backend and agent dialogs above for the rest |
   | a status on an object | `StatusIndicator`, `Badge`, `StatusBadge` |

3. **Walk it as a stranger**, from the entry point (a tile, a menu item, a setup step) to the result. At each step write down the first question a newcomer would have. The screen answers it, or the step goes.
4. **Every surface it reaches** is every place that shows the same thing or offers the same action. Find them by searching the code for the component, query or route the change touches, then add every kind of chat, the browser and each desktop webview, and a remote machine.
5. **When the right look or flow is unclear,** build one version modelled on the nearest existing screen and name the alternative in the PR. Following the app is the default; improving the app's own patterns is separate work.

## The onboarding and other setup

**The onboarding** is the few steps a new user sees on first opening the app (`ui/src/components/Onboarding.tsx`): what the first chat needs, then what is optional. A new step there needs the maintainers' yes. Every other setup window (a backend, an agent, a model, a remote machine) is an onboarding too, and follows the same rules.

- **Ask when it's needed.** Anything the first chat can start without waits for the moment it applies, and makes its case there.
- **A required step blocks; an optional one has Skip.** Never make an optional step look required.
- **Warn instead of blocking,** once per flow, at the action the warning concerns, and only when it applies.
- **Back, a header click or closing the window mid-step** lands on a step that still makes sense, never a half-finished state.
- **Setup that belongs to one machine says which.** Agents and their sign-ins are per machine, so a remote machine skips the first run but still sets up its own, and says so.

## Asking for something

Every sign-in, connection, install, permission or payment answers these before the user thinks to ask them:

- **What is it?** In a few words, for someone who has never heard of it.
- **Why would I want it?** What it does for them in OpenResearch, today.
- **I don't have one. Now what?** How to get one, and how long it takes.
- **What does it cost?** Time, money, and what OpenResearch will read or send.
- **Can I skip it?** Yes or no, and what skipping loses.

A step that names the thing and offers a button answers none of them. orx-copy has the words.

## Dialogs

**There is one dialog system:** `useWorkspace().dialogs` from `ui/src/dialog.tsx`. Never a raw `<Dialog>` for a question, form or flow, nor a native `<dialog>`, `window.confirm`, `alert` or `prompt`. A full-screen viewer or a drawer is the exception.

| Need | Call |
| --- | --- |
| yes or no | `showConfirmDialog({ title, description, confirmLabel, cancelLabel, destructive, onConfirm, onCancel })` |
| one value | `showTextDialog({ …, initialValue, validateInput })` |
| arbitrary content that closes itself | `showFormDialog(title, render)` |
| a form or a multi-step flow | `showCustomDialog({ component, props, onConfirm, onCancel })`; first check whether a wrapper already fits |

- **Do the work in `onConfirm`.** While it runs, the dialog disables itself and shows a spinner. A throw keeps it open, shakes it and shows the error's message, so throw an error the user can read instead of closing and toasting.
- **`onCancel: "disable"` makes a dialog that can't be dismissed:** Escape and clicks outside do nothing. Use it when the user must answer, or when closing by accident would lose input. On `showConfirmDialog` and `showTextDialog` it also removes Cancel and labels the button OK, so there it is an acknowledgement. A custom dialog under `"disable"` can still render its own button that calls the `cancel` prop.
- **Fit the dialog to what it shows.** Its height follows its content, and every step asks for something.
- **Confirm only what can't be undone or would lose work:** deleting, abandoning a run, quitting with work running, overwriting. Name the consequence in the description. Routine, reversible actions get no dialog.
- **Buttons name the action** ("Quit", "Delete project"), never OK when a verb fits. `destructive: true` gives a destructive action the danger style. When backing out is the safe choice, `cancelLabel` says what backing out keeps.
- **Offer the fewest choices that do the job.**
- **Say what leaves the machine before it leaves.** Publishing or sharing names what goes and where, in the dialog, before anything is sent.
- **An editor guards unsaved edits on close.**

## Setup flows

- **Setup opens where it's asked for.** Whatever offers it (a tile, a menu item, an error's button) opens it in place, with its fields, its limits and a test that proves it works. Never a page that says to go set it up somewhere else.
- **No terminal where a form or a progress bar works.** A wait shows progress, anything the user would type or paste back gets a field, and the raw output stays collapsed beneath for when the happy path fails.
- **Copy never promises UI that isn't there.**
- **Users pick models, not harnesses.** What every row in a group shares goes once in the group's heading, never repeated on each row.
- **Stay in the app.** An in-app view beats a link that opens a browser tab.

## Feedback, empty and error states

- **Respond within a frame.** A window opens at once and shows its progress (`Spinner`, `LoadingRow`, `ProgressBar`).
- **No dead ends.** Every page a user can reach lets them do something there. A page with nothing to show says what will appear and offers the action that fills it.
- **Say why nothing happened.** A button that can't act now is disabled with the reason nearby, or acts and explains. It never silently does nothing.
- **Errors live on the object,** next to the chat, run, host or field they concern, with the way forward. `showAlert` toasts are for events with no home; warnings and errors stay until closed, so keep them for what the user must see.
- **Nothing disappears silently.** Blocked or unrenderable content leaves a visible trace: alt text, a plain link, "couldn't render". When a turn, a queued message or a run can't finish, show why and offer the next step.
- **A view shows changes made elsewhere as they happen,** by the CLI, an agent or another window, and never overwrites what the user is typing with them (arc/dashboard.md, "Live state").
- **A reload or restart mid-action reopens on where that action stands,** read back from the server, never on a blank form, a lost draft or a spinner with nothing behind it.
- **Never show success over a degraded state,** and never the previous item's data under the next item's name. A view that has lost its event stream says so (`OfflineBanner`).

## Opening things

- **Anything that names something opens it:** a sub-agent's message opens that sub-agent, a run card its logs, a reference what it points at.
- **Open through `tabOpenGestureHandlers`** (`ui/src/tabPreview.ts`): a click previews in the reusable preview tab, and a double-click or Enter keeps it open, as files, runs and artifacts already do.

## Chats

- **Messages from orx and other agents go through `AgentMessageCard`.** A new kind of message extends it rather than drawing its own bubble.
- **`@` adds context of any kind** (files, papers, chats). What it adds is a chip that carries the reference as data: never paste its text, and never match a pattern back out of text.
- **A tool in the chat stays out of the way until it's used,** and never slows a long chat down.
- **A long transcript stays smooth.** Only the visible window of messages is mounted, and nothing jumps when one grows.

## Settings and menus

- **Every setting has a visible effect,** and its control fits the choice: a `Switch` for on and off, `SEGMENTED_CLASS_NAME` (`ui/src/components/ui/Segmented.ts`) for a few exclusive options, a menu for long lists.
- **A setting that implies an action takes it at once.**
- **A control appears only where it does something,** and a remote workspace hides local-only settings rather than failing on them.
- **Context menus hold only what applies to the thing clicked.** Creation first, destruction last. Disable an item that would do nothing, rather than letting it end in an error.

## Hide the machinery

Users see projects, chats, experiments, runs, papers and machines. Harnesses, ACP, `orx-host`, worktrees, leases, descriptors and the engine stay out of view, unless one tells two things apart or is the cause of an error the user has to act on.

## Names and voice

Names, the onboarding's voice and public-facing prose are the maintainers' call. Propose them with options.
