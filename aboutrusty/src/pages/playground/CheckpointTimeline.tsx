import { GitFork, History } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { Checkpoint } from "./engine";

interface CheckpointTimelineProps {
  checkpoints: Checkpoint[];
  selectedId: string | null;
  canFork: boolean;
  onSelect: (id: string | null) => void;
  onFork: (cp: Checkpoint) => void;
}

/**
 * Versioned checkpoints — one at every super-step boundary (step, full
 * channel state, next-node set). Click one to inspect its snapshot; fork
 * from it to branch the timeline.
 */
export function CheckpointTimeline({
  checkpoints,
  selectedId,
  canFork,
  onSelect,
  onFork,
}: CheckpointTimelineProps) {
  if (checkpoints.length === 0) {
    return (
      <div className="flex h-48 flex-col items-center justify-center gap-2 text-center">
        <History size={22} className="text-muted-foreground/50" />
        <p className="text-sm text-muted-foreground">
          Checkpoints appear here at every super-step boundary.
        </p>
      </div>
    );
  }

  const newestFirst = [...checkpoints].reverse();

  return (
    <div className="space-y-2">
      <p className="font-code text-[11px] text-muted-foreground">
        newest first · click to inspect the snapshot ·{" "}
        <GitFork size={11} className="inline" /> fork from that boundary
      </p>
      <ol className="space-y-1.5">
        {newestFirst.map((cp) => {
          const selected = cp.checkpoint_id === selectedId;
          return (
            <li key={cp.checkpoint_id}>
              <div
                className={`flex w-full items-center gap-2 rounded-lg border px-3 py-2 text-left transition-colors ${
                  selected
                    ? "border-primary/50 bg-accent/40"
                    : "bg-card hover:bg-secondary/60"
                }`}
              >
                <button
                  type="button"
                  className="flex min-w-0 flex-1 items-center gap-2"
                  onClick={() => onSelect(selected ? null : cp.checkpoint_id)}
                >
                  <Badge
                    variant={cp.kind === "suspension" ? "outline" : "secondary"}
                    className={`shrink-0 font-code text-[10px] ${
                      cp.kind === "suspension"
                        ? "border-amber-600/50 text-amber-700"
                        : ""
                    }`}
                  >
                    step {cp.step}
                  </Badge>
                  <span className="font-code text-[11px] text-muted-foreground">
                    {cp.checkpoint_id}
                  </span>
                  <span className="min-w-0 truncate font-code text-[11px]">
                    {cp.nodes.join(", ")}
                    {cp.kind === "suspension" && " · suspended"}
                  </span>
                  <span className="ml-auto hidden shrink-0 font-code text-[10px] text-muted-foreground sm:inline">
                    next [{cp.next.join(", ")}] · t+{cp.clock}
                  </span>
                </button>
                <Button
                  size="sm"
                  variant="ghost"
                  className="h-7 shrink-0 px-2"
                  disabled={!canFork}
                  title={
                    canFork
                      ? "fork_thread: copy history up to this checkpoint into a new thread"
                      : "Pause the run before forking"
                  }
                  onClick={() => onFork(cp)}
                >
                  <GitFork size={13} className="mr-1" />
                  <span className="font-code text-[11px]">fork</span>
                </Button>
              </div>
            </li>
          );
        })}
      </ol>
      <div className="rounded-lg border border-primary/30 bg-accent/30 px-3 py-2 text-xs leading-relaxed text-accent-foreground">
        <strong>Fork first, replay on the fork.</strong> Replaying on the
        original thread appends new checkpoints on top of the old timeline —
        supported, but rarely what you want.
      </div>
    </div>
  );
}
