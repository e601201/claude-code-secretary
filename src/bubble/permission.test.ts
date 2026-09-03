import { describe, expect, test } from "bun:test";

import { permissionDetail, previewLines } from "./bubble";
import type { RelayedPermission } from "../generated/RelayedPermission";

const base: RelayedPermission = {
  request_id: "abcde",
  tool_name: "Bash",
  description: "テストを実行する",
  input_preview: '{"command":"bun test","timeout":120000}',
};

describe("previewLines", () => {
  test("JSON オブジェクトはキー: 値の行になる", () => {
    expect(previewLines(base.input_preview)).toEqual(["command: bun test", "timeout: 120000"]);
  });

  test("JSON でなければそのまま 1 行、空なら無し", () => {
    expect(previewLines("command: ls ⋯ 3 code points elided ⋯")).toEqual([
      "command: ls ⋯ 3 code points elided ⋯",
    ]);
    expect(previewLines("   ")).toEqual([]);
  });
});

describe("permissionDetail", () => {
  test("説明とプレビューを改行でつなぐ", () => {
    expect(permissionDetail(base)).toBe("テストを実行する\ncommand: bun test\ntimeout: 120000");
  });

  test("説明が無ければプレビューだけ", () => {
    expect(permissionDetail({ ...base, description: "" })).toBe("command: bun test\ntimeout: 120000");
  });

  test("長ければ末尾を省く", () => {
    const long = permissionDetail(
      { ...base, input_preview: JSON.stringify({ command: "x".repeat(300) }) },
      40,
    );
    expect([...long].length).toBe(40);
    expect(long.endsWith("…")).toBe(true);
  });
});
