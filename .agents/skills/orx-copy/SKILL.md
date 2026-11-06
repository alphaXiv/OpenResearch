---
name: orx-copy
description: Write and review the words a user reads in the OpenResearch app: labels, buttons, menu items, dialog titles and descriptions, onboarding and setup steps, empty states, errors, refusals, warnings, toasts, tooltips, placeholders, status lines and counts. Also adding them to them to the Paraglide catalogs. Read this before typing any user-visible string in `ui/`, including a one-word label, and before rewriting existing copy. Pairs with orx-flows, which decides what a screen asks for. In this repo, use it in place of any general UI skill, including when reviewing someone else's change.
---

# Copy in OpenResearch

## Writing a string

1. **Name the slot** and take its budget:

   | Slot | Budget |
   | --- | --- |
   | button, menu item | 1–3 words, verb first: "Add machine", "Try again" |
   | dialog or step title | 3–6 words, no final period |
   | description, empty state, error | 1–2 short sentences |
   | tooltip | a few words naming the action; never the only place something vital is said |
   | toast | one line |
   | status, badge | 1–2 words: "Connected", "Needs sign-in" |

   Leave room for translations, which often run longer than English.
2. **Write the shortest true version** in the user's terms.
3. **Run the checks.**
   - **Can the reader act on it?** Cut what they can't use.
   - **Would someone with no context know what it means?** Name the thing and its effect, never a word that only makes sense to whoever built the feature. Write "Couldn't reach your cluster", not "SSH master exited with status 255".
   - **Would a stranger know why they should care?** (below)
   - **Is every fact on screen true?** A timestamp labelled "started" is when it started.

## Making the case

orx-flows lists what a request must answer. The words:

- **Lead with what they get,** in OpenResearch, today, in terms of their own work. Never a feature list of the other service.
- **Answer the newcomer who has none yet:** that they can get one, how, and how long it takes.
- **Name the cost plainly:** time, money, and what is read or sent.
- **Never oversell.** No "unlock", "supercharge", "seamless" or exclamation marks.

## Errors, warnings and refusals

- **What happened, then what to do:** "A project with that name already exists. Choose another name."
- **Give the cause when it changes what they do next,** in their terms. Never invent one: if the cause is unknown, say what didn't work and offer a retry.
- **Name exactly what's wrong, how to see it, and the safe fix.** A refusal names each offending thing (every path, not "a path"), gives a command the user can paste to inspect it, and says what to change and what to leave alone. Each cause gets its own message, never one generic line for several.
- **Every refusal has an exit:** a button, a link to the setting, or the next step. A "Choose another machine" button beats a bare "Machine unavailable".
- **No blame, no apology, no jokes:** never "you forgot", "invalid", "oops" or "sorry".
- **Raw errors go in details.** A disclosure ("Error details") holds the stderr or status code; the line above it is human.

## Empty states, loading and status

- **Empty:** say what's missing and how it will appear, then offer the action that fills it, its label answering the title ("No machines yet." → "Add a machine"). When something else fills it, like an agent, say that instead of offering a button.
- **Loading:** one present participle and an ellipsis, "Checking the connection…". Narrate stages only when the wait is long and the stages differ.
- **Counts handle zero and one.** Plurals are `_one`/`_other` keys chosen with `Intl.PluralRules(getLocale())`. Zero usually needs its own sentence or no line at all: never "0 files". Languages with more plural forms than English (ru, pl, ar) still get only the two keys, so translate both into a form that is right for every count, such as a label and the number ("Файлы: {count}").
- **Don't narrate what the screen already shows.** No toast for a change that is visible where the user is looking.

## Labels, tooltips and placeholders

- **Name things the way users do:** a model by the name its provider gives it, a machine by the host alias the user typed.
- **Compact labels drop what the context already says,** like "your" on a pill or a badge.
- **One term per concept,** the same on every screen. Check the new word against the nouns already in `ui/messages/en.json`.
- **A placeholder is an example, never the label or a rule.** The label stays visible.
- **An icon-only control has a tooltip and an accessible name** naming the action, both from one message: `<Tooltip interactive content={m.x()}><IconButton aria-label={m.x()} …/></Tooltip>`. `Button` and `IconButton` reject `title`.
- **A shortcut in text is filled in with `formatForDisplay(appHotkeys[id].hotkey ?? [])`,** so it reads ⌘ on macOS and Ctrl elsewhere. Never type ⌘ or Ctrl into a catalog.

## Conventions

- **Sentence case** everywhere: "Choose a coding agent", not "Choose a Coding Agent".
- **The ellipsis is one character (…)**, never three periods. A label ends in one when the action needs more input before it happens ("Rename…").
- **No em dashes.** One is the sign of a clause written wrong, so rewrite the whole phrase; swapping the dash for a comma or a period leaves the same clause.
- **Product names keep their own spelling:** OpenResearch, alphaXiv, Claude Code, Codex, OpenCode, GitHub, Slurm. The app is "OpenResearch"; `orx` appears only where the user would type the command.
- **Straight apostrophes and quotes** (`'`, `"`), never curly ones.
- **No exclamation marks, no emoji.**

## Paraglide

- **Every user-visible string is a message:** `m.key()` from `ui/src/paraglide/messages.js`, never a literal in JSX, including `aria-label` and tooltip text.
- **Keys name the place and the gist,** in the existing style: `onboarding_install_git`, `settings_page_…`, `chat_…`. Reuse `common_cancel`, `common_confirm` and the other `common_` keys rather than making copies.
- **Wrap every interpolated value** so it can't reorder the sentence around it: `ltr(value)` for paths, IPs, commands and IDs, `autoDir(value)` for things users name, like a project, chat or host (`ui/src/i18n.ts`). No lint checks this.
- **Format through the locale:** `fmtNumber`, `fmtClock` and `fmtBytes` from `ui/src/i18n.ts`, and `new Intl.ListFormat(getLocale())` to join a list, never `", "` and "and".
- **Add each key to every catalog in `ui/messages/`,** translated, never English pasted into the others. Keep `{placeholders}` identical in each. `pnpm lint:i18n` fails on a missing key, a mismatched placeholder or an HTML entity.
- **Rewording a string rewrites it in every catalog.** If the meaning changes, rename the key so a stale translation can't survive.
- **A new catalog, or a large batch of keys, gets a review pass in that language** before it ships.

## Reviewing copy

Watch for:

- a fact followed by its justification (", so", an appositive) in the voice of a design document;
- non-events ("Nothing was deleted.");
- reassurance about a loss the user can already see;
- a second sentence that restates the first;
- a subsystem blamed by name ("The engine has not reported…").

Leave alone: bare labels ("Copy", "Running", "Failed"), gerunds with an ellipsis for waits, and a detail line carrying a real gotcha the user can't see.
