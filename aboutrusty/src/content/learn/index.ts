import type { Article } from "./types";
import { architecture } from "./architecture";
import { serverQuickstart } from "./serverQuickstart";
import { humanInTheLoop } from "./humanInTheLoop";
import { studio } from "./studio";
import { roadmapAndStability } from "./roadmapAndStability";

/** Articles in display order — index badges 01–05 derive from this order. */
export const articles: Article[] = [
  architecture,
  serverQuickstart,
  humanInTheLoop,
  studio,
  roadmapAndStability,
];

export function getArticle(slug: string | undefined): Article | undefined {
  return articles.find((a) => a.slug === slug);
}

export function getAdjacent(slug: string): {
  prev?: Article;
  next?: Article;
} {
  const i = articles.findIndex((a) => a.slug === slug);
  if (i === -1) return {};
  return { prev: articles[i - 1], next: articles[i + 1] };
}
