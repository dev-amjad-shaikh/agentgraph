import { useEffect, useRef } from "react";
import { Terminal } from "lucide-react";
import type { SimFrame } from "./engine";

interface EventLogProps {
  frames: SimFrame[];
  threadId: string;
}

const FRAME_COLORS: Record<SimFrame["event"], string> = {
  metadata: "text-zinc-400",
  updates: "text-amber-300",
  values: "text-emerald-300",
  end: "text-orange-400",
};

/**
 * The SSE stream, rendered like a terminal: metadata → updates → values →
 * end frames, each carrying the {checkpoint_id}:{step}:{seq} frame id that
 * Last-Event-ID reconnects dedupe against.
 */
export function EventLog({ frames, threadId }: EventLogProps) {
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [frames.length, threadId]);

  return (
    <div className="bg-code overflow-hidden rounded-xl border border-black/40 shadow-lg">
      <div className="flex items-center justify-between border-b border-white/10 px-4 py-2">
        <span className="flex items-center gap-2 font-code text-xs text-white/60">
          <Terminal size={13} />
          runs/stream · thread {threadId}
        </span>
        <span className="font-code text-[10px] uppercase tracking-wider text-white/40">
          SSE
        </span>
      </div>
      <div className="h-80 overflow-y-auto p-4">
        {frames.length === 0 ? (
          <p className="font-code text-xs text-white/40">
            # No frames yet — the stream starts when you run.
            <br />
            # stream_mode: [updates, values]
          </p>
        ) : (
          <div className="space-y-3">
            {frames.map((f) => (
              <div key={`${threadId}-${f.seq}`} className="font-code text-[11.5px] leading-relaxed">
                <div>
                  <span className="text-white/35">event: </span>
                  <span className={FRAME_COLORS[f.event]}>{f.event}</span>
                </div>
                <div>
                  <span className="text-white/35">id: </span>
                  <span className="text-white/60">{f.frameId}</span>
                </div>
                <div className="break-all">
                  <span className="text-white/35">data: </span>
                  <span className="text-white/85">{JSON.stringify(f.data)}</span>
                </div>
              </div>
            ))}
            <div ref={endRef} />
          </div>
        )}
      </div>
    </div>
  );
}
