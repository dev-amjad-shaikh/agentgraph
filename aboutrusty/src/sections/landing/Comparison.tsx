import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { SectionHeading } from "./SectionHeading";

const COLUMNS = [
  "Rusty",
  "LangGraph (framework)",
  "LangGraph Platform",
  "Rust LLM frameworks (rig, langchain-rust)",
];

interface ComparisonRow {
  aspect: string;
  values: [string, string, string, string];
}

const ROWS: ComparisonRow[] = [
  {
    aspect: "Language",
    values: ["Rust (tokio)", "Python (JS port available)", "Hosts LangGraph agents", "Rust"],
  },
  {
    aspect: "Execution model",
    values: [
      "Graph over schema-declared JSON state channels, Pregel/BSP super-steps",
      "State graphs, Pregel-inspired super-steps",
      "Same as the framework",
      "Provider / tool / agent abstractions; no checkpointed graph runtime",
    ],
  },
  {
    aspect: "Durability",
    values: [
      "Checkpoint at every super-step boundary; memory, JSON-file, or Postgres backends",
      "Pluggable savers: memory, SQLite, Postgres",
      "Managed persistence",
      "—",
    ],
  },
  {
    aspect: "Human-in-the-loop / time travel",
    values: [
      "Interrupt + resume; fork + replay from any checkpoint",
      "Interrupts; checkpoint time travel",
      "Yes",
      "—",
    ],
  },
  {
    aspect: "Deployment",
    values: [
      "Single static binary; library or server",
      "Your Python application",
      "Managed (from $35/mo) or enterprise self-host",
      "Library only",
    ],
  },
  {
    aspect: "Remote nodes / WASM sandbox",
    values: [
      "HTTP worker protocol, interrupts included / Wasmtime fuel + memory caps",
      "—",
      "—",
      "— (rig's core library is WASM-compatible)",
    ],
  },
  {
    aspect: "Server surface",
    values: [
      "Agent-Protocol subset: threads, runs, SSE, assistants, crons, KV, tenants",
      "— (library only)",
      "Full hosted platform",
      "—",
    ],
  },
  {
    aspect: "License",
    values: ["MIT OR Apache-2.0", "MIT", "Commercial", "MIT"],
  },
  {
    aspect: "Package registry",
    values: ["Not yet published", "PyPI / npm", "—", "crates.io"],
  },
];

export function Comparison() {
  return (
    <section className="mx-auto max-w-6xl px-4 py-20 sm:px-6 sm:py-28">
      <SectionHeading
        eyebrow="Honest comparison"
        title="How Rusty compares."
        description="Factual as of 2026-08-06; — means “not present, or not verified by us”. Sources: LangChain's checkpointing and time-travel documentation; third-party LangGraph pricing breakdown (2026-07); the rig project README."
      />
      <div className="mt-12 overflow-x-auto rounded-xl border bg-card shadow-sm">
        <Table className="min-w-[900px]">
          <TableHeader>
            <TableRow className="bg-muted/60">
              <TableHead className="w-48" />
              {COLUMNS.map((col, i) => (
                <TableHead
                  key={col}
                  className={i === 0 ? "font-semibold text-primary" : ""}
                >
                  {col}
                </TableHead>
              ))}
            </TableRow>
          </TableHeader>
          <TableBody>
            {ROWS.map((row) => (
              <TableRow key={row.aspect}>
                <TableCell className="align-top text-sm font-medium">
                  {row.aspect}
                </TableCell>
                {row.values.map((value, i) => (
                  <TableCell
                    key={i}
                    className={`align-top text-sm leading-relaxed ${
                      i === 0 ? "text-foreground" : "text-muted-foreground"
                    }`}
                  >
                    {value}
                  </TableCell>
                ))}
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
      <p className="mx-auto mt-8 max-w-2xl text-center text-sm leading-relaxed text-muted-foreground">
        If you want a batteries-included Python ecosystem or a fully managed
        control plane today, LangGraph and LangGraph Platform are further
        along.
      </p>
    </section>
  );
}
