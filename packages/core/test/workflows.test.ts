import { mkdtemp, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  DEFAULT_CONFIG,
  loadWorkflows,
  Orchestrator,
  parseWorkflow,
  render,
  runWorkflow,
  type Agent,
  type Workflow,
  type WorkflowEvent,
} from "../src/index.js";

/** Faux agent : répond le prompt reçu, échoue si le prompt contient « ÉCHEC ». */
function echoAgent(id: Agent["id"], prompts: string[] = []): Agent {
  return {
    id,
    isAvailable: async () => true,
    async *run(request) {
      prompts.push(request.prompt);
      yield { type: "text", text: `${id} a reçu : ${request.prompt}` };
      yield { type: "done", exitCode: request.prompt.includes("ÉCHEC") ? 1 : 0 };
    },
  };
}

async function execute(workflow: Workflow, options: Partial<Parameters<typeof runWorkflow>[2]> = {}, prompts: string[] = []) {
  const orchestrator = new Orchestrator(DEFAULT_CONFIG, [echoAgent("claude", prompts), echoAgent("codex", prompts)]);
  const events: WorkflowEvent[] = [];
  for await (const event of runWorkflow(parseWorkflow(workflow), orchestrator, { cwd: process.cwd(), ...options })) events.push(event);
  const status = (id: string) => events.find((e) => e.type === "step.done" && e.stepId === id)?.["status" as never];
  const done = events.at(-1);
  return { events, status, result: done?.type === "workflow.done" ? done.status : undefined };
}

const node = (code: string) => `node -e "${code}"`;

describe("parseWorkflow", () => {
  it("accepte un workflow valide", () => {
    const wf = parseWorkflow({ name: "ok", on: ["commit", { save: "src/**/*.ts" }], steps: [{ id: "a", run: "echo" }] });
    expect(wf.name).toBe("ok");
  });

  it("refuse les erreurs avec un message lisible", () => {
    expect(() => parseWorkflow({ name: "x", steps: [] })).toThrow("au moins une étape");
    expect(() => parseWorkflow({ name: "x", steps: [{ id: "a", run: "x", prompt: "y" }] })).toThrow("exactement un");
    expect(() => parseWorkflow({ name: "x", steps: [{ id: "a", run: "x" }, { id: "a", run: "y" }] })).toThrow("en double");
    expect(() => parseWorkflow({ name: "x", steps: [{ id: "a", run: "x", if: "steps.b.failed" }] })).toThrow("inconnue");
    expect(() => parseWorkflow({ name: "x", on: ["midi"], steps: [{ id: "a", run: "x" }] })).toThrow("déclencheur");
  });
});

describe("loadWorkflows", () => {
  it("charge les fichiers JSON du dossier .supacode/workflows", async () => {
    const dir = await mkdtemp(join(tmpdir(), "supacode-"));
    expect(await loadWorkflows(dir)).toEqual([]);
    await mkdir(join(dir, ".supacode", "workflows"), { recursive: true });
    await writeFile(join(dir, ".supacode", "workflows", "review.json"), JSON.stringify({ name: "Review", steps: [{ id: "a", run: "echo" }] }));
    const files = await loadWorkflows(dir);
    expect(files.map((f) => f.id)).toEqual(["review"]);
  });
});

describe("runWorkflow", () => {
  it("enchaîne commandes et agents en injectant la sortie des étapes", async () => {
    const prompts: string[] = [];
    const { status, result } = await execute(
      {
        name: "chaîne",
        steps: [
          { id: "lint", run: node("console.log('2 erreurs')") },
          { id: "fix", agent: "codex", prompt: "Corrige : {{steps.lint.output}} dans {{file}}" },
        ],
      },
      { context: { file: "app.ts" } },
      prompts,
    );
    expect(status("lint")).toBe("success");
    expect(status("fix")).toBe("success");
    expect(prompts[0]).toBe("Corrige : 2 erreurs dans app.ts");
    expect(result).toBe("success");
  });

  it("saute les étapes après un échec, sauf celles qui le rattrapent", async () => {
    const { status, result } = await execute({
      name: "rattrapage",
      steps: [
        { id: "lint", run: node("process.exit(3)") },
        { id: "fix", prompt: "corrige", agent: "codex", if: "steps.lint.failed" },
        { id: "tests", run: node("") },
        { id: "notif", run: node(""), if: "always" },
      ],
    });
    expect(status("lint")).toBe("failed");
    expect(status("fix")).toBe("success");
    expect(status("tests")).toBe("success");
    expect(status("notif")).toBe("success");
    expect(result).toBe("success");
  });

  it("échoue et saute la suite sur un échec non rattrapé", async () => {
    const { status, result } = await execute({
      name: "échec",
      steps: [
        { id: "build", run: node("process.exit(1)") },
        { id: "deploy", run: node("") },
      ],
    });
    expect(status("deploy")).toBe("skipped");
    expect(result).toBe("failed");
  });

  it("lance les étapes parallèles et échoue si l'une échoue", async () => {
    const { events, status } = await execute({
      name: "parallèle",
      steps: [
        {
          id: "checks",
          parallel: [
            { id: "a", prompt: "ok", agent: "claude" },
            { id: "b", prompt: "ÉCHEC", agent: "codex" },
          ],
        },
      ],
    });
    expect(status("a")).toBe("success");
    expect(status("b")).toBe("failed");
    expect(status("checks")).toBe("failed");
    expect(events.filter((e) => e.type === "step.start").map((e) => (e as { stepId: string }).stepId)).toEqual(["checks", "a", "b"]);
  });

  it("un échec rattrapé dans un groupe parallèle ne fait pas échouer le workflow", async () => {
    const { status, result } = await execute({
      name: "groupe",
      steps: [
        { id: "checks", parallel: [{ id: "a", run: node("") }, { id: "b", run: node("process.exit(1)") }] },
        { id: "fix", agent: "codex", prompt: "corrige", if: "steps.b.failed" },
        { id: "suite", run: node("") },
      ],
    });
    expect(status("checks")).toBe("failed");
    expect(status("suite")).toBe("success");
    expect(result).toBe("success");
  });

  it("demande la validation et respecte un refus", async () => {
    const asked: string[] = [];
    const wf: Workflow = { name: "validation", steps: [{ id: "ok", approve: "Déployer ?" }, { id: "deploy", run: node("") }] };
    const refused = await execute(wf, { approve: async (_, message) => (asked.push(message), false) });
    expect(asked).toEqual(["Déployer ?"]);
    expect(refused.status("deploy")).toBe("skipped");
    const accepted = await execute(wf, { approve: async () => true });
    expect(accepted.result).toBe("success");
  });
});

describe("render", () => {
  it("laisse les variables inconnues intactes", () => {
    expect(render("{{a}} et {{ b }}", { a: "1" })).toBe("1 et {{ b }}");
  });
});
