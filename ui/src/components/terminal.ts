import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";

export function mountTerminal(wrap: HTMLDivElement, disableStdin: boolean) {
  const rootStyles = getComputedStyle(document.documentElement);
  const terminal = new Terminal({
    convertEol: true,
    disableStdin,
    fontSize: 12,
    fontFamily:
      rootStyles.getPropertyValue("--mono").trim() ||
      "ui-monospace, Menlo, Consolas, monospace",
    scrollback: 20000,
    theme: {
      background: rootStyles.getPropertyValue("--term-bg").trim(),
      foreground: rootStyles.getPropertyValue("--term-foreground").trim(),
      cursor: disableStdin
        ? rootStyles.getPropertyValue("--term-bg").trim()
        : rootStyles.getPropertyValue("--term-foreground").trim(),
      selectionBackground: rootStyles.getPropertyValue("--term-selection").trim(),
    },
  });
  const fit = new FitAddon();
  terminal.loadAddon(fit);
  terminal.open(wrap);
  const resize = () => {
    try {
      fit.fit();
    } catch {
      // The container may briefly have zero size while a panel opens or closes.
    }
  };
  resize();
  const observer = new ResizeObserver(resize);
  observer.observe(wrap);

  return {
    terminal,
    dispose() {
      observer.disconnect();
      terminal.dispose();
    },
  };
}
