import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ArrowLeft, Plus, RefreshCw, SquareTerminal } from "lucide-react";
import { listAcpHarnesses, listAcpPresets, type AcpDefinition, type Harness, type HarnessId, type HarnessSetupCommands } from "../api";
import type { DialogContentProps } from "../dialog";
import { getHarnessesQuery, getHarnessSetupCommandsQuery, refreshHarnesses } from "../queries/settings";
import { workspaceKey } from "../queries/client";
import { m } from "../paraglide/messages.js";
import { HarnessLogo } from "./HarnessLogo";
import { renderNote } from "./agentNote";
import { Button, DialogContent, DialogTitle, Tooltip } from "./ui";

export function sortHarnesses(harnesses: Harness[]) {
  return [...harnesses].sort((a, b) => {
    const rank = (id: HarnessId) => id === "claude-code" ? 0 : id === "codex" ? 1 : id === "opencode" ? 2 : 3;
    return rank(a.id) - rank(b.id);
  });
}

export function canConfigureHarness(harness: Harness, remote: boolean, commands?: HarnessSetupCommands) {
  if (harness.needsConfigRepair) return false;
  if (harness.id.startsWith("acp:") && harness.installed && !harness.adapterMissing) return true;
  if (remote || !commands || (harness.logo === "zcode-acp-server" && !harness.installed && !harness.adapterMissing)) return false;
  return !harness.installed || harness.installBroken || harness.adapterMissing || harness.authState === "unsupported" || (harness.authState === "needsLogin" && (harness.id !== "claude-code" || harness.loginEligible));
}

export function AddHarnessDialog({ initialCandidate, remote, cancel, onCustom, onEdit, onSetup }: DialogContentProps<void> & {
  initialCandidate?: HarnessId;
  remote: boolean;
  onCustom: () => void;
  onEdit: (definition: AcpDefinition) => void;
  onSetup: (harness: Harness, commands?: HarnessSetupCommands) => void;
}) {
  const harnessQuery = useQuery(getHarnessesQuery());
  const definitions = useQuery({ queryKey: workspaceKey("getAcpHarnesses"), queryFn: listAcpHarnesses });
  const setupCommands = useQuery(getHarnessSetupCommandsQuery());
  const [candidateId, setCandidateId] = useState(initialCandidate);
  const [refreshing, setRefreshing] = useState(false);
  const harnesses = sortHarnesses(harnessQuery.data ?? []);
  const added = harnesses.filter((h) => h.installed || h.installBroken || definitions.data?.some((definition) => definition.id === h.id));
  const candidate = harnesses.find((h) => h.id === candidateId);
  const acpAvailable = definitions.data !== undefined && !definitions.isError;
  const acpError = definitions.isError;
  const openSetup = (harness: Harness) => {
    cancel();
    onSetup(harness, setupCommands.data?.[harness.id]);
  };
  const onSelect = (id: HarnessId | null) => {
    const harness = harnesses.find((h) => h.id === id);
    if (!harness) { setCandidateId(undefined); return; }
    if (canConfigureHarness(harness, remote, setupCommands.data?.[harness.id])) openSetup(harness);
    else setCandidateId(id ?? undefined);
  };
  const onRefresh = () => {
    setRefreshing(true);
    void refreshHarnesses(true, true).catch(() => {}).finally(() => setRefreshing(false));
  };
  const headingRef = useRef<HTMLHeadingElement>(null);
  const presets = useQuery({ queryKey: workspaceKey("getAcpPresets"), queryFn: listAcpPresets });

  useEffect(() => {
    headingRef.current?.focus();
  }, [candidate?.id]);

  const candidateName = presets.data?.find((preset) => preset.setupUrl === candidate?.setupUrl)?.name ?? candidate?.name;
  const needsZCode = candidate?.logo === "zcode-acp-server" && !candidate.installed && !candidate.adapterMissing;
  const choices = [
    ...harnesses.filter((h) => !h.id.startsWith("acp:")).map((h) => ({ harness: h, name: h.name })),
    ...(presets.data ?? []).flatMap((preset) => {
      const harness = harnesses.find((h) => h.setupUrl === preset.setupUrl);
      return harness ? [{ harness, name: preset.name }] : [];
    }),
  ];

  return (
    <DialogContent initialFocus={headingRef} className="w-180 max-h-[calc(100vh_-_40px)] overflow-y-auto p-5 text-text">
      {candidate ? (
        <>
          <Button variant="ghost" size="small" onClick={() => onSelect(null)}>
            <ArrowLeft size={14} />
            {m.harness_choose_another()}
          </Button>
          <div className="mt-4 flex items-center gap-3">
            <HarnessLogo harness={candidate.id} logo={candidate.logo} size={28} />
            <DialogTitle ref={headingRef} tabIndex={-1} className="m-0 text-xl font-medium">{candidateName}</DialogTitle>
          </div>
          <p className="my-4 text-sm text-subtext" role="status">
            {needsZCode ? m.harness_zcode_required() : renderNote(candidate.agentNote) ?? m.harness_install_help({ name: candidateName ?? candidate.name })}
          </p>
          <div className="mt-5 flex flex-wrap items-center justify-end gap-2">
            <Button onClick={cancel}>{m.settings_page_cancel()}</Button>
            <Button onClick={onRefresh} disabled={refreshing}>
              <RefreshCw size={14} />
              {m.settings_page_refresh()}
            </Button>
            {canConfigureHarness(candidate, remote, setupCommands.data?.[candidate.id]) && (
              <Button variant="primary" onClick={() => openSetup(candidate)}>
                <SquareTerminal size={14} />
                {candidate.adapterMissing
                  ? m.acp_install_adapter()
                  : !candidate.installed || candidate.installBroken
                    ? m.harness_setup_install()
                    : candidate.authState === "unsupported"
                      ? m.harness_setup_update()
                      : m.harness_setup_login()}
              </Button>
            )}
            {candidate.setupUrl && (
              <a
                href={needsZCode ? "https://zcode.z.ai/en/docs/install" : candidate.setupUrl}
                target="_blank"
                rel="noreferrer"
                className="text-sm text-accent-blue-strong underline"
              >
                {m.acp_setup_docs()}
              </a>
            )}
            {candidate.id.startsWith("acp:") && !candidate.setupUrl && (
              <Button onClick={() => {
                const definition = definitions.data?.find((item) => item.id === candidate.id);
                if (definition) { cancel(); onEdit(definition); }
              }}>{m.activity_edit()}</Button>
            )}
          </div>
        </>
      ) : (
        <>
          <DialogTitle ref={headingRef} tabIndex={-1} className="m-0 text-xl font-medium">{m.harness_add()}</DialogTitle>
          <p className="mt-2 mb-4 text-sm text-subtext">{m.harness_add_description()}</p>
          {(!acpAvailable || !presets.data) && (
            <p role={acpError || presets.isError ? "alert" : "status"} className="mb-4 text-sm text-subtext">
              {acpError || presets.isError ? m.acp_definitions_load_failed() : m.acp_definitions_loading()} {(acpError || presets.isError) && (
                <Button
                  variant="ghost"
                  onClick={() => {
                    void definitions.refetch();
                    void presets.refetch();
                  }}
                >
                  {m.app_retry()}
                </Button>
              )}
            </p>
          )}
          <div className="grid grid-cols-1 gap-2 sm:grid-cols-2 lg:grid-cols-6">
            {choices.map(({ harness: h, name }) => {
              const alreadyAdded = added.some((item) => item.agentReady && (item.id === h.id || (h.setupUrl && item.setupUrl === h.setupUrl)));
              const unavailable = h.id.startsWith("acp:") && !acpAvailable;
              const featured = h.id === "claude-code" || h.id === "codex";
              const button = (
                <button
                  type="button"
                  aria-disabled={alreadyAdded || unavailable || undefined}
                  aria-label={alreadyAdded ? `${name}: ${m.harness_already_added()}` : name}
                  onClick={() => {
                    if (!alreadyAdded && !unavailable) onSelect(h.id);
                  }}
                  className={`flex ${featured ? "min-h-16 py-3" : "min-h-12 py-2"} w-full items-center gap-3 rounded-lg border border-border bg-background px-3 text-left text-sm font-medium outline-none hover:bg-surface focus-visible:outline-2 focus-visible:outline-focus focus-visible:outline-offset-2 aria-disabled:cursor-not-allowed aria-disabled:opacity-50`}
                >
                  <HarnessLogo harness={h.id} logo={h.logo} size={24} />
                  <span>{name}</span>
                </button>
              );
              const columns = featured ? "lg:col-span-3" : "lg:col-span-2";
              return alreadyAdded ? (
                <Tooltip key={h.id} interactive content={m.harness_already_added()} className={`w-full rounded-lg ${columns}`}>{button}</Tooltip>
              ) : (
                <span key={h.id} className={columns}>{button}</span>
              );
            })}
            <button
              type="button"
              aria-disabled={!acpAvailable || undefined}
              onClick={() => {
                if (acpAvailable) { cancel(); onCustom(); }
              }}
              className="flex min-h-12 items-center gap-3 rounded-lg border border-border bg-background px-3 py-2 text-left text-sm font-medium outline-none hover:bg-surface focus-visible:outline-2 focus-visible:outline-focus focus-visible:outline-offset-2 aria-disabled:cursor-not-allowed aria-disabled:opacity-50 lg:col-span-2"
            >
              <Plus size={24} />
              {m.harness_custom_acp()}
            </button>
          </div>
        </>
      )}
      {!candidate && (
        <div className="mt-5 flex justify-end">
          <Button onClick={cancel}>{m.settings_page_cancel()}</Button>
        </div>
      )}
    </DialogContent>
  );
}
