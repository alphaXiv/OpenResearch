import { m } from "../../paraglide/messages.js";
import type { RunModel } from "../../runMonitor/model";
import { Tooltip } from "../ui";
import { Delta } from "./Delta";

export function PendingValue() {
  return <Tooltip content={m.run_value_pending()} className="rounded-xs"><span aria-hidden="true">—</span></Tooltip>;
}

export function HeadlineStat({ model }: { model: RunModel }) {
  const headline = model.headline;
  return (
    <div className="min-w-40 shrink-0">
      <div className="truncate text-xs text-ink-subtext">{headline?.label ?? m.common_loading()}</div>
      <div className="text-xl font-semibold leading-7 tabular-nums">{headline?.known ? headline.value : <PendingValue />}</div>
      <div className="h-4 whitespace-nowrap text-xs">{headline?.delta && <><Delta delta={headline.delta} said />{!headline.delta.flat && <span className="text-ink-subtext"> {m.run_than_parent()}</span>}</>}</div>
    </div>
  );
}
