import type { ReactNode } from "react";
import { Link, useParams } from "react-router";
import { ArrowLeft, ArrowRight, ChevronRight, Clock } from "lucide-react";
import { CodeBlock } from "@/components/shared/CodeBlock";
import {
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Separator } from "@/components/ui/separator";
import { getAdjacent, getArticle } from "@/content/learn";
import type { ContentBlock } from "@/content/learn/types";

/** Render inline `code` and **bold** markers inside plain text. */
function renderInline(text: string): ReactNode[] {
  const tokenRe = /(\*\*[^*]+\*\*|`[^`]+`)/g;
  return text.split(tokenRe).map((seg, i) => {
    if (seg.startsWith("**") && seg.endsWith("**") && seg.length > 4) {
      return (
        <strong key={i} className="font-semibold text-foreground">
          {seg.slice(2, -2)}
        </strong>
      );
    }
    if (seg.startsWith("`") && seg.endsWith("`") && seg.length > 2) {
      return (
        <code
          key={i}
          className="rounded bg-muted px-1.5 py-0.5 font-code text-[0.82em] text-foreground"
        >
          {seg.slice(1, -1)}
        </code>
      );
    }
    return seg;
  });
}

function BlockRenderer({ block }: { block: ContentBlock }) {
  switch (block.type) {
    case "heading":
      return block.level === 2 ? (
        <h2 className="mb-4 mt-12 font-display text-2xl tracking-tight first:mt-0 sm:text-[1.7rem]">
          {block.text}
        </h2>
      ) : (
        <h3 className="mb-3 mt-10 font-display text-xl tracking-tight first:mt-0">
          {block.text}
        </h3>
      );

    case "paragraph":
      return (
        <p className="mb-5 leading-7 text-foreground/85 sm:leading-8">
          {renderInline(block.text)}
        </p>
      );

    case "list": {
      const items = block.items.map((item, i) => (
        <li key={i} className="leading-7 text-foreground/85">
          {renderInline(item)}
        </li>
      ));
      return block.ordered ? (
        <ol className="mb-6 list-decimal space-y-2.5 pl-6 marker:font-code marker:text-sm marker:text-primary/70">
          {items}
        </ol>
      ) : (
        <ul className="mb-6 list-disc space-y-2.5 pl-6 marker:text-primary/60">
          {items}
        </ul>
      );
    }

    case "code":
      return (
        <div className="mb-6">
          <CodeBlock
            code={block.code}
            language={block.language}
            title={block.title}
          />
        </div>
      );

    case "table":
      return (
        <div className="mb-6 overflow-hidden rounded-lg border border-border bg-card">
          <Table>
            {block.caption && <TableCaption>{block.caption}</TableCaption>}
            <TableHeader>
              <TableRow className="bg-muted/60 hover:bg-muted/60">
                {block.head.map((h, i) => (
                  <TableHead
                    key={i}
                    className="h-auto whitespace-normal px-3 py-2.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground"
                  >
                    {h}
                  </TableHead>
                ))}
              </TableRow>
            </TableHeader>
            <TableBody>
              {block.rows.map((row, i) => (
                <TableRow key={i}>
                  {row.map((cell, j) => (
                    <TableCell
                      key={j}
                      className="whitespace-normal px-3 py-2.5 align-top text-sm leading-6"
                    >
                      {renderInline(cell)}
                    </TableCell>
                  ))}
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      );

    case "callout": {
      if (block.variant === "quote") {
        return (
          <figure className="mb-6 rounded-r-lg border-l-4 border-primary bg-accent/50 px-5 py-4">
            <blockquote className="font-display text-[1.05rem] italic leading-relaxed text-accent-foreground">
              {renderInline(block.text)}
            </blockquote>
          </figure>
        );
      }
      const isWarning = block.variant === "warning";
      return (
        <aside
          className={`mb-6 rounded-r-lg border-l-4 px-5 py-4 ${
            isWarning
              ? "border-amber-700/70 bg-amber-100/50"
              : "border-primary/50 bg-accent/40"
          }`}
        >
          {block.title && (
            <p
              className={`mb-1.5 text-sm font-semibold ${
                isWarning ? "text-amber-900" : "text-accent-foreground"
              }`}
            >
              {block.title}
            </p>
          )}
          <p
            className={`text-sm leading-6 ${
              isWarning ? "text-amber-900/90" : "text-foreground/85"
            }`}
          >
            {renderInline(block.text)}
          </p>
        </aside>
      );
    }
  }
}

export function LearnArticle() {
  const { slug } = useParams<{ slug: string }>();
  const article = getArticle(slug);

  if (!article) {
    return (
      <div className="mx-auto w-full max-w-3xl px-4 py-20 text-center sm:px-6">
        <h1 className="font-display text-3xl">Article not found</h1>
        <p className="mt-4 text-muted-foreground">
          There is no Learn article at this address.
        </p>
        <Link
          to="/learn"
          className="mt-6 inline-flex items-center gap-2 text-primary underline-offset-4 hover:underline"
        >
          <ArrowLeft size={16} />
          Back to Learn
        </Link>
      </div>
    );
  }

  const { prev, next } = getAdjacent(article.slug);

  return (
    <div className="mx-auto w-full max-w-3xl px-4 py-10 sm:px-6 sm:py-14">
      {/* Breadcrumb */}
      <nav
        aria-label="Breadcrumb"
        className="mb-8 flex items-center gap-1.5 text-sm text-muted-foreground"
      >
        <Link
          to="/learn"
          className="transition-colors hover:text-primary"
        >
          Learn
        </Link>
        <ChevronRight size={14} className="text-muted-foreground/50" />
        <span className="truncate text-foreground/70">{article.title}</span>
      </nav>

      {/* Header */}
      <header className="mb-8">
        <h1 className="font-display text-3xl leading-tight tracking-tight sm:text-4xl">
          {article.title}
        </h1>
        <p className="mt-4 text-base leading-7 text-muted-foreground sm:text-lg sm:leading-8">
          {article.description}
        </p>
        <p className="mt-4 inline-flex items-center gap-1.5 font-code text-xs text-muted-foreground/80">
          <Clock size={12} />
          {article.readingTime}
        </p>
      </header>

      <Separator className="mb-10" />

      {/* Content blocks */}
      <div>
        {article.blocks.map((block, i) => (
          <BlockRenderer key={i} block={block} />
        ))}
      </div>

      {/* Prev / next */}
      <nav
        aria-label="More articles"
        className="mt-14 grid grid-cols-1 gap-4 border-t border-border pt-8 sm:grid-cols-2"
      >
        {prev ? (
          <Link
            to={`/learn/${prev.slug}`}
            className="group rounded-lg border border-border p-4 transition-colors hover:border-primary/40 hover:bg-accent/30"
          >
            <span className="flex items-center gap-1.5 font-code text-xs uppercase tracking-wider text-muted-foreground">
              <ArrowLeft
                size={13}
                className="transition-transform group-hover:-translate-x-0.5"
              />
              Previous
            </span>
            <span className="mt-2 block font-display text-base leading-snug text-foreground transition-colors group-hover:text-primary">
              {prev.title}
            </span>
          </Link>
        ) : (
          <span className="hidden sm:block" />
        )}
        {next ? (
          <Link
            to={`/learn/${next.slug}`}
            className="group rounded-lg border border-border p-4 text-right transition-colors hover:border-primary/40 hover:bg-accent/30"
          >
            <span className="flex items-center justify-end gap-1.5 font-code text-xs uppercase tracking-wider text-muted-foreground">
              Next
              <ArrowRight
                size={13}
                className="transition-transform group-hover:translate-x-0.5"
              />
            </span>
            <span className="mt-2 block font-display text-base leading-snug text-foreground transition-colors group-hover:text-primary">
              {next.title}
            </span>
          </Link>
        ) : (
          <span className="hidden sm:block" />
        )}
      </nav>
    </div>
  );
}
