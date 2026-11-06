import { useMachines } from "../machineSelection";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { Popover } from "@base-ui/react/popover";
import { useInfiniteQuery } from "@tanstack/react-query";
import { ArrowLeft, ArrowUpRight, Check, FileText, Folder, Search } from "lucide-react";
import { useContext, useRef, useState } from "react";
import { AlphaXivNotConnectedError, type AlphaXivPaper } from "../api";
import { fmtNumber } from "../i18n";
import { m } from "../paraglide/messages.js";
import { useSignIn } from "./AccountChip";
import { EscapeScope } from "./EscapeScope";
import { LitSourceLogo } from "./LitSourceLogo";
import { useDebouncedValue } from "./NewProjectForm";
import { Button, IconButton, Input, SEGMENTED_CLASS_NAME, Spinner, Tooltip } from "./ui";

const STATE_CLASS_NAME =
  "flex h-full flex-col items-center justify-center gap-3 p-6 text-center text-sm text-subtext";

type Props = {
  onAdd: (papers: AlphaXivPaper[]) => void;
  className: string;
};

export function LibraryPicker(props: Props) {
  const { useOpenResearchAccount } = useMachines().owner.queries;
  const account = useOpenResearchAccount();
  const user = account.data?.user;
  if (!account.data?.loggedIn || !user) return null;
  return <SignedInLibraryPicker key={user.id} user={user} {...props} />;
}

function SignedInLibraryPicker({
  onAdd,
  className,
  user,
}: Props & {
  user: { id: string; alphaxivUrl: string | null };
}) {
  const workspace = useMachines().owner;
  const openSignIn = useSignIn();
  const { workspaceKey } = workspace;
  const { getAlphaXivPapers } = workspace.api;
  const { useAlphaXivFolders } = workspace.queries;
  const { portalContainer } = useContext(WorkspaceDomContext);
  const folders = useAlphaXivFolders();
  const [open, setOpen] = useState(false);
  const [view, setView] = useState<"recent" | "folders" | { folderId: string }>("recent");
  const [search, setSearch] = useState("");
  const [selected, setSelected] = useState<AlphaXivPaper[]>([]);
  const trigger = useRef<HTMLButtonElement>(null);
  const searchInput = useRef<HTMLInputElement>(null);
  const added = useRef(false);
  const debouncedSearch = useDebouncedValue(search.trim(), 200);
  const query = search.trim() ? debouncedSearch : "";
  const folderId = typeof view === "object" ? view.folderId : null;
  const folder = folders.data?.find((item) => item.id === folderId);
  const papers = useInfiniteQuery({
    queryKey: workspaceKey("alphaXivPapers", user.id, folderId, query),
    queryFn: ({ pageParam, signal }) => getAlphaXivPapers(folderId, pageParam, query, signal),
    enabled: open && view !== "folders" && !folders.error,
    initialPageParam: null as string | null,
    getNextPageParam: (page) => page.nextCursor,
  }, workspace.queryClient);
  const error = folders.error ?? (view !== "folders" ? papers.error : null);
  const visibleFolders =
    folders.data?.filter((item) =>
      item.name.toLocaleLowerCase().includes(search.trim().toLocaleLowerCase()),
    ) ?? [];
  const allPapers = papers.data?.pages.flatMap((page) => page.papers) ?? [];
  const alphaUrl = user.alphaxivUrl;

  const changeOpen = (next: boolean) => {
    setOpen(next);
    if (next) added.current = false;
    else {
      setSelected([]);
      setSearch("");
      setView("recent");
    }
  };

  return (
    <EscapeScope enabled={open} onEscape={() => changeOpen(false)}>
      <Popover.Root open={open} onOpenChange={changeOpen}>
        <Tooltip interactive disabled={open} content={m.library_picker()}>
          <Popover.Trigger
            ref={trigger}
            render={<IconButton size="small" className={className} aria-label={m.library_picker()} />}
          >
            <LitSourceLogo source="alphaxiv" decorative framed={false} className="text-subtext [&_svg_*]:fill-current" />
          </Popover.Trigger>
        </Tooltip>
        <Popover.Portal container={portalContainer}>
          <Popover.Positioner
            side="top"
            align="start"
            sideOffset={8}
            collisionPadding={12}
            className="z-50"
          >
            <Popover.Popup
              aria-label={m.library_title()}
              initialFocus={searchInput}
              finalFocus={() => (added.current ? false : trigger.current)}
              className="flex w-104 max-w-[calc(100vw-1.5rem)] h-[min(34rem,var(--available-height))] flex-col overflow-hidden rounded-xl border border-border bg-background text-text shadow-menu outline-none"
            >
              <div className="flex items-center justify-between gap-3 px-3.5 pt-3 pb-2.5">
                <span className="text-sm font-medium">{m.library_title()}</span>
                {alphaUrl && (
                  <a
                    href={new URL("/bookmarks", alphaUrl).href}
                    target="_blank"
                    rel="noreferrer"
                    className="inline-flex items-center gap-1 text-sm text-subtext hover:text-text focus-visible:outline-2 focus-visible:outline-focus"
                  >
                    {m.library_open_alphaxiv()}
                    <ArrowUpRight size={14} aria-hidden="true" />
                  </a>
                )}
              </div>
              <div className="relative mx-3 mb-2">
                <Search
                  size={15}
                  className="pointer-events-none absolute start-2.5 top-2 text-muted"
                  aria-hidden="true"
                />
                <Input
                  ref={searchInput}
                  value={search}
                  onChange={(event) => setSearch(event.target.value)}
                  aria-label={m.library_search()}
                  placeholder={m.library_search()}
                  className="ps-8"
                />
              </div>
              <div className="mx-3 mb-2 flex h-7">
                {folderId ? (
                  <Button
                    variant="ghost"
                    size="small"
                    onClick={() => {
                      setView("folders");
                      setSearch("");
                    }}
                    className="-ms-2.5 min-w-0"
                  >
                    <ArrowLeft size={14} aria-hidden="true" />
                    <span className="truncate" dir="auto">
                      {folder?.name ?? m.library_folders()}
                    </span>
                  </Button>
                ) : (
                  <div className={`${SEGMENTED_CLASS_NAME} w-full [&_button]:flex-1 [&_button]:justify-center`}>
                    {(["recent", "folders"] as const).map((tab) => (
                      <button
                        key={tab}
                        type="button"
                        className={view === tab ? "active" : undefined}
                        aria-pressed={view === tab}
                        onClick={() => {
                          setView(tab);
                          setSearch("");
                        }}
                      >
                        {tab === "recent" ? m.library_recent() : m.library_folders()}
                      </button>
                    ))}
                  </div>
                )}
              </div>
              <div className="min-h-0 min-w-0 flex-1 overflow-y-auto overscroll-contain px-1.5 pb-1.5">
                {error instanceof AlphaXivNotConnectedError ? (
                  <div className={STATE_CLASS_NAME}>
                    <p>{m.library_connect_description()}</p>
                    <Popover.Close
                      render={<Button />}
                      onClick={() => openSignIn(true, trigger.current)}
                    >
                      {m.library_connect()}
                    </Popover.Close>
                  </div>
                ) : error ? (
                  <div role="alert" className={STATE_CLASS_NAME}>
                    <p>{m.library_error()}</p>
                    <Button
                      onClick={() => {
                        void folders.refetch();
                        if (view !== "folders") void papers.refetch();
                      }}
                    >
                      {m.app_retry()}
                    </Button>
                  </div>
                ) : folders.isLoading || (view !== "folders" && papers.isPending) ? (
                  <p role="status" className={`${STATE_CLASS_NAME} flex-row`}>
                    <Spinner />
                    {m.library_loading()}
                  </p>
                ) : view === "folders" ? (
                  visibleFolders.length === 0 ? (
                    <p className={STATE_CLASS_NAME}>
                      {search ? m.library_no_matches() : m.library_empty_folders()}
                    </p>
                  ) : (
                    <ul className="m-0 list-none p-0">
                      {visibleFolders.map((item) => (
                        <li key={item.id}>
                          <Button
                            variant="ghost"
                            className="w-full justify-start gap-2.5 font-normal"
                            onClick={() => {
                              setView({ folderId: item.id });
                              setSearch("");
                            }}
                          >
                            <Folder
                              size={17}
                              className="shrink-0 text-subtext"
                              aria-hidden="true"
                            />
                            <span className="min-w-0 flex-1 truncate text-start" dir="auto">
                              {item.name}
                            </span>
                            <span className="text-subtext tabular-nums">{fmtNumber(item.paperCount)}</span>
                          </Button>
                        </li>
                      ))}
                    </ul>
                  )
                ) : allPapers.length === 0 ? (
                  <p className={STATE_CLASS_NAME}>
                    {query
                      ? m.library_no_matches()
                      : folderId
                        ? m.library_empty_papers()
                        : m.library_empty()}
                  </p>
                ) : (
                  <ul aria-label={folder?.name ?? m.library_recent()} className="m-0 list-none p-0">
                    {allPapers.map((paper) => {
                      const checked = selected.some(
                        (item) => item.paperGroupId === paper.paperGroupId,
                      );
                      return (
                        <li key={paper.paperGroupId}>
                          <button
                            type="button"
                            aria-pressed={checked}
                            onClick={() =>
                              setSelected((current) =>
                                checked
                                  ? current.filter(
                                      (item) => item.paperGroupId !== paper.paperGroupId,
                                    )
                                  : [...current, paper],
                              )
                            }
                            className={`group flex w-full items-center gap-3 rounded-lg p-2 text-start focus-visible:outline-2 focus-visible:outline-focus ${checked ? "bg-highlight" : "hover:bg-surface"}`}
                          >
                            <span
                              className="flex h-15 w-12 shrink-0 items-center justify-center rounded-sm border border-border bg-panel text-muted"
                              aria-hidden="true"
                            >
                              <FileText size={21} />
                            </span>
                            <span className="min-w-0 flex-1">
                              <span
                                className="line-clamp-2 break-words text-sm font-medium leading-snug"
                                dir="auto"
                              >
                                {paper.title}
                              </span>
                              <span className="mt-1 block truncate text-xs text-subtext" dir="auto">
                                {paper.authors.join(", ")}
                                {paper.authorCount > paper.authors.length && (
                                  <>
                                    {" "}
                                    {m.library_more_authors({
                                      count: fmtNumber(paper.authorCount - paper.authors.length),
                                    })}
                                  </>
                                )}
                              </span>
                            </span>
                            <span
                              className={`flex size-4 shrink-0 items-center justify-center rounded-sm border ${checked ? "border-primary bg-primary text-background" : "border-border"}`}
                              aria-hidden="true"
                            >
                              {checked && <Check size={12} />}
                            </span>
                          </button>
                        </li>
                      );
                    })}
                  </ul>
                )}
                {view !== "folders" && papers.hasNextPage && (
                  <Button
                    variant="ghost"
                    className="mt-1 w-full"
                    disabled={papers.isFetchingNextPage}
                    onClick={() => void papers.fetchNextPage()}
                  >
                    {papers.isFetchingNextPage ? m.library_loading() : m.library_more()}
                  </Button>
                )}
              </div>
              <div className="flex shrink-0 items-center justify-between gap-3 border-t border-border px-3 py-2.5">
                <span role="status" className="text-xs text-subtext">
                  {m.library_selected({ count: fmtNumber(selected.length) })}
                </span>
                <Popover.Close
                  disabled={selected.length === 0}
                  render={<Button variant="primary" />}
                  onClick={() => {
                    added.current = true;
                    onAdd(selected);
                  }}
                >
                  {m.library_add_papers()}
                </Popover.Close>
              </div>
            </Popover.Popup>
          </Popover.Positioner>
        </Popover.Portal>
      </Popover.Root>
    </EscapeScope>
  );
}
