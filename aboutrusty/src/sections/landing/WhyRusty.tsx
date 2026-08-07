import { ShieldCheck, Package, Network, Globe } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { SectionHeading } from "./SectionHeading";

interface Reason {
  icon: LucideIcon;
  title: string;
  body: string;
}

const REASONS: Reason[] = [
  {
    icon: ShieldCheck,
    title: "Durability is a requirement, not a nicety.",
    body: "Every super-step boundary is checkpointed — resume after a crash, suspend for human approval, fork and replay any historical step.",
  },
  {
    icon: Package,
    title: "Deployment should be one binary.",
    body: "Your graphs compile into your server: no Python runtime, no Redis, no orchestration config file. Cargo.toml is the new langgraph.json.",
  },
  {
    icon: Network,
    title: "Your nodes aren't all in one place.",
    body: "Remote nodes execute graph steps on remote services over HTTP (interrupts cross the wire), and WasmNode runs untrusted modules in a Wasmtime sandbox with fuel and memory caps.",
  },
  {
    icon: Globe,
    title: "Your clients aren't Rust.",
    body: "The server is the interop layer: zero-dependency Python and TypeScript SDKs talk HTTP/SSE to it.",
  },
];

export function WhyRusty() {
  return (
    <section className="mx-auto max-w-6xl px-4 py-20 sm:px-6 sm:py-28">
      <SectionHeading
        eyebrow="Why Rusty exists"
        title="LangGraph's execution model, rebuilt on tokio."
        description="LangGraph proved the execution model: state channels with reducers, super-step parallelism, and checkpoints that turn durability, human-in-the-loop, and time travel into one primitive. Rusty rebuilds that model on tokio for teams who want it without operating a Python service."
      />
      <div className="mt-12 grid gap-5 sm:grid-cols-2">
        {REASONS.map((reason) => (
          <Card key={reason.title} className="bg-card">
            <CardHeader className="gap-3">
              <span className="flex h-9 w-9 items-center justify-center rounded-lg bg-accent text-accent-foreground">
                <reason.icon size={18} strokeWidth={2} />
              </span>
              <CardTitle className="font-display text-lg font-semibold leading-snug">
                {reason.title}
              </CardTitle>
            </CardHeader>
            <CardContent>
              <p className="text-sm leading-relaxed text-muted-foreground">
                {reason.body}
              </p>
            </CardContent>
          </Card>
        ))}
      </div>
    </section>
  );
}
