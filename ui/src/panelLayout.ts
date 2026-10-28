export const PANEL_MIN_WIDTH = 360;
const RAIL_WIDTH = 300;
const CHAT_MIN_SPACE = 380;

export function sidebarFitsWithPanel(width: number): boolean {
  return width >= RAIL_WIDTH + CHAT_MIN_SPACE + PANEL_MIN_WIDTH;
}

export function panelMaxWidth(width: number, sidebarVisible: boolean): number {
  const available = width - (sidebarVisible ? RAIL_WIDTH : 0);
  return Math.max(Math.min(PANEL_MIN_WIDTH, Math.floor(available / 2)), available - CHAT_MIN_SPACE);
}

export function sideChatPanelWidth(): number {
  return Math.max(500, Math.round(window.innerWidth * 0.33));
}

export function initialPanelWidth(): number {
  return Math.max(PANEL_MIN_WIDTH, Math.min(480, Math.round(window.innerWidth * 0.25)));
}
