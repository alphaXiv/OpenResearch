import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState } from "react";
import { RunBanner } from "../src/components/run/RunBanner";
import { Button } from "../src/components/ui";
import { overwriteGetLocale } from "../src/paraglide/runtime.js";
import { WorkspaceContext } from "../src/workspace";
import { WorkspaceDomContext } from "../src/WorkspaceDom";
import { sessionId } from "../tests/fixtures/runMonitor/gallery";
import { galleryWorkspace, groups } from "./backend";

type Entry = (typeof groups)[number]["specimens"][number];

function Specimen({ entry, client, locale }: { entry: Entry; client: QueryClient; locale: "en" | "ar" }) {
  const [workspace] = useState(() => galleryWorkspace(entry, client));
  const [portalContainer, setPortalContainer] = useState<HTMLDivElement | null>(null);
  const [pinned, pin] = useState(entry.pin !== undefined);
  const [action, setAction] = useState("");
  const area = useRef<HTMLDivElement>(null);
  const name = useId();

  useEffect(() => {
    if (!pinned || entry.pin !== "stage" || !portalContainer) return;
    const trigger = area.current?.querySelector<HTMLButtonElement>('button[aria-haspopup="dialog"][aria-expanded]');
    if (!trigger) return;
    // A click without a pointer press opens this trigger without dismissing the other pinned cards.
    const open = () => {
      if (trigger.getAttribute("aria-expanded") === "false") {
        trigger.dispatchEvent(new MouseEvent("click", { bubbles: true, detail: 1 }));
      }
    };
    const observer = new MutationObserver(open);
    observer.observe(trigger, { attributes: true, attributeFilter: ["aria-expanded"] });
    open();
    return () => observer.disconnect();
  }, [entry.pin, pinned, portalContainer, locale]);

  useEffect(() => workspace.events.onHostConnectRequest(() => {
    const host = workspace.events.takeHostConnect();
    setAction(`Connect to ${host} in the dashboard.`);
  }), [workspace]);

  return (
    <article className="gallery-specimen" aria-labelledby={name} data-specimen={entry.id}>
      <div className="gallery-specimen-heading">
        <h3 id={name}>{entry.label}</h3>
        {entry.pin && <Button size="small" variant="ghost" aria-pressed={pinned} onClick={() => pin(!pinned)}>
          {{ stage: "Pin stage card", eta: "Pin time tooltip", stop: "Pin Stop tooltip" }[entry.pin]}
        </Button>}
      </div>
      <div ref={area} lang={locale} dir={locale === "ar" ? "rtl" : "ltr"} data-pin={pinned ? entry.pin : undefined} data-specimen-output>
        <WorkspaceContext.Provider value={workspace}>
          <WorkspaceDomContext.Provider value={{ portalContainer: portalContainer ?? undefined, id: (part) => `${name}-${part}` }}>
            <RunBanner key={locale} sessionId={sessionId} agent="Claude"
              onOpenRunPage={(run) => setAction(`Open ${entry.runs.find(({ item }) => item.run.id === run.id)?.item.experimentName ?? run.id} in the dashboard.`)}
              onOpenRun={(_runId, _intent, seek) => setAction(seek ? `Open the flagged log excerpt at byte ${seek.at} in the dashboard.` : "Open the run in the dashboard.")} />
            <workspace.dialogs.DialogManager />
            <div className="gallery-portals" ref={setPortalContainer} />
          </WorkspaceDomContext.Provider>
        </WorkspaceContext.Provider>
      </div>
      {action && <p className="gallery-action" role="status">{action}</p>}
    </article>
  );
}

function newClient() {
  return new QueryClient({ defaultOptions: { queries: {
    gcTime: Infinity, retry: false, retryOnMount: false,
    refetchOnWindowFocus: false, refetchOnReconnect: false,
  } } });
}

export function Gallery() {
  const [client] = useState(newClient);
  const [theme, setTheme] = useState<"light" | "dark">("light");
  const [width, setWidth] = useState("1440");
  const [locale, setLocale] = useState<"en" | "ar">("en");
  const [group, setGroup] = useState("all");
  const visible = group === "all" ? groups : groups.filter(({ id }) => id === group);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);
  useEffect(() => {
    document.documentElement.lang = locale;
  }, [locale]);

  return (
    <QueryClientProvider client={client}>
      <header className="gallery-header" lang="en">
        <div className="gallery-toolbar">
          <h1>Run strip gallery</h1>
          <div className="gallery-controls">
            <label>Theme
              <select value={theme} onChange={(event) => setTheme(event.target.value === "dark" ? "dark" : "light")}>
                <option value="light">Light</option>
                <option value="dark">Dark</option>
              </select>
            </label>
            <label>Width
              <select value={width} onChange={(event) => setWidth(event.target.value)}>
                <option value="1440">1440 px</option>
                <option value="720">720 px</option>
                <option value="420">420 px</option>
              </select>
            </label>
            <label>Language
              <select value={locale} onChange={(event) => {
                const next = event.target.value === "ar" ? "ar" : "en";
                overwriteGetLocale(() => next);
                setLocale(next);
              }}>
                <option value="en">English</option>
                <option value="ar" lang="ar">العربية</option>
              </select>
            </label>
            <label>Group
              <select value={group} onChange={(event) => setGroup(event.target.value)}>
                <option value="all">All specimens</option>
                {groups.map(({ id, name }) => <option key={id} value={id}>{name}</option>)}
              </select>
            </label>
          </div>
        </div>
      </header>
      <main className="gallery-main" lang="en">
        <p className="gallery-intro">Fixture runs for the chat composer. Expand a strip or open its stage card to inspect it.</p>
        <div className="gallery-column" style={{ width: `${width}px` }}>
          {visible.map(({ id, name, specimens }) => (
            <section key={id} className="gallery-group" aria-label={name}>
              <h2>{name}</h2>
              {specimens.map((entry) => <Specimen key={entry.id} entry={entry} client={client} locale={locale} />)}
            </section>
          ))}
        </div>
      </main>
    </QueryClientProvider>
  );
}
