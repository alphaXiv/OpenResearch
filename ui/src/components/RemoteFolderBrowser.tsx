import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { browseProjectPathQuery } from "../queries/projects";
import { m } from "../paraglide/messages.js";
import {
  Folder,
  FolderGit2,
  Home,
  ArrowUp,
  ChevronRight,
  Eye,
  EyeOff,
  FileText,
  Search,
} from "lucide-react";
import { Spinner } from "./ui";

interface RemoteFolderBrowserProps {
  selectedPath: string;
  onSelectPath: (path: string) => void;
  className?: string;
}

export function RemoteFolderBrowser({
  selectedPath,
  onSelectPath,
  className = "",
}: RemoteFolderBrowserProps) {
  const [browsingPath, setBrowsingPath] = useState<string>("");
  const [showHidden, setShowHidden] = useState(false);
  const [showFiles] = useState(false);
  const [filter, setFilter] = useState("");

  const query = useQuery(browseProjectPathQuery(browsingPath));
  const data = query.data;
  const currentPath = data?.currentPath ?? browsingPath;

  const pathSegments = currentPath ? currentPath.split("/").filter(Boolean) : [];

  const visibleEntries = (data?.entries ?? []).filter((entry) => {
    if (!showHidden && entry.hidden) return false;
    if (!showFiles && !entry.isDir) return false;
    if (filter.trim() && !entry.name.toLowerCase().includes(filter.trim().toLowerCase())) {
      return false;
    }
    return true;
  });

  return (
    <div
      className={`remote-folder-browser flex flex-col border border-border rounded-md bg-surface text-sm overflow-hidden ${className}`}
    >
      {/* Navigation Header */}
      <div className="flex items-center gap-1 p-1.5 border-b border-border bg-background text-xs">
        {/* Home jump */}
        <button
          type="button"
          className="inline-flex items-center gap-1 px-2 py-1 rounded text-subtext hover:text-text hover:bg-hover-subtle cursor-pointer border-0 bg-transparent"
          title={m.new_project_home_folder()}
          onClick={() => setBrowsingPath(data?.homePath ?? "~")}
        >
          <Home size={13} />
          <span>{m.new_project_home_folder()}</span>
        </button>

        {/* Up parent jump */}
        {data?.parentPath && (
          <button
            type="button"
            className="inline-flex items-center gap-1 px-2 py-1 rounded text-subtext hover:text-text hover:bg-hover-subtle cursor-pointer border-0 bg-transparent"
            title={m.new_project_parent_folder()}
            onClick={() => setBrowsingPath(data.parentPath!)}
          >
            <ArrowUp size={13} />
            <span>..</span>
          </button>
        )}

        {/* Breadcrumb Path */}
        <div
          className="flex-1 min-w-0 flex items-center gap-1 px-1 overflow-x-auto whitespace-nowrap scrollbar-none text-muted"
          dir="ltr"
        >
          <button
            type="button"
            className="hover:text-text cursor-pointer border-0 bg-transparent p-0 text-muted"
            onClick={() => setBrowsingPath("/")}
          >
            /
          </button>
          {pathSegments.map((segment, idx) => {
            const segPath = "/" + pathSegments.slice(0, idx + 1).join("/");
            const isLast = idx === pathSegments.length - 1;
            return (
              <span key={segPath} className="inline-flex items-center gap-1">
                <span>/</span>
                <button
                  type="button"
                  className={`hover:text-text cursor-pointer border-0 bg-transparent p-0 ${
                    isLast ? "font-medium text-text" : "text-subtext"
                  }`}
                  onClick={() => setBrowsingPath(segPath)}
                >
                  {segment}
                </button>
              </span>
            );
          })}
        </div>

        {/* Show hidden toggle */}
        <button
          type="button"
          className={`p-1 rounded cursor-pointer border-0 bg-transparent ${
            showHidden ? "text-text bg-hover-subtle" : "text-muted hover:text-text"
          }`}
          title={m.new_project_show_hidden()}
          aria-label={m.new_project_show_hidden()}
          onClick={() => setShowHidden(!showHidden)}
        >
          {showHidden ? <Eye size={13} /> : <EyeOff size={13} />}
        </button>
      </div>

      {/* Filter bar */}
      <div className="flex items-center gap-1.5 px-2 py-1 border-b border-border bg-background text-xs">
        <Search size={12} className="text-muted shrink-0" />
        <input
          type="text"
          className="flex-1 min-w-0 bg-transparent border-0 outline-none text-xs text-text placeholder:text-muted"
          placeholder={m.new_project_filter_folders()}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
        {currentPath && (
          <button
            type="button"
            className="text-xs text-subtext hover:text-text underline cursor-pointer bg-transparent border-0 p-0 shrink-0"
            onClick={() => onSelectPath(currentPath)}
          >
            {m.new_project_select_this_folder()}
          </button>
        )}
      </div>

      {/* Folder listing */}
      <div className="h-44 max-h-44 overflow-y-auto p-1 flex flex-col gap-0.5" role="list">
        {query.isPending && (
          <div className="flex items-center justify-center h-full text-subtext gap-2">
            <Spinner />
          </div>
        )}

        {query.isError && (
          <div className="p-3 text-center text-xs text-accent-red">
            {query.error instanceof Error ? query.error.message : String(query.error)}
          </div>
        )}

        {!query.isPending && !query.isError && visibleEntries.length === 0 && (
          <div className="flex items-center justify-center h-full text-xs text-subtext p-4 text-center">
            {m.new_project_no_folders_found()}
          </div>
        )}

        {!query.isPending &&
          !query.isError &&
          visibleEntries.map((entry) => {
            const isSelected =
              selectedPath.replace(/[\\/]+$/, "") === entry.path.replace(/[\\/]+$/, "");
            return (
              <div
                key={entry.path}
                className={`group flex items-center justify-between gap-1.5 px-2 py-1 rounded cursor-pointer transition-colors select-none ${
                  isSelected
                    ? "bg-hover-subtle font-medium text-text border border-border"
                    : "text-subtext hover:bg-hover-subtle hover:text-text"
                }`}
                onClick={() => {
                  if (entry.isDir) {
                    onSelectPath(entry.path);
                  }
                }}
                onDoubleClick={() => {
                  if (entry.isDir) {
                    setBrowsingPath(entry.path);
                    onSelectPath(entry.path);
                  }
                }}
                title={entry.path}
              >
                <div className="flex items-center gap-1.5 min-w-0 flex-1">
                  {entry.isDir ? (
                    entry.isGit ? (
                      <FolderGit2 size={15} className="shrink-0 text-text" />
                    ) : (
                      <Folder size={15} className="shrink-0 text-muted group-hover:text-text" />
                    )
                  ) : (
                    <FileText size={14} className="shrink-0 text-muted" />
                  )}
                  <span className="truncate text-xs font-normal text-text">{entry.name}</span>
                  {entry.isGit && (
                    <span className="text-xs px-1 py-0.5 rounded bg-surface border border-border text-subtext shrink-0 leading-none">
                      git
                    </span>
                  )}
                </div>

                {entry.isDir && (
                  <button
                    type="button"
                    className="opacity-0 group-hover:opacity-100 p-0.5 rounded hover:bg-surface text-subtext hover:text-text border-0 bg-transparent cursor-pointer shrink-0"
                    title={entry.name}
                    onClick={(e) => {
                      e.stopPropagation();
                      setBrowsingPath(entry.path);
                      onSelectPath(entry.path);
                    }}
                  >
                    <ChevronRight size={14} />
                  </button>
                )}
              </div>
            );
          })}
      </div>
    </div>
  );
}
