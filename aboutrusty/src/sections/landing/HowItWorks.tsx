import { CodeBlock } from "@/components/shared/CodeBlock";
import { SectionHeading } from "./SectionHeading";

const RUST_EXAMPLE = `use rusty_agent_runtime::prelude::*;
use serde_json::json;
use std::sync::Arc;
struct Echo; // scripted ChatModel: one canned reply (see examples/react_agent.rs for tools)
#[async_trait::async_trait]
impl ChatModel for Echo {
    async fn chat(&self, _: &[ChatMessage], _: &[serde_json::Value]) -> Result<ChatResponse> {
        Ok(ChatResponse { message: ChatMessage::assistant("42"), model: None, usage: None })
    }
}
#[tokio::main]
async fn main() -> Result<()> {
    let graph = create_react_agent(Arc::new(Echo), ToolRegistry::new())?;
    let spec = StateSpec::new().channel("messages", Reducer::AddMessages);
    let mut input = State::new();
    input.insert("messages", json!([ChatMessage::user("What is 17 + 25?")]));
    let outcome = Executor::new().run(&graph, &spec, input, RunConfig::new("demo")).await?;
    assert!(matches!(outcome, ExecutionOutcome::Done(_)));
    Ok(())
}`;

interface Step {
  number: string;
  title: string;
  body: string;
}

const STEPS: Step[] = [
  {
    number: "01",
    title: "Define the graph",
    body: "GraphBuilder wires nodes over state channels with per-key reducers. Topology is validated at compile() — before any node or paid LLM call runs.",
  },
  {
    number: "02",
    title: "Execute in super-steps",
    body: "A Pregel/BSP loop: plan → parallel over an immutable snapshot → barrier → merge via reducers → route. Each step is transactional and guarded by max_steps.",
  },
  {
    number: "03",
    title: "Checkpoint everything",
    body: "A versioned checkpoint is written at every step boundary — the one primitive behind resume after a crash, human-in-the-loop interrupts, and fork & replay time travel.",
  },
];

export function HowItWorks() {
  return (
    <section className="border-y bg-secondary/40">
      <div className="mx-auto max-w-6xl px-4 py-20 sm:px-6 sm:py-28">
        <SectionHeading
          eyebrow="How it works"
          title="One execution model, end to end."
          description="A ReAct agent over a scripted model — no network, deterministic output. The same compiled graph runs embedded, behind the HTTP/SSE server, or across remote and WASM nodes."
        />

        <div className="mt-12 grid gap-8 md:grid-cols-3">
          {STEPS.map((step) => (
            <div key={step.number} className="flex flex-col gap-3">
              <span className="font-code text-sm text-primary">
                {step.number}
              </span>
              <h3 className="font-display text-xl font-semibold tracking-tight">
                {step.title}
              </h3>
              <p className="text-sm leading-relaxed text-muted-foreground">
                {step.body}
              </p>
            </div>
          ))}
        </div>

        <div className="mt-12">
          <CodeBlock
            code={RUST_EXAMPLE}
            language="rust"
            title="rusty-core/examples/react_agent.rs — condensed"
          />
          <p className="mt-3 text-center text-xs text-muted-foreground">
            Swap <code className="font-code">Echo</code> for{" "}
            <code className="font-code">OpenAiCompatibleClient</code> to talk to
            OpenAI, vLLM, Ollama, LM Studio, or a compatible gateway.
          </p>
        </div>
      </div>
    </section>
  );
}
