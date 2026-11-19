import { ContextMenu } from "@base-ui/react/context-menu";
import { Check } from "lucide-react";
import { useContext, useId, useRef, useState, type KeyboardEvent, type ReactElement, type RefObject } from "react";
import { autoDir, fmtNumber } from "../../../i18n";
import { m } from "../../../paraglide/messages.js";
import type { ChartBlock, ChartEdit, RunMonitor, ViewBlock } from "../../../runMonitor/types";
import { useWorkspace } from "../../../workspace";
import { WorkspaceDomContext } from "../../../WorkspaceDom";
import { cn } from "../../ui/cn";
import { MenuItem, MENU_LABEL, MENU_POPUP, MENU_SEPARATOR } from "../../ui/MenuItem";
import { showAlert } from "../../ui/Toast";
import type { ChartData } from "./series";

function Choices<T extends string | number>({
  label, value, options, reason, busy, onChange,
}: {
  label: string;
  value: T;
  options: { value: T; label: string; disabled?: boolean }[];
  reason?: string;
  busy: boolean;
  onChange: (value: T) => void;
}) {
  const description = useId();
  return (
    <ContextMenu.RadioGroup value={value} aria-describedby={reason ? description : undefined}>
      <ContextMenu.GroupLabel className={cn(MENU_LABEL, "text-ink-subtext")}>{label}</ContextMenu.GroupLabel>
      {options.map((option) => (
        <ContextMenu.RadioItem
          key={option.value}
          value={option.value}
          disabled={busy || option.disabled}
          nativeButton
          render={<MenuItem size="compact" className="data-highlighted:bg-surface" />}
          onClick={() => { if (option.value !== value) onChange(option.value); }}
        >
          {option.label}
          <ContextMenu.RadioItemIndicator><Check size={14} aria-hidden="true" /></ContextMenu.RadioItemIndicator>
        </ContextMenu.RadioItem>
      ))}
      {reason && <p id={description} className="m-0 px-2 py-1 text-xs text-ink-subtext">{reason}</p>}
    </ContextMenu.RadioGroup>
  );
}

const limitMessage = {
  categorical: m.run_chart_log_categories,
  "no-points": m.run_chart_log_no_points,
  "non-positive": m.run_chart_log_non_positive,
};

export function ChartMenu({ monitor, block, limits, trigger, anchor, onOpen }: {
  monitor: RunMonitor;
  block: ChartBlock & Pick<ViewBlock, "phase">;
  limits: ChartData["log"];
  trigger: ReactElement;
  anchor: RefObject<HTMLDivElement | null>;
  onOpen: () => void;
}) {
  const { portalContainer } = useContext(WorkspaceDomContext);
  const workspace = useWorkspace();
  const menu = useRef<ContextMenu.Root.Actions>(null);
  const opened = useRef(false);
  const saving = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function save(edit: ChartEdit) {
    if (saving.current) return;
    saving.current = true;
    setBusy(true);
    setError(null);
    const scope = workspace.workspaceScope();
    try {
      await workspace.api.editRunChart(monitor, monitor.view.blocks.indexOf(block), edit);
      menu.current?.close();
    } catch (cause) {
      if (!workspace.isCurrentScope(scope)) return;
      const reason = cause instanceof Error ? cause.message : String(cause);
      setError(reason);
      if (!opened.current || !anchor.current) {
        showAlert(m.run_chart_save_failed(), "error", { description: reason });
      }
    } finally {
      saving.current = false;
      setBusy(false);
    }
  }

  function keyboardMenu(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key !== "ContextMenu" && !(event.shiftKey && event.key === "F10")) return;
    event.preventDefault();
    event.stopPropagation();
    const { left, top, height } = event.currentTarget.getBoundingClientRect();
    event.currentTarget.dispatchEvent(new MouseEvent("contextmenu", {
      bubbles: true, cancelable: true, button: 2, clientX: left + 8, clientY: top + height / 2,
    }));
  }

  const smoothable = block.from === undefined && (block.mark === undefined || block.mark === "line");
  const levels = [
    { value: 0, label: m.run_chart_smoothing_off() },
    { value: 0.5, label: m.run_chart_smoothing_light() },
    { value: 0.25, label: m.run_chart_smoothing_medium() },
    { value: 0.1, label: m.run_chart_smoothing_strong() },
  ];
  return (
    <ContextMenu.Root actionsRef={menu} onOpenChange={(next) => {
      opened.current = next;
      if (next) onOpen();
    }}>
      <ContextMenu.Trigger
        render={trigger}
        ref={anchor}
        onKeyDown={keyboardMenu}
        aria-haspopup="menu"
        aria-keyshortcuts="Shift+F10 ContextMenu"
      />
      <ContextMenu.Portal container={portalContainer}>
        <ContextMenu.Positioner className="z-100" collisionPadding={8}>
          <ContextMenu.Popup
            finalFocus={anchor}
            aria-label={m.run_chart_menu()}
            className={`${MENU_POPUP} min-w-44 max-w-[min(18rem,var(--available-width))] max-h-(--available-height)`}
          >
            <p className="m-0 px-2 pt-1.5 pb-1 text-xs text-ink-subtext">{m.run_chart_menu_scope()}</p>
            {error && <p role="alert" className="m-0 px-2 py-1 text-xs text-accent-red [overflow-wrap:anywhere]">{m.run_chart_save_failed()} {error}</p>}
            {(["y", "x"] as const).map((axis) => {
              const limit = limits[axis];
              return <Choices
                key={axis}
                label={axis === "y" ? m.run_chart_y_axis() : m.run_chart_x_axis()}
                value={(axis === "y" ? block.yScale : block.xScale) ?? "linear"}
                options={[
                  { value: "linear", label: m.run_chart_scale_linear() },
                  { value: "log", label: m.run_chart_scale_log(), disabled: limit !== null },
                ]}
                reason={limit ? limitMessage[limit]() : undefined}
                busy={busy}
                onChange={(value) => void save({ kind: axis, log: value === "log" })}
              />;
            })}
            <ContextMenu.Separator className={MENU_SEPARATOR} />
            {smoothable ? block.chart.map((signal) => {
              const weight = monitor.view.signals[signal]?.smooth ?? 0;
              const options = levels.some(({ value }) => value === weight)
                ? levels
                : [...levels, { value: weight, label: m.run_chart_smoothing_custom({ weight: fmtNumber(weight) }) }];
              return <Choices
                key={signal}
                label={block.chart.length > 1 ? m.run_chart_smoothing_signal({ signal: autoDir(monitor.view.signals[signal]?.label ?? signal) }) : m.run_chart_smoothing()}
                value={weight}
                options={options}
                busy={busy}
                onChange={(value) => void save({ kind: "smooth", signal, weight: value || null })}
              />;
            }) : <>
              <ContextMenu.Item disabled nativeButton render={<MenuItem size="compact" />}>{m.run_chart_smoothing()}</ContextMenu.Item>
              <p className="m-0 px-2 py-1 text-xs text-ink-subtext">{m.run_chart_smoothing_unavailable()}</p>
            </>}
            {busy && <p role="status" className="m-0 px-2 py-1 text-xs text-ink-subtext">{m.common_saving()}</p>}
          </ContextMenu.Popup>
        </ContextMenu.Positioner>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  );
}
