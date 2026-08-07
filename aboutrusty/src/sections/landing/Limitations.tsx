import { AlertTriangle } from "lucide-react";
import { SectionHeading } from "./SectionHeading";

interface Limitation {
  title: string;
  body: string;
}

const LIMITATIONS: Limitation[] = [
  {
    title: "Single-node executor.",
    body: "One process runs the super-step loop. Remote nodes distribute node work, but the executor itself is not clustered and has no failover.",
  },
  {
    title: "No durable queue.",
    body: "Queued runs live in an in-memory per-thread FIFO; a server restart drops pending (not-yet-started) runs. Durable queues and autoscaling are open R1.0 items.",
  },
  {
    title: "Persistence is single-node.",
    body: "The core executor checkpoints only when you attach a Checkpointer; InMemoryCheckpointer is for dev/test and loses state on restart. On the server, checkpoints and the assistants / crons / KV store default to JSON files on local disk — Postgres requires the postgres feature, and there is no replication either way.",
  },
  {
    title: "Idempotency contract.",
    body: "Checkpoints happen at step boundaries, never mid-node: resume re-executes a node from its start, so node logic must be idempotent.",
  },
  {
    title: "Open by default in dev.",
    body: "With no API keys configured the server runs unauthenticated, and its CORS layer is permissive — restrict both before exposing it to a network.",
  },
];

export function Limitations() {
  return (
    <section className="border-y bg-secondary/40">
      <div className="mx-auto max-w-6xl px-4 py-20 sm:px-6 sm:py-28">
        <SectionHeading
          eyebrow="Known limitations"
          title="What v0.x is not."
          description="Rusty is explicit about its current edges. We would rather you know them now than discover them in production."
        />
        <div className="mx-auto mt-12 max-w-3xl rounded-xl border border-primary/25 bg-card p-6 shadow-sm sm:p-8">
          <div className="flex items-center gap-3">
            <span className="flex h-9 w-9 items-center justify-center rounded-lg bg-accent text-accent-foreground">
              <AlertTriangle size={18} strokeWidth={2} />
            </span>
            <h3 className="font-display text-lg font-semibold tracking-tight">
              Production readiness, stated plainly
            </h3>
          </div>
          <ul className="mt-6 space-y-5">
            {LIMITATIONS.map((item) => (
              <li key={item.title} className="flex gap-3">
                <span
                  aria-hidden="true"
                  className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-primary"
                />
                <p className="text-sm leading-relaxed text-muted-foreground">
                  <span className="font-semibold text-foreground">
                    {item.title}
                  </span>{" "}
                  {item.body}
                </p>
              </li>
            ))}
          </ul>
          <p className="mt-6 border-t pt-5 text-sm leading-relaxed text-muted-foreground">
            <span className="font-semibold text-foreground">
              Deliberately rejected:
            </span>{" "}
            PyO3 / napi-rs bindings and a cdylib / C ABI — the HTTP/SSE server
            is the polyglot interop layer instead.
          </p>
        </div>
      </div>
    </section>
  );
}
