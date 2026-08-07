import { useState } from "react";
import { Check, Copy } from "lucide-react";

interface CodeBlockProps {
  code: string;
  language?: string;
  title?: string;
  className?: string;
}

/**
 * Shared dark warm-charcoal code block with optional title bar and copy button.
 * Used across landing, learn, and playground pages — keep it dependency-free.
 */
export function CodeBlock({ code, language, title, className = "" }: CodeBlockProps) {
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      /* clipboard unavailable — ignore */
    }
  };

  return (
    <div
      className={`bg-code overflow-hidden rounded-xl border border-black/40 shadow-lg ${className}`}
    >
      {(title || language) && (
        <div className="flex items-center justify-between border-b border-white/10 px-4 py-2">
          <span className="font-code text-xs text-white/60">{title ?? ""}</span>
          <div className="flex items-center gap-3">
            {language && (
              <span className="font-code text-[10px] uppercase tracking-wider text-white/40">
                {language}
              </span>
            )}
            <button
              onClick={copy}
              aria-label="Copy code"
              className="text-white/50 transition-colors hover:text-white/90"
            >
              {copied ? <Check size={14} /> : <Copy size={14} />}
            </button>
          </div>
        </div>
      )}
      <pre className="overflow-x-auto p-4">
        <code className="font-code text-[13px] leading-relaxed">{code}</code>
      </pre>
    </div>
  );
}
