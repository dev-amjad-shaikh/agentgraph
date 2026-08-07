import { Link } from "react-router";
import { ArrowRight, Clock } from "lucide-react";
import { articles } from "@/content/learn";

export function LearnIndex() {
  return (
    <div className="mx-auto w-full max-w-4xl px-4 py-14 sm:px-6 sm:py-20">
      {/* Editorial hero */}
      <header className="mb-12 sm:mb-16">
        <p className="mb-3 font-code text-xs uppercase tracking-[0.2em] text-primary">
          Documentation
        </p>
        <h1 className="font-display text-4xl leading-tight tracking-tight sm:text-5xl">
          Learn Rusty
        </h1>
        <p className="mt-5 max-w-2xl text-base leading-7 text-muted-foreground sm:text-lg sm:leading-8">
          Five articles that take you from the execution model to a served
          graph, human-in-the-loop, the zero-build debug UI, and the project's
          stability contract. Every command, identifier, and claim is traced to
          the source docs.
        </p>
      </header>

      {/* Article cards */}
      <div className="divide-y divide-border border-y border-border">
        {articles.map((article, i) => (
          <Link
            key={article.slug}
            to={`/learn/${article.slug}`}
            className="group flex gap-5 py-7 transition-colors first:pt-8 last:pb-8 sm:gap-8 sm:py-8"
          >
            <span
              aria-hidden
              className="font-display select-none text-3xl leading-none text-primary/35 transition-colors group-hover:text-primary/70 sm:text-4xl"
            >
              {String(i + 1).padStart(2, "0")}
            </span>
            <span className="min-w-0 flex-1">
              <span className="flex items-start justify-between gap-4">
                <span className="font-display text-xl leading-snug text-foreground transition-colors group-hover:text-primary sm:text-2xl">
                  {article.title}
                </span>
                <ArrowRight
                  size={18}
                  className="mt-1.5 shrink-0 text-muted-foreground/40 transition-all group-hover:translate-x-1 group-hover:text-primary"
                />
              </span>
              <span className="mt-2 block max-w-2xl text-sm leading-6 text-muted-foreground sm:text-[15px] sm:leading-7">
                {article.description}
              </span>
              <span className="mt-3 inline-flex items-center gap-1.5 font-code text-xs text-muted-foreground/80">
                <Clock size={12} />
                {article.readingTime}
              </span>
            </span>
          </Link>
        ))}
      </div>
    </div>
  );
}
