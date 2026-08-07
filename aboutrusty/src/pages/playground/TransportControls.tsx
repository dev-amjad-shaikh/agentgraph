import { Gauge, Pause, Play, RotateCcw, StepForward } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Kbd } from "@/components/ui/kbd";
import { Separator } from "@/components/ui/separator";
import { PHASES, type Phase, type ThreadStatus } from "./engine";

interface TransportControlsProps {
  status: ThreadStatus;
  step: number;
  phase: Phase | null;
  speed: 1 | 2;
  canRun: boolean;
  canStep: boolean;
  onRun: () => void;
  onPause: () => void;
  onStep: () => void;
  onReset: () => void;
  onSpeed: (s: 1 | 2) => void;
}

const STATUS_STYLES: Record<ThreadStatus, string> = {
  idle: "bg-muted text-muted-foreground",
  running: "bg-primary text-primary-foreground",
  paused: "bg-accent text-accent-foreground",
  interrupted: "bg-amber-600/15 text-amber-700 border border-amber-600/40",
  done: "bg-emerald-700/10 text-emerald-800 border border-emerald-700/30",
};

/**
 * Run / Pause / Step / Reset + speed, with the six-beat super-step loop
 * (plan → parallel → barrier → merge → route → checkpoint) lighting up as
 * each phase passes. The barrier makes the whole step transactional.
 */
export function TransportControls({
  status,
  step,
  phase,
  speed,
  canRun,
  canStep,
  onRun,
  onPause,
  onStep,
  onReset,
  onSpeed,
}: TransportControlsProps) {
  const running = status === "running";
  const phaseIdx = phase ? PHASES.indexOf(phase) : -1;

  return (
    <Card>
      <CardHeader className="pb-3">
        <div className="flex items-center justify-between gap-2">
          <CardTitle className="text-base">Transport</CardTitle>
          <Badge className={STATUS_STYLES[status]} variant="secondary">
            {status}
          </Badge>
        </div>
        <CardDescription>
          One super-step = plan → parallel → barrier → merge → route →
          checkpoint. Transactional as a whole.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex flex-wrap items-center gap-2">
          {running ? (
            <Button size="sm" variant="secondary" onClick={onPause}>
              <Pause size={14} className="mr-1.5" />
              Pause
              <Kbd className="ml-2">R</Kbd>
            </Button>
          ) : (
            <Button size="sm" onClick={onRun} disabled={!canRun}>
              <Play size={14} className="mr-1.5" />
              Run
              <Kbd className="ml-2">R</Kbd>
            </Button>
          )}
          <Button
            size="sm"
            variant="outline"
            onClick={onStep}
            disabled={!canStep}
            title="Advance exactly one super-step"
          >
            <StepForward size={14} className="mr-1.5" />
            Step
            <Kbd className="ml-2">S</Kbd>
          </Button>
          <Button size="sm" variant="ghost" onClick={onReset}>
            <RotateCcw size={14} className="mr-1.5" />
            Reset
          </Button>
          <div className="ml-auto flex items-center gap-1">
            <Gauge size={14} className="text-muted-foreground" />
            {([1, 2] as const).map((s) => (
              <Button
                key={s}
                size="sm"
                variant={speed === s ? "secondary" : "ghost"}
                className="h-7 px-2 font-code text-xs"
                onClick={() => onSpeed(s)}
              >
                {s}×
              </Button>
            ))}
          </div>
        </div>

        <Separator />

        <div aria-label="Super-step phases" className="space-y-2">
          <div className="flex flex-wrap items-center gap-y-1.5">
            {PHASES.map((p, i) => {
              const isCurrent = i === phaseIdx;
              const isPast = phaseIdx > i;
              return (
                <span key={p} className="flex items-center">
                  <span
                    className={`rounded-md px-2 py-1 font-code text-[11px] transition-colors ${
                      isCurrent
                        ? "bg-primary text-primary-foreground shadow-sm"
                        : isPast
                          ? "bg-accent text-accent-foreground"
                          : "bg-muted/60 text-muted-foreground"
                    }`}
                  >
                    {p}
                  </span>
                  {i < PHASES.length - 1 && (
                    <span className="mx-1 text-[10px] text-muted-foreground">→</span>
                  )}
                </span>
              );
            })}
          </div>
          <p className="font-code text-[11px] text-muted-foreground">
            {phase
              ? phase === "parallel"
                ? "nodes run on immutable snapshots — they cannot see each other's writes"
                : phase === "barrier"
                  ? "all-or-nothing: writes become visible only here"
                  : phase === "merge"
                    ? "reducers merge partial updates into the channels"
                    : phase === "route"
                      ? "static edges, Command::goto, or Route decide the next active set"
                      : phase === "checkpoint"
                        ? "a versioned checkpoint is persisted at the step boundary"
                        : "the executor plans the active set for this super-step"
              : `super-step ${step} — press Run or Step`}
          </p>
        </div>
      </CardContent>
    </Card>
  );
}
