import { GitBranch } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import type { ThreadSim } from "./engine";

interface BranchesPanelProps {
  threads: ThreadSim[];
  activeId: string;
  onSelect: (id: string) => void;
}

/**
 * Threads = timelines. fork_thread copies a thread's history (oldest first,
 * up to the chosen checkpoint) into a new thread id; replay then runs from
 * that checkpoint's state and next-node set — two divergent histories.
 */
export function BranchesPanel({ threads, activeId, onSelect }: BranchesPanelProps) {
  return (
    <Card>
      <CardHeader className="pb-3">
        <CardTitle className="flex items-center gap-2 text-base">
          <GitBranch size={15} className="text-primary" />
          Timelines
        </CardTitle>
        <CardDescription>
          A thread namespaces checkpoints. Fork one at a checkpoint to branch
          history, then replay on the fork.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <ol className="space-y-1.5">
          {threads.map((t) => {
            const active = t.id === activeId;
            return (
              <li key={t.id}>
                <button
                  type="button"
                  onClick={() => onSelect(t.id)}
                  className={`flex w-full items-center gap-2 rounded-lg border px-3 py-2 text-left transition-colors ${
                    active
                      ? "border-primary/50 bg-accent/40"
                      : "bg-card hover:bg-secondary/60"
                  }`}
                >
                  <span className="font-code text-xs font-medium">{t.id}</span>
                  <Badge
                    variant={t.persona === "main" ? "secondary" : "default"}
                    className="font-code text-[10px]"
                  >
                    {t.persona === "main" ? "main" : "fork"}
                  </Badge>
                  {t.forkedFrom && (
                    <span className="min-w-0 truncate font-code text-[10px] text-muted-foreground">
                      from {t.forkedFrom.thread} @ {t.forkedFrom.checkpoint}
                    </span>
                  )}
                  <span className="ml-auto shrink-0 font-code text-[10px] text-muted-foreground">
                    {t.checkpoints.length} cp · {t.status}
                  </span>
                </button>
              </li>
            );
          })}
        </ol>
      </CardContent>
    </Card>
  );
}
