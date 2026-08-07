import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { SectionHeading } from "./SectionHeading";

interface ComponentRow {
  piece: string;
  path: string;
  description: string;
}

const COMPONENTS: ComponentRow[] = [
  {
    piece: "Rusty Core",
    path: "rusty-core/ (rusty-agent-runtime)",
    description:
      "The engine: state channels + reducers, graph builder, super-step executor, checkpoints (memory / JSON file / Postgres), interrupts, Send fan-out, prebuilt ReAct agent, MCP client, remote nodes, WASM nodes. No HTTP.",
  },
  {
    piece: "Rusty Server",
    path: "rusty-server/",
    description:
      "axum HTTP/SSE server: threads, background / blocking / streaming runs, checkpoint history, fork + replay, assistants, crons, KV store, multi-tenant API-key auth.",
  },
  {
    piece: "Rusty Worker",
    path: "rusty-worker/",
    description:
      "Worker SDK: serves your node handlers over HTTP so RemoteNode can execute them remotely.",
  },
  {
    piece: "Rusty OTel",
    path: "rusty-otel/",
    description:
      "One-call tracing subscriber setup with optional OTLP span export.",
  },
  {
    piece: "Rusty Studio",
    path: "studio/",
    description:
      "Zero-build debug UI: connect, run, stream, inspect state and checkpoint history, fork and replay.",
  },
  {
    piece: "Rusty SDKs",
    path: "sdks/python/ · sdks/typescript/",
    description:
      "Zero-dependency rusty_client (Python) and @rusty-runtime/client (TypeScript) clients for the server API.",
  },
];

export function ComponentsTable() {
  return (
    <section className="border-y bg-secondary/40">
      <div className="mx-auto max-w-6xl px-4 py-20 sm:px-6 sm:py-28">
        <SectionHeading
          eyebrow="Components"
          title="Four crates, a studio, and two SDKs."
          description="Packages version independently. The crates are implemented but not yet published to any registry — crates.io / npm / PyPI publishing is on the R1.0 roadmap."
        />
        <div className="mt-12 overflow-hidden rounded-xl border bg-card shadow-sm">
          <Table>
            <TableHeader>
              <TableRow className="bg-muted/60">
                <TableHead className="w-40">Piece</TableHead>
                <TableHead className="w-64">Path</TableHead>
                <TableHead>What it is</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {COMPONENTS.map((row) => (
                <TableRow key={row.piece}>
                  <TableCell className="align-top font-medium">
                    {row.piece}
                  </TableCell>
                  <TableCell className="align-top font-code text-xs text-muted-foreground">
                    {row.path}
                  </TableCell>
                  <TableCell className="align-top text-sm leading-relaxed text-muted-foreground">
                    {row.description}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      </div>
    </section>
  );
}
