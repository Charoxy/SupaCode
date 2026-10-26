import { ClaudeAgent } from "./agents/claude.js";
import { CodexAgent } from "./agents/codex.js";
import type { SupaCodeConfig } from "./config.js";
import { route, type RouteOverride } from "./router.js";
import type { Agent, AgentEvent, AgentId, RouteDecision } from "./types.js";

export interface RunOptions {
  cwd: string;
  override?: RouteOverride;
  signal?: AbortSignal;
}

const INSTALL_HINTS: Record<AgentId, string> = {
  claude: "npm install -g @anthropic-ai/claude-code",
  codex: "npm install -g @openai/codex",
};

/** Choisit l'agent pour chaque demande puis relaie ses événements. */
export class Orchestrator {
  private readonly agents: Map<AgentId, Agent>;

  constructor(
    private readonly config: SupaCodeConfig,
    agents: Agent[] = [new ClaudeAgent(), new CodexAgent()],
  ) {
    this.agents = new Map(agents.map((a) => [a.id, a]));
  }

  decide(prompt: string, override?: RouteOverride): RouteDecision {
    return route(prompt, this.config, override);
  }

  async *run(prompt: string, options: RunOptions): AsyncGenerator<AgentEvent> {
    const decision = this.decide(prompt, options.override);
    yield { type: "route", decision };

    const agent = this.agents.get(decision.agent);
    if (!agent || !(await agent.isAvailable())) {
      yield {
        type: "error",
        message: `L'agent « ${decision.agent} » est introuvable : installe sa CLI (${INSTALL_HINTS[decision.agent]}) ou force un autre agent avec --agent.`,
      };
      yield { type: "done", exitCode: 127 };
      return;
    }

    yield* agent.run({ prompt, cwd: options.cwd, model: decision.model }, options.signal);
  }
}
