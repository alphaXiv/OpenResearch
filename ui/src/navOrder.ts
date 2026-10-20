import { useState, type DragEvent } from "react";

/** `ids` in the saved order. Ids the saved order doesn't know (added since it
 * was saved) keep their default place relative to their neighbours; saved ids
 * that no longer exist are dropped. */
export function applyNavOrder<T extends string>(ids: readonly T[], saved: readonly string[]): T[] {
  const known = saved.filter((id): id is T => ids.includes(id as T));
  const result = [...known];
  ids.forEach((id, index) => {
    if (result.includes(id)) return;
    // Insert after the nearest default-order predecessor that is already placed.
    const before = ids.slice(0, index).reverse().find((previous) => result.includes(previous));
    result.splice(before === undefined ? 0 : result.indexOf(before) + 1, 0, id);
  });
  return result;
}

/** `order` with `moved` placed where `target` is. */
export function moveNavItem<T extends string>(order: readonly T[], moved: T, target: T): T[] {
  const from = order.indexOf(moved);
  const to = order.indexOf(target);
  if (from < 0 || to < 0 || from === to) return [...order];
  const next = [...order];
  next.splice(from, 1);
  next.splice(to, 0, moved);
  return next;
}

function readOrder(key: string): string[] {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(key) ?? "[]");
    return Array.isArray(parsed) ? parsed.filter((id): id is string => typeof id === "string") : [];
  } catch {
    return [];
  }
}

/** A nav list the user reorders by dragging; the order is kept per browser
 * under `storageKey`. Spread `itemProps(id)` on each item. */
export function useNavOrder<T extends string>(storageKey: string, ids: readonly T[]) {
  const [saved, setSaved] = useState<string[]>(() => readOrder(storageKey));
  const [dragging, setDragging] = useState<T | null>(null);
  const order = applyNavOrder(ids, saved);

  const save = (next: T[]) => {
    setSaved(next);
    try { localStorage.setItem(storageKey, JSON.stringify(next)); } catch {}
  };

  const itemProps = (id: T) => ({
    draggable: true,
    "data-dragging": dragging === id ? "" : undefined,
    onDragStart: (event: DragEvent) => {
      event.dataTransfer.effectAllowed = "move";
      event.dataTransfer.setData("text/plain", id);
      setDragging(id);
    },
    onDragOver: (event: DragEvent) => {
      if (dragging === null) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = "move";
      if (dragging !== id) save(moveNavItem(order, dragging, id));
    },
    onDrop: (event: DragEvent) => event.preventDefault(),
    onDragEnd: () => setDragging(null),
  });

  const reset = () => save([...ids]);
  const customized = order.some((id, index) => id !== ids[index]);

  return { order, itemProps, reset, customized };
}
