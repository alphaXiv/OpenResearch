/** Floating panel sizing: keep both the panel and the chat column usable. */
export const PANEL_MIN_WIDTH = 360;
export const PANEL_MARGIN = 10;
export const WORKSPACE_CARD_MIN_WIDTH = 1448; // 1420px content plus the body’s 14px gutters.
// Space the rest of the layout needs beside the panel: the 272px rail, the
// chat column's minimum, and the gutters/margins between the three columns
// (app-body padding 14×2, rail inner margin 14, end-pane inner margin 14).
export const RAIL_WIDTH = 272;
export const CHAT_MIN_SPACE = 380;
export const LAYOUT_CHROME = RAIL_WIDTH + 14 * 4;
// Once a drag pushes the panel past its usable max by this much, it snaps to
// fullscreen — a bit of resistance you have to overcome deliberately.
export const FULLSCREEN_SNAP_SLOP = 80;
// Inward drag needed before snapping back to the last non-fullscreen width.
export const FULLSCREEN_RESTORE_DRAG = 48;

/** The widest the floating panel can be while leaving the rail + chat usable. */
export function panelMaxWidth(): number {
  return Math.max(PANEL_MIN_WIDTH, window.innerWidth - LAYOUT_CHROME - CHAT_MIN_SPACE);
}

export function initialPanelWidth(): number {
  const max = panelMaxWidth();
  return Math.max(PANEL_MIN_WIDTH, Math.min(760, max, Math.round(window.innerWidth * 0.4)));
}
