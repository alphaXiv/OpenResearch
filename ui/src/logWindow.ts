import type { LogIdent, LogPage } from "./api";
import type { RunLogEvent } from "./events";

/** One request's worth of log, and what the viewer shows at once: small enough that wrapped lines stay within xterm's scrollback. */
export const LOG_WINDOW = 256 * 1024;

export interface Span {
  start: number;
  end: number;
}

/** The window starting at `start`, kept inside a log of `len` bytes. */
export function windowAt(start: number, len: number): Span {
  const clamped = Math.max(0, Math.min(start, len - LOG_WINDOW));
  return { start: clamped, end: Math.min(len, clamped + LOG_WINDOW) };
}

export const lastWindow = (len: number) => windowAt(len - LOG_WINDOW, len);

/** Whether a span is on this computer, so reading it costs no trip to the host. */
export function isKept(kept: readonly (readonly [number, number])[], span: Span): boolean {
  return kept.some(([start, end]) => start <= span.start && span.end <= end);
}

const UNITS: Record<string, number> = { b: 1, k: 1024, m: 1024 ** 2, g: 1024 ** 3, t: 1024 ** 4 };

/** A typed position: "63%", "2.5 GB", "512m", or a byte offset. */
export function parsePosition(text: string, len: number): number | null {
  const match = /^\s*(\d+(?:[.,]\d+)?)\s*(%|[bkmgt]i?b?)?\s*$/i.exec(text);
  if (!match) return null;

  const value = Number(match[1].replace(",", "."));
  const unit = match[2]?.toLowerCase();
  const bytes = unit === "%" ? (len * value) / 100 : value * (unit ? UNITS[unit[0]] : 1);
  return Math.min(len, Math.max(0, Math.floor(bytes)));
}

/** Bytes read from mid-log may open inside a UTF-8 character; drop its continuation bytes. */
export function dropPartialChar(bytes: Uint8Array): Uint8Array {
  let skip = 0;
  while (skip < 3 && skip < bytes.length && (bytes[skip] & 0xc0) === 0x80) skip += 1;
  return bytes.subarray(skip);
}

/** A 503 from the log endpoint: the page lives on a host this computer can't reach. */
export class LogPageUnavailable extends Error {
  readonly host: string | null;
  readonly since: number;
  readonly contact: "unreachable" | "needs_auth";

  constructor(host: string | null, since: number, contact: "unreachable" | "needs_auth") {
    super("Page not on this computer");
    this.host = host;
    this.since = since;
    this.contact = contact;
  }
}

export interface UnavailablePage {
  host: string | null;
  since: number;
  contact: "unreachable" | "needs_auth";
  /** The host-recovery epoch the failed request started in. */
  epoch: number;
}

/** Retry an unavailable page once the host is live again, but only if the failure predates its return. */
export function shouldRecover(unavailable: UnavailablePage | null, hostLive: boolean, epoch: number): boolean {
  return hostLive && unavailable !== null && unavailable.epoch < epoch;
}

export interface LogView {
  /** Known log length; null until a page or event reports one. */
  len: number | null;
  kept: [number, number][];
  /** The bytes shown, or being loaded. */
  span: Span;
  following: boolean;
  loading: boolean;
  unavailable: UnavailablePage | null;
  error: string | null;
}

export const INITIAL_LOG_VIEW: LogView = { len: null, kept: [], span: { start: 0, end: 0 }, following: true, loading: true, unavailable: null, error: null };

export interface LogSink {
  reset(): void;
  write(text: string): void;
  /** Called once a fixed window has been written, so it opens on the bytes its position names. */
  scrollToTop(): void;
}

function b64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/**
 * Keeps a sink showing one window of a run's log. Following pins it to the
 * end and appends `run.log` deltas; `goTo` shows a fixed window. Every
 * navigation replaces the sink on its first page with a fresh UTF-8 decoder, so a
 * character split across unrelated windows never joins up.
 */
export function createLogFollower({ fetchPage, sink, publish, epoch, unstableMessage }: {
  unstableMessage: string;
  fetchPage: (span: Span, ident: LogIdent | null) => Promise<LogPage>;
  sink: LogSink;
  publish: (view: LogView) => void;
  /** Stamped on failed reads, for `shouldRecover`. */
  epoch: () => number;
}) {
  let view = INITIAL_LOG_VIEW;
  let disposed = false;
  let ident: LogIdent | null = null;
  let logGeneration: string | undefined;
  // Bumped by every navigation, so a superseded read drops its page.
  let generation = 0;
  let next = 0;
  let decoder = new TextDecoder();
  let partialStart = false;
  let syncing = false;
  let syncAgain = false;
  let pendingClear = false;
  let resets = 0;

  const set = (patch: Partial<LogView>) => {
    view = { ...view, ...patch };
    if (!disposed) publish(view);
  };

  const grow = (end: number) => {
    if (view.len === null || end > view.len) set({ len: end });
  };

  function show(bytes: Uint8Array) {
    if (pendingClear) {
      sink.reset();
      pendingClear = false;
      set({ span: { start: next, end: next } });
    }
    const text = decoder.decode(partialStart ? dropPartialChar(bytes) : bytes, { stream: true });
    partialStart = false;
    if (text) sink.write(text);
  }

  /** Supersedes pending reads; displayed bytes stay until a new page arrives. */
  function restart(start: number) {
    generation += 1;
    decoder = new TextDecoder();
    next = start;
    partialStart = start > 0;
    pendingClear = true;
    return generation;
  }

  async function read(span: Span, token: number) {
    const stamp = epoch();
    try {
      const page = await fetchPage(span, ident);
      if (disposed || token !== generation) return null;
      if (page.kind === "reset") {
        const replaced = ident !== null && page.info !== null;
        ident = null;
        if (replaced) {
          resets += 1;
          if (resets > 2) {
            set({ error: unstableMessage, loading: false });
            return null;
          }
          set({ len: page.info?.len ?? null, kept: [] });
          void follow();
        }
        return null;
      }
      if (page.start !== span.start) return null;
      ident = page.ident;
      const bytes = b64ToBytes(page.data);
      set({ kept: page.kept, unavailable: null, error: null });
      grow(page.len ?? page.start + bytes.length);
      return { bytes, eof: page.eof };
    } catch (err) {
      if (disposed || token !== generation) return null;
      if (err instanceof LogPageUnavailable) set({ unavailable: { host: err.host, since: err.since, contact: err.contact, epoch: stamp } });
      else set({ error: err instanceof Error ? err.message : String(err) });
      return null;
    }
  }

  async function goTo(start: number) {
    const span = windowAt(start, view.len ?? start + LOG_WINDOW);
    const token = restart(span.start);
    set({ following: false, loading: true, unavailable: null, error: null });
    while (next < span.end) {
      const page = await read({ start: next, end: span.end }, token);
      if (token !== generation) return;
      if (!page) break;
      if (page.bytes.length === 0 && !page.eof) break;
      show(page.bytes);
      next += page.bytes.length;
      set({ span: { start: span.start, end: next } });
      if (page.eof) break;
    }
    sink.scrollToTop();
    set({ loading: false });
  }

  /** Reads from `next` to the end, skipping ahead when a window or more behind. */
  async function sync() {
    if (resets > 2) return;
    if (syncing) {
      syncAgain = true;
      return;
    }
    syncing = true;
    let token = generation;
    try {
      for (;;) {
        if (view.len !== null && view.len - next > LOG_WINDOW) {
          const start = lastWindow(view.len).start;
          token = restart(start);
        }
        const page = await read({ start: next, end: next + LOG_WINDOW }, token);
        if (!page || token !== generation) break;
        if (page.bytes.length === 0 && !page.eof) break;
        show(page.bytes);
        next += page.bytes.length;
        set({ span: { start: view.span.start, end: next }, loading: false });
        if (page.eof) break;
      }
    } finally {
      // A superseded loop leaves the flag to the navigation that replaced it.
      if (token === generation && !disposed) {
        syncing = false;
        set({ loading: false });
        if (syncAgain) {
          syncAgain = false;
          void sync();
        }
      }
    }
  }

  async function follow() {
    const start = view.len === null ? 0 : lastWindow(view.len).start;
    restart(start);
    syncing = false;
    syncAgain = false;
    set({ following: true, loading: true, unavailable: null, error: null });
    await sync();
  }

  /** The file was replaced: forget what we knew and follow it from scratch. */
  function replaced() {
    ident = null;
    set({ len: null, kept: [] });
    void follow();
  }

  return {
    goTo: (start: number) => {
      resets = 0;
      void goTo(start);
    },
    follow: () => {
      resets = 0;
      void follow();
    },
    reload: () => {
      resets = 0;
      void (view.following ? follow() : goTo(view.span.start));
    },
    /** After an event-stream gap, deltas may be missing; a following view catches up. */
    onReconnect: () => {
      if (view.following) void sync();
    },
    onLog(event: Pick<RunLogEvent, "dataBase64" | "offset" | "generation">) {
      if (disposed || resets > 2) return;
      if (event.generation !== undefined) {
        const changed = logGeneration !== undefined && event.generation !== logGeneration;
        logGeneration = event.generation;
        if (changed) return replaced();
      }
      const bytes = b64ToBytes(event.dataBase64);
      // An engine run announces its length with an empty delta; a shorter one means a new file.
      if (bytes.length === 0 && view.len !== null && event.offset < view.len) return replaced();

      const end = event.offset + bytes.length;
      grow(end);
      if (!view.following) return;
      if (!syncing && bytes.length > 0 && event.offset === next) {
        show(bytes);
        next = end;
        set({ span: { start: view.span.start, end } });
      } else if (end > next) {
        void sync();
      }
    },
    dispose() {
      disposed = true;
    },
  };
}
