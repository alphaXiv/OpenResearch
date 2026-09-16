import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { Terminal } from "@xterm/xterm";

/** `dark` is the always-dark log terminal; `app` follows the light/dark theme. */
export type TerminalPalette = "dark" | "app";

const ANSI_NAMES = [
  "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
  "brightBlack", "brightRed", "brightGreen", "brightYellow", "brightBlue", "brightMagenta", "brightCyan", "brightWhite",
] as const;

function readTheme(disableStdin: boolean, palette: TerminalPalette) {
  const root = document.documentElement;
  const rootStyles = getComputedStyle(root);
  const token = (name: string) =>
    rootStyles.getPropertyValue(palette === "app" ? `--term-app-${name}` : `--term-${name}`).trim();
  // xterm's default ANSI colors assume a dark background; the light theme
  // supplies its own so bright prompts stay legible.
  const ansi: Record<string, string> = {};
  if (palette === "app" && root.dataset.theme !== "dark") {
    for (const name of ANSI_NAMES) {
      const value = rootStyles.getPropertyValue(`--term-app-ansi-${name.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}`).trim();
      if (value) ansi[name] = value;
    }
  }
  return {
    ...ansi,
    background: token("bg"),
    foreground: token("foreground"),
    cursor: disableStdin ? token("bg") : token("foreground"),
    selectionBackground: token("selection"),
  };
}

export function mountTerminal(
  wrap: HTMLDivElement,
  disableStdin: boolean,
  enableWebLinks = false,
  palette: TerminalPalette = "dark",
) {
  const rootStyles = getComputedStyle(document.documentElement);
  const terminal = new Terminal({
    convertEol: true,
    disableStdin,
    fontSize: 12,
    fontFamily:
      rootStyles.getPropertyValue("--mono").trim() ||
      "ui-monospace, Menlo, Consolas, monospace",
    scrollback: 20000,
    theme: readTheme(disableStdin, palette),
  });
  // An app-themed terminal must follow a theme toggle while it is open.
  const themeObserver =
    palette === "app"
      ? new MutationObserver(() => {
          terminal.options.theme = readTheme(terminal.options.disableStdin ?? disableStdin, palette);
        })
      : null;
  themeObserver?.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  const fit = new FitAddon();
  terminal.loadAddon(fit);
  if (enableWebLinks) {
    terminal.loadAddon(
      new WebLinksAddon((_event, uri) => {
        let url: URL;
        try {
          url = new URL(uri);
        } catch {
          return;
        }
        if (url.protocol === "http:" || url.protocol === "https:") {
          window.open(url, "_blank", "noopener,noreferrer");
        }
      }),
    );
  }
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
    fit: resize,
    dispose() {
      observer.disconnect();
      themeObserver?.disconnect();
      terminal.dispose();
    },
  };
}
