import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { Terminal } from "@xterm/xterm";

/** `dark` is the always-dark log terminal; `app` follows the light/dark theme. */
export type TerminalPalette = "dark" | "app";

function readTheme(disableStdin: boolean, palette: TerminalPalette) {
  const rootStyles = getComputedStyle(document.documentElement);
  const token = (name: string) =>
    rootStyles.getPropertyValue(palette === "app" ? `--term-app-${name}` : `--term-${name}`).trim();
  return {
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
          terminal.options.theme = readTheme(disableStdin, palette);
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
    dispose() {
      observer.disconnect();
      themeObserver?.disconnect();
      terminal.dispose();
    },
  };
}
