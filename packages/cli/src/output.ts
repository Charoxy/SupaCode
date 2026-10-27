import type { AgentEvent, RouteDecision } from "@supacode/core";

export function formatDecision(d: RouteDecision): string {
  return `${d.agent}${d.model ? ` (${d.model})` : ""} : ${d.reason}`;
}

export function printAgentEvent(event: AgentEvent, indent = ""): void {
  switch (event.type) {
    case "route":
      console.log(`${indent}▶ ${formatDecision(event.decision)}`);
      break;
    case "text":
      console.log(event.text.replace(/^/gm, indent));
      break;
    case "tool":
      console.log(`${indent}· ${event.name}${event.detail ? ` ${event.detail}` : ""}`);
      break;
    case "error":
      console.error(`${indent}✖ ${event.message}`);
      break;
    // Les logs et la fin ne sont visibles qu'en --json.
  }
}
