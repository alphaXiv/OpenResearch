import { queryOptions } from "@tanstack/react-query";

import type { LocalMachine, RuntimeInfo, UpdateStatus } from "./api";

/** Canary and dev builds on this machine; stable releases never offer Report a bug. */
export const bugReportsOffered = (runtime: RuntimeInfo, status: UpdateStatus | null | undefined) =>
  runtime.kind === "local" && (runtime.buildChannel === "development" || (status?.build.startsWith("canary-") ?? false));

/** The dev server's checkout, where it forked from main, and its pull request, from `ui/vite.config.ts`. */
export type DevCheckout = { commit: string; base: string | null; pr: string | null };

export const devCheckoutQuery = queryOptions({
  queryKey: ["dev-checkout"],
  queryFn: async (): Promise<DevCheckout> => {
    const res = await fetch("/__dev/commit?pr", { headers: { accept: "application/json" } });
    const body = await res.json();
    if (!res.ok) throw new Error(body.error);
    return body;
  },
  staleTime: 60_000,
});

/** `copied`, or why the window could not copy one; `error` is null when nothing tried. */
export type Screenshot = { copied: true } | { copied: false; error: string | null };

/** Asks the desktop window for a screenshot on the clipboard; undefined when it never answers, as an older window won't. */
export function copyScreenshot(ipc: NonNullable<Window["ipc"]>): Promise<Screenshot | undefined> {
  return new Promise((resolve) => {
    const answered = (event: Event) => {
      const { error } = (event as CustomEvent<{ error: string | null }>).detail;
      done(error === null ? { copied: true } : { copied: false, error });
    };
    const timer = setTimeout(() => done(undefined), 5_000);
    function done(screenshot: Screenshot | undefined) {
      clearTimeout(timer);
      window.removeEventListener("orx:screenshot", answered);
      resolve(screenshot);
    }

    window.addEventListener("orx:screenshot", answered);
    ipc.postMessage("report:screenshot");
  });
}

/** A new issue: a bare draft in Linear's app for the team, the template on GitHub for everyone else. Written for the team, so English. */
export function bugReportUrl({ linear, version, screenshot, machine, checkout }: {
  linear: boolean;
  version: string;
  screenshot: Screenshot;
  machine: LocalMachine | Error;
  checkout: DevCheckout | Error | null;
}): string {
  const paste = navigator.platform.startsWith("Mac") ? "Cmd+V" : "Ctrl+V";
  const shot = screenshot.copied
    ? `(remember to paste your screenshot with ${paste})`
    : screenshot.error
      ? `(the window couldn't capture a screenshot: ${screenshot.error})`
      : "(attach a screenshot if it helps)";

  const build = [
    /^\d/.test(version) ? `v${version}` : version,
    ...machine instanceof Error
      ? [`machine unknown (${machine.message})`]
      : [machine.osName ?? `${machine.os}/${machine.arch}`, machine.model, machine.chip],
  ].filter(Boolean).join(", ");
  const commit = checkout && (checkout instanceof Error
    ? `Dev Build from a checkout git and jj couldn't read: ${checkout.message}`
    : `${checkout.base ? `Dev Build based on \`${checkout.base}\` from main` : `Dev Build from \`${checkout.commit}\`, fork point on main unknown`}${checkout.pr ? ` (${checkout.pr})` : ""}.`);
  const footer = ["---", build, commit].filter(Boolean).join("\n\n");

  if (linear) {
    const description = `${shot}\n\n${footer}`;
    return `linear://alphaxiv/team/ORX/new?${query({ description, status: "Triage" })}`;
  }
  const body = [
    "### What happened\n",
    "### What you expected\n",
    "### Steps to reproduce\n\n1. ",
    `### Screenshot\n\n${shot}`,
    footer,
  ].join("\n\n");
  return `https://github.com/alphaXiv/OpenResearch/issues/new?${query({ body, labels: "bug" })}`;
}

const query = (params: Record<string, string>) => new URLSearchParams(params).toString().replaceAll("+", "%20");
