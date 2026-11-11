import { createFileRoute } from "@tanstack/react-router";
import { Spinner } from "../components/ui";
import { m } from "../paraglide/messages.js";

/** The tab sign-in reserves during the click, until the server says where alphaXiv's page is. */
export const Route = createFileRoute("/remote-launch")({ component: SignInTab });

function SignInTab() {
  return (
    <main className="app flex h-full items-center justify-center bg-background text-text">
      <div role="status" className="flex items-center gap-2 text-sm text-subtext"><Spinner />{m.account_continuing_to_alphaxiv()}</div>
    </main>
  );
}
