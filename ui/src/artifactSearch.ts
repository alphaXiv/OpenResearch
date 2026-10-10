import type { ArtifactSearchJob } from "./api";

type Client = {
  start: (q: string, after?: string) => Promise<ArtifactSearchJob>;
  status: (id: string) => Promise<ArtifactSearchJob>;
  resume: (id: string) => Promise<ArtifactSearchJob>;
  cancel: (id: string) => Promise<unknown>;
};

/** One page's lifetime, including a start response arriving after unmount.
 * No automatic retries/restarts: resume always addresses the same server job. */
export class ArtifactSearchSession {
  private job: ArtifactSearchJob | null = null;
  private disposed = false;
  private version = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  constructor(private client: Client, private publish: (job: ArtifactSearchJob) => void, private fail: (error: Error) => void) {}

  async start(query: string, after?: string) {
    try {
      const job = await this.client.start(query, after);
      this.job = job;
      if (this.disposed) { void this.client.cancel(job.id).catch(() => {}); return; }
      this.accept(job);
    } catch (error) { this.failed(error); }
  }

  private accept(job: ArtifactSearchJob) {
    if (this.disposed) return;
    this.job = job;
    this.publish(job);
    clearTimeout(this.timer);
    if (job.status === "running" || job.status === "paused") {
      // Paused searches still renew their lease while the panel is open.
      this.timer = setTimeout(() => void this.poll(), job.status === "paused" ? 2_000 : 250);
    }
  }

  private async poll() {
    if (!this.job || this.disposed) return;
    const version = this.version;
    try {
      const job = await this.client.status(this.job.id);
      if (version === this.version) this.accept(job);
    } catch (error) { if (version === this.version) this.failed(error); }
  }

  async resume() {
    if (!this.job || this.disposed || this.job.status !== "paused") return;
    this.version++;
    clearTimeout(this.timer);
    this.job = { ...this.job, status: "running" };
    this.publish(this.job);
    try { this.accept(await this.client.resume(this.job.id)); }
    catch (error) { this.failed(error); }
  }

  cancel() {
    if (this.job && !this.disposed) this.publish({ ...this.job, status: "cancelled" });
    this.dispose();
  }

  dispose() {
    if (this.disposed) return;
    this.disposed = true;
    this.version++;
    clearTimeout(this.timer);
    if (this.job) void this.client.cancel(this.job.id).catch(() => {});
  }

  private failed(error: unknown) {
    if (!this.disposed) {
      this.fail(error instanceof Error ? error : new Error(String(error)));
      this.dispose();
    }
  }
}
