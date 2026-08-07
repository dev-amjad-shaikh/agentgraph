/**
 * Typed content model for the Learn section.
 * Articles are ordered lists of blocks — no markdown library involved.
 * Inline text supports `code` and **bold** markers, rendered by LearnArticle.
 */

export type HeadingBlock = {
  type: "heading";
  level: 2 | 3;
  text: string;
};

export type ParagraphBlock = {
  type: "paragraph";
  text: string;
};

export type ListBlock = {
  type: "list";
  ordered?: boolean;
  items: string[];
};

export type CodeBlockData = {
  type: "code";
  language: string;
  title?: string;
  code: string;
};

export type TableBlock = {
  type: "table";
  head: string[];
  rows: string[][];
  caption?: string;
};

export type CalloutVariant = "quote" | "note" | "warning";

export type CalloutBlock = {
  type: "callout";
  variant: CalloutVariant;
  title?: string;
  text: string;
};

export type ContentBlock =
  | HeadingBlock
  | ParagraphBlock
  | ListBlock
  | CodeBlockData
  | TableBlock
  | CalloutBlock;

export interface Article {
  slug: string;
  title: string;
  description: string;
  readingTime: string;
  blocks: ContentBlock[];
}
