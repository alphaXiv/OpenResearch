---
name: orx-accessibility
description: Make the OpenResearch app work by keyboard and assistive technology, and prove it without turning a screen reader on. Covers roles, names and states, ids, focus and Escape, keyboard shortcuts and the app menu, dialogs, menus and popovers through Base UI, live regions for status, long transcripts, reduced motion, contrast in both themes and on translucent surfaces, target sizes, zoom, and tests that assert on the accessibility tree. Use whenever you add or change a control, popup, dialog, menu, list, shortcut or focus behaviour in `ui/`, and whenever someone mentions screen readers, VoiceOver, NVDA, ARIA, keyboard navigation or WCAG. In this repo, use it in place of any general accessibility skill, including when reviewing someone else's change.
---

# Accessibility in OpenResearch

## The tree is the test

Assert on roles, names, states, focus and live regions. Never turn VoiceOver, Narrator, NVDA or Orca on in the user's session or ask the user what they heard; a listening pass happens in a VM, when the user asks for one.

- **In unit tests,** mount with `render` or `renderApp` from `ui/tests/render.mjs` and find things by what a screen reader exposes: ``find(`button[aria-label="${m.key()}"]`)``, `[role=dialog]`, `[aria-expanded=true]`. `find` matches text content, so an icon button is found by its label. After `press("Escape")`, assert where `document.activeElement` is.
- **In `e2e/`,** use Playwright's `getByRole`, as the specs in `e2e/tests/` do.
- **Accessibility must not cost performance.** The transcript mounts only its visible window, so it is an ARIA feed (`role="feed"`, `aria-setsize`, `aria-posinset`, `aria-busy`; `ui/src/components/ChatPanel.tsx`) rather than every message in the DOM. A long list follows the same pattern.

## Use the parts that already handle it

- **Dialogs come from `useWorkspace().dialogs`** (orx-flows). Base UI moves focus in, traps it, and returns it on close; `DialogTitle` and `DialogDescription` name and describe the dialog. A dialog with no visible title takes `aria-label`.
- **Menus, popovers and selects use Base UI** (`@base-ui/react`), which carries their roles, arrow keys and focus return. Never build one from `div`s and click handlers.
- **Buttons are `Button` and `IconButton`.** An `IconButton` takes an `aria-label` from a Paraglide message naming the action, inside `<Tooltip interactive>` with the same message. A `Tooltip` without `interactive` is a focusable `role="img"` info icon: right for a lone hint glyph, wrong around a control.
- **Semantic elements first:** `button`, `a href`, `label`, headings, lists. ARIA only where no element fits.
- **Never hard-code an `id`.** Every machine the user has opened stays mounted, so `aria-labelledby`, `aria-describedby` and `htmlFor` point at `useId()` or `useContext(WorkspaceDomContext).id(name)`.

## Keyboard and focus

- **Every action has a keyboard path.** Tab reaches every control in visual order, including chips and pills inside an editor; Enter and Space press; Escape closes the innermost popup and nothing else.
- **Escape is scoped.** Wrap a surface that handles Escape in `EscapeScope` (`ui/src/components/EscapeScope.tsx`), which gives it the key exclusively while it is open.
- **App shortcuts live in `appHotkeys`** (`ui/src/hotkeys.ts`), each named by a message. `ui/src/appMenu.ts` builds native menu items from those entries with `command(appHotkeys[id])`, added there by hand. Check a new chord against the existing ones and the ones each platform reserves, such as quit, close window and settings.
- **Focus is visible.** Primitives draw the ring; a custom focusable copies theirs: `focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:outline-offset-2`. The ring wraps the control itself, never the row around it.
- **Focus goes where the next action is.** A surface opened to be typed into focuses its input, as a new chat focuses the composer (`useComposerFocus`), and a closed popup or dialog returns focus to its trigger (Base UI). Follow the nearest existing mover; add a new one only with a reason.

## Names, states and announcements

- **A name is what a sighted user reads,** unique among its siblings, and it names the action, not the icon ("Close tab", never "X").
- **Expose every state the control has:** `aria-pressed` for toggles, `aria-expanded` and `aria-haspopup` for anything that opens, `aria-selected` and `aria-current` for the current item, `disabled` with the reason visible nearby.
- **Status that changes without focus moving is announced** through a live region (`role="status"`, or `role="alert"` for errors): a turn finishing, a run failing, a copy confirming. Drawn but silent is a bug.
- **Don't announce noise.** Streaming tokens and log lines don't go into a live region; their completion does.

## Visual floor

- **Contrast:** 4.5:1 for text, 3:1 for icons, control edges and focus rings, in light and dark, measured against what is actually behind the element: a translucent or blurred surface, a selected or hovered row, an image.
- **Targets are at least 24×24 px** (`IconButton` small is 28).
- **Motion is optional:** Tailwind motion goes behind `motion-safe:`, and a CSS animation in `app.css` gets a rule in its `prefers-reduced-motion: reduce` block, so it becomes an instant change.
- **Zoom works.** The desktop app zooms with its zoom shortcuts in `appHotkeys`; check the change at 150% and 200% for clipping and overlap.
- **Colour is never the only signal.** A failed run is red and says "Failed".

## Checklist

- [ ] role fits what it does, from a semantic element or a Base UI part
- [ ] name is a Paraglide message naming the action, unique among siblings
- [ ] every state the control has is exposed; ids come from `useId()` or `WorkspaceDomContext`
- [ ] reachable by Tab in visual order; focus ring wraps the control; Escape backs out of the innermost popup
- [ ] a popup takes focus and gives it back
- [ ] status changes are announced; streaming text is not
- [ ] contrast holds in both themes against what is behind it; targets are 24 px or more
- [ ] motion is optional; zoom to 200% doesn't clip
- [ ] a test asserts on the role and name of what changed
