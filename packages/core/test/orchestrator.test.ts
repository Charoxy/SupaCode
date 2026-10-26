import { describe, expect, it } from "vitest";
import { DEFAULT_CONFIG, Orchestrator, type Agent, type AgentEvent } from "../src/index.js";

function fakeAgent(id: Agent["id"], available: boolean): Agent {
  return {
    id,
    isAvailable: async () => available,
    async *run(request) {
      yield { type: "text", text: `${id}:${request.model ?? "défaut"}` };
      yield { type: "done", exitCode: 0 };
    },
  };
}

async function collect(events: AsyncIterable<AgentEvent>): Promise<AgentEvent[]> {
  const out: AgentEvent[] = [];
  for await (const event of events) out.push(event);
  return out;
}

describe("Orchestrator", () => {
  it("émet la décision puis relaie l'agent choisi", async () => {
    const orchestrator = new Orchestrator(DEFAULT_CONFIG, [fakeAgent("claude", true), fakeAgent("codex", true)]);
    const events = await collect(orchestrator.run("corrige la typo", { cwd: "." }));
    expect(events.map((e) => e.type)).toEqual(["route", "text", "done"]);
    expect(events[1]).toEqual({ type: "text", text: "codex:défaut" });
  });

  it("renvoie une erreur claire si l'agent n'est pas installé", async () => {
    const orchestrator = new Orchestrator(DEFAULT_CONFIG, [fakeAgent("claude", true), fakeAgent("codex", false)]);
    const events = await collect(orchestrator.run("corrige la typo", { cwd: "." }));
    expect(events[1]).toMatchObject({ type: "error" });
    expect(events[2]).toEqual({ type: "done", exitCode: 127 });
  });
});
