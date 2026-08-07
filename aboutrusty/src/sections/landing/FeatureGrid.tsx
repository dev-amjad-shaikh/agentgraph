import {
  Database,
  Layers,
  Save,
  Hand,
  History,
  Network,
  Box,
  Plug,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { SectionHeading } from "./SectionHeading";

interface Feature {
  icon: LucideIcon;
  title: string;
  body: string;
}

const FEATURES: Feature[] = [
  {
    icon: Database,
    title: "State channels & 4 reducers",
    body: "Every state key is a versioned channel: Overwrite, Append, DeepMerge, or AddMessages (ID-aware message upsert). Writes to undeclared channels are rejected.",
  },
  {
    icon: Layers,
    title: "Super-step executor",
    body: "Pregel/BSP execution — plan → parallel → barrier → merge → route — with each step transactional and a max_steps guard on cycles.",
  },
  {
    icon: Save,
    title: "Checkpoints",
    body: "A versioned checkpoint at every super-step boundary, on memory, JSON-file, or Postgres backends. Durable execution from one primitive.",
  },
  {
    icon: Hand,
    title: "Interrupts + human-in-the-loop",
    body: "ctx.interrupt(payload) suspends a run durably; resume with a human decision via command.resume — over Rust or over HTTP.",
  },
  {
    icon: History,
    title: "Fork & replay time travel",
    body: "Branch any thread at any historical checkpoint and replay from it. Fork first, replay on the fork.",
  },
  {
    icon: Network,
    title: "RemoteNode over HTTP",
    body: "Execute graph steps on remote worker services. HITL interrupts cross the wire — a remote node can suspend the whole run.",
  },
  {
    icon: Box,
    title: "WasmNode sandbox",
    body: "Run untrusted WebAssembly modules as nodes in a Wasmtime sandbox with fuel and memory caps — same Node trait, no worker fleet.",
  },
  {
    icon: Plug,
    title: "MCP client",
    body: "Call any MCP server's tools from Rusty Tool impls over stdio; MCP servers register into the ToolRegistry like native tools.",
  },
];

export function FeatureGrid() {
  return (
    <section className="mx-auto max-w-6xl px-4 py-20 sm:px-6 sm:py-28">
      <SectionHeading
        eyebrow="Capabilities"
        title="The LangGraph quartet — and then some."
        description="State graph, durable checkpointing, interrupts, and resumable execution as first-class primitives, plus remote and sandboxed WASM nodes."
      />
      <div className="mt-12 grid gap-x-8 gap-y-10 sm:grid-cols-2 lg:grid-cols-4">
        {FEATURES.map((feature) => (
          <div key={feature.title} className="flex flex-col gap-3">
            <span className="flex h-9 w-9 items-center justify-center rounded-lg bg-accent text-accent-foreground">
              <feature.icon size={18} strokeWidth={2} />
            </span>
            <h3 className="text-sm font-semibold tracking-tight">
              {feature.title}
            </h3>
            <p className="text-sm leading-relaxed text-muted-foreground">
              {feature.body}
            </p>
          </div>
        ))}
      </div>
    </section>
  );
}
