import { useInfiniteQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { AlphaXivNotConnectedError, getAlphaXivPapers } from "../api";
import { queryClient, workspaceKey } from "../queries/client";
import { useAlphaXivFolders, useOpenResearchAccount } from "../queries/settings";
import { m } from "../paraglide/messages.js";
import { Button, Spinner } from "./ui";
import { accountErrorMessage } from "./OpenResearchAccount";

export function LibraryView() {
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => { heading.current?.focus(); }, []);
  const account = useOpenResearchAccount();
  const [selected, setSelected] = useState<string | null>(null);
  const folders = useAlphaXivFolders();
  const folder = folders.data?.find((item) => item.id === selected) ?? folders.data?.[0];
  const papers = useInfiniteQuery({
    queryKey: workspaceKey("alphaXivPapers", folder?.id ?? null),
    queryFn: ({ signal, pageParam }) => getAlphaXivPapers(folder!.id, pageParam, signal),
    enabled: !!folder,
    initialPageParam: null as string | null,
    getNextPageParam: (page) => page.nextCursor,
    retry: false,
  });
  useEffect(() => {
    if (papers.error instanceof AlphaXivNotConnectedError) void queryClient.invalidateQueries({ queryKey: workspaceKey("alphaXivFolders") });
  }, [papers.error]);
  const error = folders.error ?? papers.error;
  const connect = () => window.dispatchEvent(new Event(account.data?.loggedIn ? "alphaxiv-connect" : "openresearch-sign-in"));
  const retry = () => { void folders.refetch(); if (folder) void papers.refetch(); };
  const allPapers = papers.data?.pages.flatMap((page) => page.papers) ?? [];

  return <>
    <h1 ref={heading} tabIndex={-1}>{m.library_title()}</h1>
    {account.isLoading ? <p role="status"><Spinner /> {m.common_checking()}</p>
      : account.isError ? <div role="alert"><p>{accountErrorMessage(account.error)}</p><Button onClick={() => void account.refetch()}>{m.app_retry()}</Button></div>
      : !account.data?.loggedIn || error instanceof AlphaXivNotConnectedError ? <div className="mt-6"><p>{m.library_connect_description()}</p><Button onClick={connect}>{account.data?.loggedIn ? m.library_connect() : m.account_sign_in_alphaxiv()}</Button></div>
      : error ? <div role="alert" className="mt-6"><p>{m.library_error()}</p><Button onClick={retry}>{m.app_retry()}</Button></div>
      : folders.isLoading ? <p role="status" className="mt-6"><Spinner /> {m.library_loading()}</p>
      : !folder ? <p className="mt-6">{m.library_empty_folders()}</p>
      : <div className="mt-6">
          <nav aria-label={m.library_folders()}><ul className="m-0 flex flex-wrap gap-2 p-0 list-none">{folders.data?.map((item) => <li key={item.id}><button type="button" aria-current={folder.id === item.id ? "true" : undefined} onClick={() => setSelected(item.id)} className={`min-h-8 rounded-md border border-border px-3 py-1 text-sm focus-visible:outline-2 focus-visible:outline-focus ${folder.id === item.id ? "bg-panel text-text" : "text-subtext hover:bg-surface"}`}>{item.name} <span className="text-subtext">({item.paperCount})</span></button></li>)}</ul></nav>
          <h2 className="mt-6 text-xl font-medium" dir="auto">{folder.name}</h2>
          {papers.isLoading ? <p role="status"><Spinner /> {m.library_loading()}</p> : allPapers.length === 0 ? <p>{m.library_empty_papers()}</p> : <ul className="m-0 list-none divide-y divide-border p-0">{allPapers.map((paper) => <li key={paper.paperGroupId} className="py-3"><a className="break-words font-medium text-text underline" href={paper.url} target="_blank" rel="noreferrer" dir="auto">{paper.title}</a><p className="mb-0 mt-1 text-sm text-subtext" dir="auto">{paper.authors.join(", ")}{paper.authorCount > paper.authors.length && <> {m.library_more_authors({ count: String(paper.authorCount - paper.authors.length) })}</>} · {paper.publishedAt.slice(0, 10)}</p></li>)}</ul>}
          {papers.hasNextPage && <Button disabled={papers.isFetchingNextPage} onClick={() => void papers.fetchNextPage()}>{papers.isFetchingNextPage ? m.library_loading() : m.library_more()}</Button>}
        </div>}
  </>;
}
