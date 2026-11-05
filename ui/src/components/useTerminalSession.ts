import { useEffect, useEffectEvent, useRef, type RefObject } from "react";
import type { mountTerminal } from "./terminal";

export function useTerminalSession(
  element: RefObject<HTMLDivElement | null>,
  identity: string,
  open: (element: HTMLDivElement) => ReturnType<typeof mountTerminal>,
) {
  const session = useRef<{
    identity: string;
    mounted: ReturnType<typeof mountTerminal>;
  } | null>(null);
  const create = useEffectEvent(open);

  useEffect(() => {
    const wrap = element.current;
    if (!wrap) return;
    if (session.current?.identity !== identity) {
      session.current?.mounted.dispose();
      session.current = { identity, mounted: create(wrap) };
    }
    const current = session.current;
    return () =>
      queueMicrotask(() => {
        // Activity hides a workspace without ending its shell.
        if (!wrap.isConnected && session.current === current) {
          current.mounted.dispose();
          session.current = null;
        }
      });
  }, [identity, element]);

  return session;
}
