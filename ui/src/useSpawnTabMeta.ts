import { useEffect, useState } from "react";

import { queryClient } from "./queries/client";
import { getChatMessagesQuery } from "./queries/chat";
import { findPartById, spawnRowTitle } from "./components/ChatPanel";
import { onChatEvent } from "./events";
import { type SubagentViewDef } from "./workspaceTabs";
import { type ChatMessage } from "./api";

/** Live title + running state for open sub-agent tabs, straight off the spawn
 * parts' message stream — so a tab is named for its task and shimmers while
 * the agent still works (the open-time `label` is only the seed/fallback). */
export function useSpawnTabMeta(subagentTabs: SubagentViewDef[]) {
  const [spawnMeta, setSpawnMeta] = useState<Record<string, { label: string; running: boolean }>>({});
  useEffect(() => {
    // Closed tabs drop their metadata — the map only ever holds open tabs.
    setSpawnMeta((prev) => {
      const open = new Set(subagentTabs.map((t) => t.spawnPartId));
      if (Object.keys(prev).every((id) => open.has(id))) return prev;
      return Object.fromEntries(Object.entries(prev).filter(([id]) => open.has(id)));
    });
    if (subagentTabs.length === 0) return;
    let live = true;
    // Spawn ids a live event already updated: the initial fetch can resolve
    // AFTER newer stream frames and must not roll those tabs back (a stale
    // `running` snapshot would shimmer forever).
    const liveUpdated = new Set<string>();
    const apply = (msgs: ChatMessage[], tabs: SubagentViewDef[], fromSeed: boolean) => {
      setSpawnMeta((prev) => {
        let next = prev;
        for (const t of tabs) {
          if (fromSeed && liveUpdated.has(t.spawnPartId)) continue;
          for (const m of msgs) {
            const part = findPartById(m.parts, t.spawnPartId);
            if (!part) continue;
            if (!fromSeed) liveUpdated.add(t.spawnPartId);
            const meta = { label: spawnRowTitle(part), running: part.state?.status === "running" };
            const cur = next[t.spawnPartId];
            if (!cur || cur.label !== meta.label || cur.running !== meta.running) {
              if (next === prev) next = { ...prev };
              next[t.spawnPartId] = meta;
            }
            break;
          }
        }
        return next;
      });
    };
    // Generation token: a reconnect starts fresh seeds, and a stale in-flight
    // response from an earlier generation must not land after them.
    let seedGen = 0;
    const seed = () => {
      const gen = ++seedGen;
      for (const sid of new Set(subagentTabs.map((t) => t.sessionId))) {
        queryClient.fetchQuery({ ...getChatMessagesQuery(sid), staleTime: 0 })
          .then(({ messages }) => {
            if (live && gen === seedGen)
              apply(messages, subagentTabs.filter((t) => t.sessionId === sid), true);
          })
          .catch(() => {});
      }
    };
    seed();
    const off = onChatEvent((ev) => {
      if (ev.type === "reconnected") {
        // Frames lost during the outage may include the terminal update —
        // refetch, letting the fresh seed overwrite everything.
        liveUpdated.clear();
        seed();
        return;
      }
      if (ev.type !== "message") return;
      const tabs = subagentTabs.filter((t) => t.sessionId === ev.sessionId);
      if (tabs.length) apply([ev.message], tabs, false);
    });
    return () => {
      live = false;
      off();
    };
  }, [subagentTabs]);

  return spawnMeta;
}
