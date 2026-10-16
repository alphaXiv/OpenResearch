import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { CheckCircle2, CircleX, Stethoscope } from "lucide-react";
import { m } from "../../paraglide/messages.js";
import { getComputeSettingsQuery } from "../../queries/settings";
import { TARGET_LABELS } from "../../computeTargets";
import { BackendLogo } from "../../components/BackendLogos";
import { SETTINGS_CARD_CLASS_NAME, TARGET_KIND } from "../../components/SettingsPage";
import { Badge, Button, Spinner } from "../../components/ui";
import { cn } from "../../components/ui/cn";
import { diagnoseCompute } from "./api";
import type { ComputeTargetId } from "../../api";

export function ComputeDoctor() {
  const settings = useQuery(getComputeSettingsQuery());
  const [backend, setBackend] = useState<ComputeTargetId>("colab");
  const probe = useMutation({ mutationFn: diagnoseCompute });
  const targets = settings.data?.targets ?? [{ id: "colab" as const }];
  return (
    <section className={cn(SETTINGS_CARD_CLASS_NAME, "mb-8")}>
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2 text-base font-medium text-text">
            <Stethoscope size={16} className="text-primary shrink-0" aria-hidden="true" />
            <span>{m.research_doctor_title()}</span>
          </div>
          <p className="mt-1 mb-0 text-sm text-subtext">{m.research_doctor_hint()}</p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <div className="flex items-center gap-2 rounded-md border border-border bg-surface px-2.5 py-1">
            <BackendLogo kind={TARGET_KIND[backend] ?? "local"} size={16} />
            <select
              aria-label={m.research_backend()}
              value={backend}
              disabled={probe.isPending}
              className="cursor-pointer border-0 bg-transparent py-0.5 pe-6 text-sm text-text focus:outline-none"
              onChange={event => {
                setBackend(event.target.value as ComputeTargetId);
                probe.reset();
              }}
            >
              {targets.map(target => (
                <option key={target.id} value={target.id}>{TARGET_LABELS[target.id]()}</option>
              ))}
            </select>
          </div>
          <Button variant="primary" disabled={probe.isPending} onClick={() => probe.mutate(backend)}>
            {probe.isPending ? <Spinner /> : <Stethoscope size={14} />}
            {m.research_diagnose()}
          </Button>
        </div>
      </div>
      {probe.error && (
        <p role="alert" className="mt-3 mb-0 rounded-md border border-danger-notice-border bg-accent-red-subtle px-3 py-2 text-sm text-accent-red">
          {probe.error.message}
        </p>
      )}
      {probe.data && (
        <div aria-live="polite" className="mt-4 border-t border-border-variant pt-3.5 text-sm">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <Badge variant={probe.data.ready ? "success" : "warning"}>
              {probe.data.ready ? m.research_check_passed() : m.research_check_failed()}
            </Badge>
            <span className="break-all font-mono text-xs text-subtext">
              orx {probe.data.version} · {probe.data.buildChannel} · {probe.data.executable}
            </span>
          </div>
          <ul className="mt-3 mb-0 flex list-none flex-col gap-2 p-0">
            {probe.data.checks.map((check, index) => {
              const passed = check.status === "pass";
              return (
                <li
                  key={`${check.id}-${index}`}
                  className={cn(
                    "rounded-md border px-3 py-2",
                    passed ? "border-border-variant bg-surface" : "border-danger-notice-border bg-accent-red-subtle",
                  )}
                >
                  <div className="flex items-start gap-2">
                    {passed ? (
                      <CheckCircle2 size={15} className="mt-0.5 shrink-0 text-accent-green" aria-hidden="true" />
                    ) : (
                      <CircleX size={15} className="mt-0.5 shrink-0 text-accent-red" aria-hidden="true" />
                    )}
                    <div className="min-w-0 flex-1">
                      <div className="font-medium text-text">{check.message}</div>
                      {check.action && (
                        <pre className="mt-1.5 mb-0 overflow-x-auto rounded border border-border bg-background px-2.5 py-1.5 font-mono text-xs whitespace-pre-wrap text-subtext">
                          {check.action}
                        </pre>
                      )}
                    </div>
                  </div>
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </section>
  );
}

