import { Link } from "react-router";
import { ArrowRight, Github } from "lucide-react";
import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/shared/CodeBlock";
import { SectionHeading } from "./SectionHeading";

const LOCAL_SETUP = `git clone https://github.com/dev-amjad-shaikh/rusty.git && cd rusty
./scripts/dev.sh        # local: Rusty Server on :8100 + Rusty Studio on :8000`;

const DOCKER_SETUP = `docker compose up       # the same pair, containerized`;

export function FinalCta() {
  return (
    <section className="relative overflow-hidden">
      <div
        aria-hidden="true"
        className="pointer-events-none absolute inset-x-0 bottom-0 h-[420px] bg-[radial-gradient(ellipse_60%_60%_at_50%_100%,hsl(var(--primary)/0.10),transparent_70%)]"
      />
      <div className="relative mx-auto max-w-6xl px-4 py-20 sm:px-6 sm:py-28">
        <SectionHeading
          eyebrow="Get started"
          title="Run your first graph in ten minutes."
          description="No Docker, no database, no Redis required — everything runs in one process. Or take the containerized path if you prefer."
        />
        <div className="mx-auto mt-12 grid max-w-3xl gap-6">
          <CodeBlock code={LOCAL_SETUP} language="bash" title="local" />
          <CodeBlock code={DOCKER_SETUP} language="bash" title="docker" />
        </div>
        <div className="mt-10 flex flex-col items-center justify-center gap-3 sm:flex-row">
          <Button asChild size="lg" className="gap-2">
            <Link to="/learn">
              Learn the architecture
              <ArrowRight size={16} />
            </Link>
          </Button>
          <Button asChild size="lg" variant="outline" className="gap-2">
            <a
              href="https://github.com/dev-amjad-shaikh/rusty"
              target="_blank"
              rel="noreferrer"
            >
              <Github size={16} />
              View on GitHub
            </a>
          </Button>
        </div>
      </div>
    </section>
  );
}
