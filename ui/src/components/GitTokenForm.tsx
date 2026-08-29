import { m } from "../paraglide/messages.js";
import { useState } from "react";
import { BUTTON_CLASS_NAME } from "../styleClasses";

/** Paste an Overleaf Git token with a link to where the token is minted. */
export function TokenForm<T>({
  save,
  onSaved,
  placeholder,
  createHref,
}: {
  save: (token: string) => Promise<T>;
  onSaved: (result: T) => void;
  placeholder: string;
  createHref: string;
}) {
  const [token, setToken] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (saving || !token.trim()) return;
    setSaving(true);
    setError(null);
    try {
      onSaved(await save(token.trim()));
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
        placeholder={placeholder}
        autoComplete="off"
      />
      <button type="submit" className={BUTTON_CLASS_NAME} disabled={saving || !token.trim()}>
        {saving ? m.common_saving() : m.common_save()}
      </button>
      <a href={createHref} target="_blank" rel="noreferrer">
        {m.git_token_form_create_a_token()}
      </a>
      {error && <div className="error">{error}</div>}
    </form>
  );
}
