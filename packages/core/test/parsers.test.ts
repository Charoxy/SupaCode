import { describe, expect, it } from "vitest";
import { parseClaudeLine, parseCodexLine } from "../src/index.js";

const json = (value: unknown) => JSON.stringify(value);

describe("parseClaudeLine", () => {
  it("transforme un message assistant en texte et outils", () => {
    const line = json({
      type: "assistant",
      message: {
        content: [
          { type: "text", text: "Je corrige le fichier." },
          { type: "tool_use", name: "Edit", input: { file_path: "README.md" } },
        ],
      },
    });
    expect(parseClaudeLine(line)).toEqual([
      { type: "text", text: "Je corrige le fichier." },
      { type: "tool", name: "Edit", detail: "README.md" },
    ]);
  });

  it("transforme un résultat réussi en done", () => {
    expect(parseClaudeLine(json({ type: "result", subtype: "success", is_error: false, result: "Fait." }))).toEqual([
      { type: "done", exitCode: 0, summary: "Fait." },
    ]);
  });

  it("transforme un résultat en échec en error", () => {
    const [event] = parseClaudeLine(json({ type: "result", subtype: "error_max_turns", is_error: true }));
    expect(event?.type).toBe("error");
  });

  it("garde les lignes non JSON comme log", () => {
    expect(parseClaudeLine("pas du json")).toEqual([{ type: "log", message: "pas du json" }]);
  });
});

describe("parseCodexLine", () => {
  it("transforme agent_message en texte", () => {
    const line = json({ type: "item.completed", item: { id: "item_1", type: "agent_message", text: "Terminé." } });
    expect(parseCodexLine(line)).toEqual([{ type: "text", text: "Terminé." }]);
  });

  it("transforme command_execution en outil shell", () => {
    const line = json({ type: "item.completed", item: { type: "command_execution", command: "npm test", exit_code: 0 } });
    expect(parseCodexLine(line)).toEqual([{ type: "tool", name: "shell", detail: "npm test" }]);
  });

  it("transforme file_change en outil edit", () => {
    const line = json({ type: "item.completed", item: { type: "file_change", changes: [{ path: "src/a.ts", kind: "update" }] } });
    expect(parseCodexLine(line)).toEqual([{ type: "tool", name: "edit", detail: "src/a.ts" }]);
  });

  it("transforme turn.failed et error en error", () => {
    expect(parseCodexLine(json({ type: "turn.failed", error: { message: "quota dépassé" } }))).toEqual([
      { type: "error", message: "quota dépassé" },
    ]);
    expect(parseCodexLine(json({ type: "error", message: "boom" }))).toEqual([{ type: "error", message: "boom" }]);
  });

  it("garde le reste comme log", () => {
    const line = json({ type: "turn.started" });
    expect(parseCodexLine(line)).toEqual([{ type: "log", message: line }]);
  });
});
