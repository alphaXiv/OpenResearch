import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { Spinner } from '../components/ui'
import { m } from '../paraglide/messages.js'

export const Route = createFileRoute('/remote-launch')({
  component: RemoteLaunchComponent,
})

function RemoteLaunchComponent() {
  const [targetUrl, setTargetUrl] = useState<string | null>(null);

  useEffect(() => {
    try {
      const channel = new BroadcastChannel("orx_remote_launch");
      channel.onmessage = (event) => {
        if (event.data?.url) {
          setTargetUrl(event.data.url);
          window.location.href = event.data.url;
        }
      };
      return () => channel.close();
    } catch {
      // BroadcastChannel optional across certain environments
    }
  }, []);

  return (
    <div className="flex min-h-screen flex-col items-center justify-center bg-background p-6 text-center text-text">
      <div className="flex max-w-md flex-col items-center gap-4 rounded-xl border border-border bg-surface p-8 shadow-sm">
        <Spinner />
        <h1 className="text-lg font-medium">{m.account_sign_in_alphaxiv()}</h1>
        <p className="text-sm text-subtext leading-relaxed">
          {m.account_finish_browser()}
        </p>
        {targetUrl ? (
          <a
            href={targetUrl}
            className="mt-2 inline-flex items-center justify-center rounded-md bg-accent px-4 py-2 text-sm font-medium text-white hover:opacity-90 focus-visible:outline-2 focus-visible:outline-focus"
          >
            Continue to alphaXiv
          </a>
        ) : (
          <p className="text-xs text-muted">
            Waiting for sign-in link… If you are not redirected automatically, check the OpenResearch window.
          </p>
        )}
      </div>
    </div>
  );
}
