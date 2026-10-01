// Floating panel sizing: keep both the panel and the chat column usable.
export const PANEL_MIN_WIDTH = 360;
const RAIL_WIDTH = 272;
const CHAT_MIN_SPACE = 380;
// Four 14px gutters: app-body padding ×2, rail inner margin, end-pane inner margin.
const LAYOUT_CHROME = RAIL_WIDTH + 14 * 4;

/** The widest the floating panel can be while leaving the rail + chat usable. */
export function panelMaxWidth(): number {
  return Math.max(PANEL_MIN_WIDTH, window.innerWidth - LAYOUT_CHROME - CHAT_MIN_SPACE);
}

/** Default when no layout is saved. */
export function initialPanelWidth(): number {
  const max = panelMaxWidth();
  return Math.max(PANEL_MIN_WIDTH, Math.min(480, max, Math.round(window.innerWidth * 0.25)));
}
