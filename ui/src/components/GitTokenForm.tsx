import { useState } from "react";
import { saveGitToken, type GitSettings } from "../api";
import { BUTTON_CLASS_NAME } from "../styleClasses";

/** Paste-a-PAT fallback for GitHub access — validated server-side, stored in
 * the synced env file. Reports the refreshed git settings on success. */
export function GitTokenForm({ onSaved }: { onSaved: (g: GitSettings) => void }) {
  const [token, setToken] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (saving || !token.trim()) return;
    setSaving(true);
    setError(null);
    try {
      onSaved(await saveGitToken(token.trim()));
      setToken("");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <form className="onb-token-form flex items-center flex-wrap gap-2 mt-2 [&_input]:flex-1 [&_input]:min-w-55 [&_input]:font-mono [&_input]:text-sm [&_a]:text-sm [&_a]:text-subtext [&_a]:whitespace-nowrap [&_.error]:basis-full [&_.error]:text-accent-red [&_.error]:text-md [&_.error]:whitespace-pre-wrap" onSubmit={submit}>
      <input
        type="password"
        value={token}
        onChange={(e) => setToken(e.target.value)}
        placeholder="ghp_… personal access token"
        autoComplete="off"
      />
      <button type="submit" className={BUTTON_CLASS_NAME} disabled={saving || !token.trim()}>
        {saving ? "Checking…" : "Save"}
      </button>
      <a
        href="https://github.com/settings/tokens/new?scopes=repo,workflow&description=orx"
        target="_blank"
        rel="noreferrer"
      >
        Create a token ↗
      </a>
      {error && <div className="error">{error}</div>}
    </form>
  );
}
