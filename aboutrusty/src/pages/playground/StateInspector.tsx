import { Braces } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import type { ChannelState, Checkpoint, ScenarioDef } from "./engine";

interface StateInspectorProps {
  def: ScenarioDef;
  /** Currently inspected checkpoint, or null for live state. */
  checkpoint: Checkpoint | null;
  liveState: ChannelState;
  hasRun: boolean;
}

/**
 * Pretty JSON of the state channels. The messages channel (AddMessages
 * reducer) is highlighted — the ID-aware upsert that makes replay safe.
 */
export function StateInspector({ def, checkpoint, liveState, hasRun }: StateInspectorProps) {
  const state = checkpoint ? checkpoint.state : liveState;

  if (!hasRun && !checkpoint) {
    return (
      <div className="flex h-48 flex-col items-center justify-center gap-2 text-center">
        <Braces size={22} className="text-muted-foreground/50" />
        <p className="text-sm text-muted-foreground">
          No state yet — press Run or Step to populate the channels.
        </p>
      </div>
    );
  }

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <p className="font-code text-[11px] text-muted-foreground">
          {checkpoint
            ? `state @ ${checkpoint.checkpoint_id} · step ${checkpoint.step}`
            : "live state"}
        </p>
        {checkpoint && (
          <Badge variant="outline" className="font-code text-[10px]">
            snapshot
          </Badge>
        )}
      </div>
      {def.channels.map((ch) => {
        const value = state[ch.name];
        const isMessages = ch.name === "messages";
        return (
          <div
            key={ch.name}
            className={`rounded-lg border p-3 ${
              isMessages ? "border-primary/40 bg-accent/30" : "bg-card"
            }`}
          >
            <div className="mb-1.5 flex items-center gap-2">
              <span className="font-code text-xs font-semibold">{ch.name}</span>
              <Badge
                variant={isMessages ? "default" : "secondary"}
                className="font-code text-[10px]"
              >
                {ch.reducer}
              </Badge>
              {isMessages && (
                <span className="text-[10px] text-muted-foreground">
                  ID-aware upsert — replay never duplicates a message
                </span>
              )}
            </div>
            <pre className="max-h-64 overflow-auto rounded-md bg-code p-3 font-code text-[11.5px] leading-relaxed">
              {value === undefined ? "—" : JSON.stringify(value, null, 2)}
            </pre>
          </div>
        );
      })}
    </div>
  );
}
