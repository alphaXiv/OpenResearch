---
name: orx-look
description: Make the OpenResearch app look and move right. Covers spacing, alignment, hitboxes, density, scrolling, motion and layout shift, colour and theme tokens, light and dark, icons, logos and chips, popups across machines, and the desktop window on macOS, Windows and Linux. Use whenever you write or change CSS, Tailwind classes, a component's layout, an animation, an icon or a logo in `ui/`, and whenever something looks off, jumps, jitters or scrolls oddly. In this repo, use it in place of any general UI skill, including when reviewing someone else's change.
---

# How OpenResearch looks and moves

## The rules the build enforces

- **Theme tokens only.** Colours, borders and shadows come from `ui/src/theme.css` (`bg-background`, `bg-surface`, `text-text`, `text-subtext`, `text-muted`, `border-border`, `bg-accent-red-subtle`, `shadow-modal`…). Raw colours live only in `theme.css`, and `pnpm lint:styles` rejects `#hex`, `rgb()`, `color-mix()` and arbitrary `text-[…]`, `bg-[…]` and `border-[…]` values anywhere else. A third-party renderer (charts, diagrams, syntax) is themed from the same tokens, never left on its own defaults.
- **Tailwind's own palette is gone** (`--color-*: initial` in `theme.css`): `bg-blue-500` or `text-gray-400` compiles to nothing and no check fails. Every colour class names a token.
- **The theme is resolved by `ui/src/theme.ts`** and set as `data-theme` on the root. Never add a `prefers-color-scheme` override. Judge light and dark separately; a fix that looks right in one is checked in the other.
- **Text sizes are the scale:** `text-sm` is the app's body, `text-menu` (13 px) menus and compact rows, `text-xs` secondary detail; larger sizes are for titles and the onboarding.
- **Start and end, not left and right.** Use `ms-`, `me-`, `ps-`, `pe-`, `start-`, `end-`, `text-start` for anything that would mirror in a right-to-left language.

## The desktop webviews

The desktop app renders in the system's webview: WKWebView on macOS, WebView2 on Windows, WebKitGTK on Linux. WKWebView is whatever WebKit the Mac has, which on a Mac that skipped Safari updates goes back to Safari 14. Chromium in a dev browser forgives the most.

- **Unsupported JavaScript crashes the whole page.** Check new syntax, regex features and APIs against Safari 14.
- **WebKit has its own layout bugs.** Look at a layout change in the desktop app before calling it done (orx-testing).

## Use what exists

- **Dialogs come from the dialog system** (orx-flows); **menus, popovers, selects and tooltips from Base UI** (`@base-ui/react`); **everything else from the primitives in `ui/src/components/ui/`.** A one-off restyle of a primitive is a new variant there or a sign the design is off.
- **Tooltips wrap the control:** `<Tooltip interactive content={…}>`. Without `interactive`, `Tooltip` is its own focusable info icon. `Button`, `IconButton` and `MenuItem` reject `title`, so no native tooltip appears beside the app's.
- **Every machine the user has opened stays mounted** (`ui/src/Machines.tsx`), hidden and `inert` when not shown. A popup portals into its machine's copy, `container={useContext(WorkspaceDomContext).portalContainer}`, as every Base UI portal in `ui/src` does, never into `document.body`.
- **Use the libraries `ui/package.json` already has** before adding one, and keep what you add in proportion to the job.

## Alignment and hitboxes

- **Edges and centres line up across rows and regions,** including against the window's own controls, which share the top row with the app's content on macOS and Windows (`WindowControls`).
- **The hitbox is the whole visible control,** the same in every state the control has. Sizes and contrast are in orx-accessibility.
- **Single pixels count.** Report offsets in pixels, measured from a screenshot at 2×, and fix the cause instead of nudging.
- **After a visual fix, check every instance:** every row, both themes, open and collapsed, hovered and focused, the narrowest window.

## Scrolling and density

- **Nothing scrolls by a few pixels.** A panel that fits shows no scrollbar. Size the container so its content fits, rather than hiding the overflow.
- **Long lists are virtualized** (`@tanstack/react-virtual`, as the transcript, search and CSV viewer are): rows measure without moving what the user is reading, and the scroll position holds when a row above grows.
- **Quiet repeated controls.** One button per item becomes noise in a list; show it on hover or focus, or once for the group.
- **Separate with space before rules.**
- **Nothing floats over what the user is typing or reading,** and an indicator sits on the control it describes.
- **Plan for the edges:** the longest name a user can type, the longest list they can build, the narrowest window, and fullscreen or maximized on each platform. A truncated name keeps the full text in a tooltip.

## Motion

- **Instant or visible, never in between.** Hover and colour take 120 ms, dialogs and popovers 150 ms, with `ease-standard`. Content that changes because of typing or filtering swaps instantly.
- **Nothing jumps on render.** Reserve the final size before content arrives, so a placeholder doesn't give way to something taller.
- **Nothing changes under a drag.** What a selection reveals appears on mouse-up.
- **A refactor keeps every hover, focus and active state.**
- **Motion is optional.** Tailwind motion goes behind `motion-safe:`, as the dialog's error shake does; a CSS animation in `app.css` gets a rule in its `prefers-reduced-motion: reduce` block.

## Colour, icons and logos

- **UI glyphs are `lucide-react`** at the size of the text beside them, in `text-subtext` until hovered or active.
- **Brand marks are the brand's own** (`HarnessLogo`, `BackendLogos`, `LitSourceLogo`): never boxed, tiled or recoloured, except muted to grey where the mark is secondary. A provider gets its own mark; never invent one for a model.
- **References are pills:** an icon and the first words of what they point at, never a raw hash.
- **A choice that changes how the agent works shows its state in the composer,** with an icon and a tooltip, wherever it is set.

## When it looks off

Compare it side by side, at the same scale, with its sibling surface in the app, and name the differences in pixels. For an open call, see orx-flows, "Before you build".
