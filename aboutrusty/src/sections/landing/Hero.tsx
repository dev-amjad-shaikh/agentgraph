import { Link } from "react-router";
import { ArrowRight } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/shared/CodeBlock";

const INSTALL_SNIPPET = `[dependencies]
rusty-agent-runtime = { git = "https://github.com/dev-amjad-shaikh/rusty" }
tokio = { version = "1", features = ["full"] }
async-trait = "0.1"
serde_json = "1"`;

const FACT_BADGES = [
  "v0.x active development",
  "MIT OR Apache-2.0",
  "MSRV 1.86",
  "tokio",
];

export function Hero() {
  return (
    <section className="relative overflow-hidden">
      {/* subtle radial warm glow behind the headline */}
      <div
        aria-hidden="true"
        className="pointer-events-none absolute inset-x-0 top-0 h-[560px] bg-[radial-gradient(ellipse_60%_55%_at_50%_0%,hsl(var(--primary)/0.10),transparent_70%)]"
      />
      <div className="relative mx-auto max-w-6xl px-4 pb-20 pt-20 sm:px-6 sm:pb-28 sm:pt-28">
        <div className="flex flex-col items-center text-center">
          <div className="flex flex-wrap items-center justify-center gap-2">
            {FACT_BADGES.map((fact) => (
              <Badge
                key={fact}
                variant="secondary"
                className="font-code text-[11px] font-normal"
              >
                {fact}
              </Badge>
            ))}
          </div>

          <h1 className="mt-8 max-w-3xl font-display text-4xl font-semibold leading-[1.1] tracking-tight sm:text-5xl lg:text-6xl">
            The durable agent runtime, built in Rust.
          </h1>

          <p className="mt-6 max-w-2xl text-base leading-relaxed text-muted-foreground sm:text-lg">
            Define an agent as a graph over schema-declared JSON state. The
            engine executes it in transactional super-steps and writes a
            versioned checkpoint at every step boundary — then runs embedded in
            your process, as one static binary server, or across remote and
            WASM nodes.
          </p>

          <div className="mt-10 flex flex-col items-center gap-3 sm:flex-row">
            <Button asChild size="lg" className="gap-2">
              <Link to="/playground">
                Try the Playground
                <ArrowRight size={16} />
              </Link>
            </Button>
            <Button asChild size="lg" variant="outline">
              <Link to="/learn">Learn the architecture</Link>
            </Button>
          </div>

          <div className="mt-12 w-full max-w-2xl text-left">
            <CodeBlock
              code={INSTALL_SNIPPET}
              language="toml"
              title="Cargo.toml"
            />
            <p className="mt-3 text-center text-xs text-muted-foreground">
              Registry publishing is pending — depend on the git repo for now.
              Once published, this becomes{" "}
              <code className="font-code">cargo add rusty-agent-runtime</code>.
            </p>
          </div>
        </div>
      </div>
    </section>
  );
}
