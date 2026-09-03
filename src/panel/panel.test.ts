import { describe, expect, test } from "bun:test";

import type { SecretarySnapshot } from "../generated/SecretarySnapshot";
import { panelMode, toolLine } from "./panel";

function snap(over: Partial<SecretarySnapshot>): SecretarySnapshot {
  return {
    status: "idle",
    message: null,
    message_kind: null,
    current_tool: null,
    task_summary: null,
    pending_permission: null,
    session_id: "s",
    session_label: "app",
    tracked_sessions: 1,
    ...over,
  };
}

describe("panelMode", () => {
  test("hidden when nothing is tracked", () => {
    expect(panelMode(snap({ tracked_sessions: 0, session_id: null }))).toBe("hide");
  });
  test("shown while active, lingers on idle", () => {
    expect(panelMode(snap({ status: "working" }))).toBe("show");
    expect(panelMode(snap({ status: "waiting" }))).toBe("show");
    expect(panelMode(snap({ status: "success" }))).toBe("show");
    expect(panelMode(snap({ status: "idle" }))).toBe("linger");
  });
});

describe("toolLine", () => {
  test("permission wins over the running tool", () => {
    expect(toolLine(snap({ status: "waiting", pending_permission: "Bash", current_tool: "Bash: rm x" }))).toBe(
      "許可待ち: Bash",
    );
  });
  test("falls back to the current tool or empty", () => {
    expect(toolLine(snap({ status: "working", current_tool: "Edit: a.rs" }))).toBe("Edit: a.rs");
    expect(toolLine(snap({ status: "thinking" }))).toBe("");
  });
});
