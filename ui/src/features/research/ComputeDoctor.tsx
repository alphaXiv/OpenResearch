import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { m } from "../../paraglide/messages.js";
import { getComputeSettingsQuery } from "../../queries/settings";
import { TARGET_LABELS } from "../../computeTargets";
import { Button, Spinner } from "../../components/ui";
import { diagnoseCompute } from "./api";

export function ComputeDoctor() {
  const settings = useQuery(getComputeSettingsQuery());
  const [backend, setBackend] = useState("colab");
  const probe = useMutation({ mutationFn: diagnoseCompute });
  return <section className="mb-8 rounded-lg border border-border p-4">
    <h2 className="mt-0 mb-2 text-lg">{m.research_doctor_title()}</h2>
    <p className="text-sm text-subtext">{m.research_doctor_hint()}</p>
    <div className="flex flex-wrap items-center gap-3">
      <select aria-label={m.research_backend()} value={backend} disabled={probe.isPending}
        className="rounded border border-border bg-background p-2 text-sm"
        onChange={event => { setBackend(event.target.value); probe.reset(); }}>
        {(settings.data?.targets ?? [{ id: "colab" as const }]).map(target =>
          <option key={target.id} value={target.id}>{TARGET_LABELS[target.id]()}</option>)}
      </select>
      <Button disabled={probe.isPending} onClick={() => probe.mutate(backend)}>
        {probe.isPending && <Spinner />}{m.research_diagnose()}
      </Button>
    </div>
    {probe.error && <p role="alert" className="text-sm text-accent-red">{probe.error.message}</p>}
    {probe.data && <div aria-live="polite" className="mt-4 text-sm">
      <p className={probe.data.ready ? "text-accent-green" : "text-accent-amber"}>
        {probe.data.ready ? m.research_check_passed() : m.research_check_failed()}
      </p>
      <p className="break-all text-subtext">orx {probe.data.version} · {probe.data.buildChannel} · {probe.data.executable}</p>
      <ul className="space-y-3 ps-5">
        {probe.data.checks.map((check, index) => <li key={`${check.id}-${index}`}>
          <span className={check.status === "pass" ? "text-accent-green" : "text-accent-red"}>
            {check.status === "pass" ? "✓" : "✗"}
          </span>{" "}{check.message}
          {check.action && <p className="mt-1 whitespace-pre-wrap text-subtext">{check.action}</p>}
        </li>)}
      </ul>
    </div>}
  </section>;
}
