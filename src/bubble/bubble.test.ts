import { describe, expect, test } from "bun:test";

import { durationFor } from "./bubble";

describe("durationFor", () => {
  test("permission stays until the state changes", () => {
    expect(durationFor("Bash の実行許可を待っています", "permission")).toBe(0);
  });

  test("system messages have a fixed duration", () => {
    expect(durationFor("x", "system")).toBe(6000);
  });

  test("speech duration grows with length and is clamped", () => {
    expect(durationFor("短い", "reply")).toBe(4000);
    expect(durationFor("あ".repeat(50), "assistant")).toBe(7000);
    expect(durationFor("あ".repeat(500), "reply")).toBe(12000);
  });
});
