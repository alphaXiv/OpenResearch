import { type RefObject, useCallback, useEffect, useRef, useState } from "react";

/** Minimal Web Speech API surface. Not in every TypeScript lib.dom, and the
 * vendor-prefixed constructor is still the only one Chrome ships, so the shapes
 * we actually touch are declared here rather than pulled from the DOM lib. */
interface SpeechAlternative {
  readonly transcript: string;
}
interface SpeechResult {
  readonly isFinal: boolean;
  [index: number]: SpeechAlternative;
}
interface SpeechResultList {
  readonly length: number;
  [index: number]: SpeechResult;
}
interface SpeechResultEvent extends Event {
  readonly resultIndex: number;
  readonly results: SpeechResultList;
}
interface SpeechErrorEvent extends Event {
  readonly error: string;
}
interface SpeechRecognizer extends EventTarget {
  lang: string;
  continuous: boolean;
  interimResults: boolean;
  maxAlternatives: number;
  start(): void;
  abort(): void;
  onresult: ((event: SpeechResultEvent) => void) | null;
  onerror: ((event: SpeechErrorEvent) => void) | null;
  onend: (() => void) | null;
}
type SpeechRecognizerCtor = new () => SpeechRecognizer;

function recognizerCtor(): SpeechRecognizerCtor | null {
  if (typeof window === "undefined") return null;
  const scope = window as unknown as {
    SpeechRecognition?: SpeechRecognizerCtor;
    webkitSpeechRecognition?: SpeechRecognizerCtor;
  };
  return scope.SpeechRecognition ?? scope.webkitSpeechRecognition ?? null;
}

/** Errors the user has to act on (grant the mic, come back online); anything
 * else — chiefly `no-speech` and `aborted` — is a normal end of a listening
 * stretch and must not tear dictation down or raise a banner. */
const FATAL_ERRORS = new Set([
  "not-allowed",
  "service-not-allowed",
  "audio-capture",
  "network",
  "language-not-supported",
]);

/** Chrome ends a recognition session on its own after a silence, so staying in
 * "listening" means restarting it. A session that ends this fast having heard
 * nothing is failing rather than idling; three in a row and we stop instead of
 * spinning on start/end forever. */
const RESTART_STRIKE_MS = 400;
const MAX_RESTART_STRIKES = 3;

/** Joins two heard fragments with the space the engine leaves out. Within a
 * session a continuing transcript usually leads with its own space, but the
 * first transcript after an automatic restart does not, and concatenating
 * plainly would run the two words together. */
function joinHeard(left: string, right: string): string {
  if (!left || !right) return left + right;
  return /\s$/.test(left) || /^\s/.test(right) ? left + right : `${left} ${right}`;
}

export interface DictationHandle {
  /** False when the browser has no Web Speech API (Firefox, most non-Chromium). */
  supported: boolean;
  listening: boolean;
  /** Set only for errors the user can act on; cleared on the next start. */
  error: string | null;
  toggle: () => void;
  stop: () => void;
}

export interface DictationOptions {
  /** BCP-47 tag the recognizer listens in, e.g. `en-US`. */
  lang: string;
  /** The field dictation writes into. Its live DOM value is the source of
   * truth, which is what lets typing and speaking interleave. */
  target: RefObject<HTMLTextAreaElement | null>;
  /** Applies a dictated edit: the full new value plus where the caret lands. */
  onChange: (value: string, caret: number) => void;
  /** Maps a recognition error code to a message for the user. */
  describeError: (code: string) => string;
}

/**
 * Dictation into a controlled textarea via the browser's own speech
 * recognition — no key, no backend, no audio through orx.
 *
 * Interim words appear in the field as they are heard and are rewritten in
 * place until the phrase settles. Insertion is anchored where listening began;
 * if the value changes underneath us (the user typed, or the draft was sent),
 * we re-anchor at their caret rather than clobbering the edit.
 */
export function useDictation({ lang, target, onChange, describeError }: DictationOptions): DictationHandle {
  const [supported] = useState(() => recognizerCtor() !== null);
  const [listening, setListening] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const recognizerRef = useRef<SpeechRecognizer | null>(null);
  // Whether the user still wants to listen — `onend` fires for pauses too, so
  // intent, not the recognizer's state, decides whether to restart.
  const wantedRef = useRef(false);
  // Text around the insertion point, captured at the anchor.
  const anchorRef = useRef({ before: "", after: "" });
  // Heard since the anchor, in three parts: settled text from recognition
  // sessions that have already ended, settled text from the live one, and the
  // words still being revised. Their concatenation is what the field shows.
  const committedRef = useRef("");
  const settledRef = useRef("");
  const interimRef = useRef("");
  // First result index of the live session that is still ours to render, and
  // one past the last index we have rendered. A re-anchor hands everything
  // already shown to the user's own text, so revisions of those results are
  // dropped rather than painted a second time.
  const sessionStartRef = useRef(0);
  const renderedEndRef = useRef(0);
  // What the field held when we last looked — anything else in it is the
  // user's own typing.
  const writtenRef = useRef("");
  const caretRef = useRef<number | null>(null);
  const strikesRef = useRef(0);
  const startedAtRef = useRef(0);
  const heardRef = useRef(false);

  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const describeErrorRef = useRef(describeError);
  describeErrorRef.current = describeError;

  // The caret can only be placed once React has committed the new value, so it
  // rides along to the next render rather than being set from the event.
  useEffect(() => {
    const caret = caretRef.current;
    const el = target.current;
    caretRef.current = null;
    if (caret === null || !el || document.activeElement !== el) return;
    el.setSelectionRange(caret, caret);
  });

  /** Fixes where dictated text lands: at the caret, or at the end of the value
   * when the caret cannot be trusted. Everything heard so far is forgotten —
   * whatever the field holds now is the text surrounding the new anchor. */
  const anchorAtCaret = useCallback((atEnd: boolean) => {
    const el = target.current;
    const value = el?.value ?? "";
    const caret = !el || atEnd ? value.length : el.selectionStart ?? value.length;
    anchorRef.current = { before: value.slice(0, caret), after: value.slice(caret) };
    committedRef.current = "";
    settledRef.current = "";
    interimRef.current = "";
    // The baseline for spotting the user's own edits. Seeding it with what the
    // field holds right now (rather than "nothing written yet") is what lets
    // typing survive when it happens before the first result arrives.
    writtenRef.current = value;
  }, [target]);

  /** Hands the field back to the user when it no longer holds what we wrote —
   * they typed, or the draft was replaced. Reported so the caller can drop the
   * results that produced the text now sitting in front of the new anchor. */
  const takeOverEdit = useCallback(() => {
    const el = target.current;
    if (!el || el.value === writtenRef.current) return false;
    anchorAtCaret(false);
    return true;
  }, [anchorAtCaret, target]);

  /** Repaints the dictated span from the anchor. It reads what was heard from
   * the refs rather than taking it as an argument, so a re-anchor earlier in
   * the same event cannot put back words the field already holds. */
  const write = useCallback(() => {
    const el = target.current;
    const { before, after } = anchorRef.current;
    const heard = joinHeard(committedRef.current, joinHeard(settledRef.current, interimRef.current));
    // Dictation continues the sentence it was started in rather than running
    // into the word before it.
    const head = joinHeard(before, heard);
    const value = head + after;
    writtenRef.current = value;
    // Nothing changed: queueing a caret here would let a later, unrelated
    // render (a streaming turn repaints often) drag the caret back to it.
    if (el?.value === value) return;
    caretRef.current = head.length;
    onChangeRef.current(value, head.length);
  }, [target]);

  const teardown = useCallback(() => {
    const recognizer = recognizerRef.current;
    recognizerRef.current = null;
    if (!recognizer) return;
    recognizer.onresult = null;
    recognizer.onerror = null;
    recognizer.onend = null;
    recognizer.abort();
  }, []);

  const stop = useCallback(() => {
    wantedRef.current = false;
    teardown();
    setListening(false);
  }, [teardown]);

  const launch = useCallback(() => {
    const Ctor = recognizerCtor();
    if (!Ctor) return;
    const recognizer = new Ctor();
    recognizerRef.current = recognizer;
    recognizer.lang = lang;
    recognizer.continuous = true;
    recognizer.interimResults = true;
    recognizer.maxAlternatives = 1;
    startedAtRef.current = Date.now();
    heardRef.current = false;
    sessionStartRef.current = 0;
    renderedEndRef.current = 0;

    recognizer.onresult = (event) => {
      heardRef.current = true;
      strikesRef.current = 0;
      // Take the user's edit before this event's words land, so a re-anchor
      // cannot drop the results that arrived with it. Results already on
      // screen became part of their text, so this event's revisions of those
      // are skipped — repainting them would say the phrase twice.
      if (takeOverEdit()) {
        sessionStartRef.current = Math.max(event.resultIndex, renderedEndRef.current);
      }
      let settled = "";
      let interim = "";
      // Rebuilt from the session's whole result list rather than appended to:
      // Safari re-delivers results it already settled, and rebuilding makes
      // that a no-op instead of a phrase that doubles and keeps doubling.
      for (let i = sessionStartRef.current; i < event.results.length; i += 1) {
        const result = event.results[i];
        const transcript = result[0]?.transcript ?? "";
        if (result.isFinal) settled += transcript;
        else interim += transcript;
      }
      renderedEndRef.current = event.results.length;
      settledRef.current = settled;
      interimRef.current = interim;
      write();
    };

    recognizer.onerror = (event) => {
      if (!FATAL_ERRORS.has(event.error)) return;
      setError(describeErrorRef.current(event.error));
      stop();
    };

    recognizer.onend = () => {
      // This session's words are what the field shows, including any the
      // engine never settled; carry them so the next session appends to them.
      committedRef.current = joinHeard(
        committedRef.current,
        joinHeard(settledRef.current, interimRef.current),
      );
      settledRef.current = "";
      interimRef.current = "";
      sessionStartRef.current = 0;
      renderedEndRef.current = 0;
      if (!wantedRef.current) return;
      const quick = Date.now() - startedAtRef.current < RESTART_STRIKE_MS && !heardRef.current;
      strikesRef.current = quick ? strikesRef.current + 1 : 0;
      if (strikesRef.current >= MAX_RESTART_STRIKES) {
        setError(describeErrorRef.current("no-restart"));
        stop();
        return;
      }
      launch();
    };

    try {
      recognizer.start();
    } catch {
      // Chrome throws when the previous session is still winding down. No
      // `onend` follows a start that never happened, so retry on a delay —
      // under the same strike budget as a session that ends instantly.
      strikesRef.current += 1;
      if (strikesRef.current >= MAX_RESTART_STRIKES) {
        setError(describeErrorRef.current("no-restart"));
        stop();
        return;
      }
      window.setTimeout(() => {
        if (wantedRef.current) launch();
      }, RESTART_STRIKE_MS);
    }
  }, [lang, stop, takeOverEdit, write]);

  const start = useCallback(() => {
    if (!supported || wantedRef.current) return;
    setError(null);
    strikesRef.current = 0;
    const el = target.current;
    // A caret only means something once the field has held focus: a draft
    // filled in programmatically reports caret 0 with text already in it, and
    // dictation belongs after that text, not in front of it. The mic button
    // declines focus, so a caret the user placed mid-message survives the
    // click that starts dictation.
    anchorAtCaret(!el || document.activeElement !== el);
    el?.focus();
    wantedRef.current = true;
    setListening(true);
    launch();
  }, [anchorAtCaret, launch, supported, target]);

  const toggle = useCallback(() => {
    if (wantedRef.current) stop();
    else start();
  }, [start, stop]);

  // A language switch mid-sentence needs a fresh recognizer; keep listening.
  useEffect(() => {
    if (!wantedRef.current) return;
    teardown();
    anchorAtCaret(false);
    launch();
    // Restarting on `launch` identity would loop: `lang` is the real trigger.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [lang]);

  useEffect(() => () => {
    wantedRef.current = false;
    teardown();
  }, [teardown]);

  return { supported, listening, error, toggle, stop };
}

/** The recognizer wants a region, the UI locale only carries a language. */
export function speechLocale(locale: string): string {
  switch (locale) {
    case "en":
      return "en-US";
    case "fa":
      return "fa-IR";
    default:
      return locale;
  }
}
