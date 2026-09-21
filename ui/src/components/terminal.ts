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
    cursorAccent: token("bg"),
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
    cursorBlink: !disableStdin,
    cursorStyle: "bar",
    cursorWidth: 2,
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
  const previousOverflowY = wrap.style.overflowY;
  wrap.style.overflowY = "auto";
  terminal.open(wrap);
  let followOutput = true;
  let outputScrollTop: number | null = null;
  const onPanelScroll = () => {
    if (outputScrollTop === wrap.scrollTop) {
      outputScrollTop = null;
      return;
    }
    outputScrollTop = null;
    followOutput = wrap.scrollTop + wrap.clientHeight >= wrap.scrollHeight - 1;
  };
  wrap.addEventListener("scroll", onPanelScroll);
  const trimEmptyRows = () => {
    const element = terminal.element;
    const screen = element?.querySelector(".xterm-screen");
    const viewport = element?.querySelector(".xterm-viewport");
    if (!element || !(screen instanceof HTMLElement) || !(viewport instanceof HTMLElement)) return;
    const buffer = terminal.buffer.active;
    let rows = Math.max(0, buffer.baseY + buffer.cursorY - buffer.viewportY + 1);
    for (let row = terminal.rows - 1; row >= rows; row--) {
      if (buffer.getLine(buffer.viewportY + row)?.translateToString(true).trim()) {
        rows = row + 1;
        break;
      }
    }
    const screenHeight = screen.getBoundingClientRect().height;
    element.style.height = `${Math.max(wrap.clientHeight, Math.min(rows, terminal.rows) * screenHeight / terminal.rows)}px`;
    element.style.overflow = "hidden";
    // Preserve xterm's full scrollback viewport while clipping unused screen rows.
    viewport.style.height = `${screenHeight}px`;
  };
  const resize = () => {
    try {
      const dimensions = fit.proposeDimensions();
      if (dimensions) {
        // Keep interactive screens usable inside short, scrollable panels.
        terminal.resize(dimensions.cols, Math.max(dimensions.rows, disableStdin ? 1 : 40));
        trimEmptyRows();
      }
    } catch {
      // The container may briefly have zero size while a panel opens or closes.
    }
  };
  resize();
  const observer = new ResizeObserver(resize);
  observer.observe(wrap);
  const output = terminal.onWriteParsed(() => {
    const follow = followOutput && terminal.buffer.active.viewportY === terminal.buffer.active.baseY;
    trimEmptyRows();
    if (follow) {
      const screen = terminal.element?.querySelector(".xterm-screen");
      if (screen instanceof HTMLElement) {
        const cursorBottom = (terminal.buffer.active.cursorY + 1) * screen.getBoundingClientRect().height / terminal.rows;
        wrap.scrollTop = Math.max(0, cursorBottom - wrap.clientHeight);
        outputScrollTop = wrap.scrollTop;
      }
    }
  });
  const scrollback = terminal.onScroll(trimEmptyRows);
  const scrollPanel = (event: WheelEvent) => {
    const canScroll = event.deltaY < 0
      ? wrap.scrollTop > 0
      : wrap.scrollTop + wrap.clientHeight < wrap.scrollHeight;
    if (canScroll) event.stopPropagation();
  };
  wrap.addEventListener("wheel", scrollPanel, { capture: true });

  return {
    terminal,
    fit: resize,
    dispose() {
      observer.disconnect();
      themeObserver?.disconnect();
      wrap.removeEventListener("scroll", onPanelScroll);
      output.dispose();
      scrollback.dispose();
      wrap.removeEventListener("wheel", scrollPanel, { capture: true });
      wrap.style.overflowY = previousOverflowY;
      terminal.dispose();
    },
  };
}
