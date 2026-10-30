export const PANEL_MIN_WIDTH = 360;
export const RAIL_DEFAULT_WIDTH = 300;
const RAIL_MIN_WIDTH = 220;
const RAIL_MAX_WIDTH = 480;
const CHAT_MIN_SPACE = 380;

export const clampRailWidth = (width: number) => Math.round(Math.min(Math.max(width, RAIL_MIN_WIDTH), RAIL_MAX_WIDTH));

export function sidebarFitsWithPanel(width: number, railWidth: number): boolean {
  return width >= railWidth + CHAT_MIN_SPACE + PANEL_MIN_WIDTH;
}

/** The widest the sidebar can grow before an open panel would hide it. */
export const railWidthBesidePanel = (width: number) => width - CHAT_MIN_SPACE - PANEL_MIN_WIDTH;

export function panelMaxWidth(width: number, sidebarWidth: number): number {
  const available = width - sidebarWidth;
  return Math.max(Math.min(PANEL_MIN_WIDTH, Math.floor(available / 2)), available - CHAT_MIN_SPACE);
}

export function sideChatPanelWidth(): number {
  return Math.max(500, Math.round(window.innerWidth * 0.33));
}

export function initialPanelWidth(): number {
  return Math.max(PANEL_MIN_WIDTH, Math.min(480, Math.round(window.innerWidth * 0.25)));
}
