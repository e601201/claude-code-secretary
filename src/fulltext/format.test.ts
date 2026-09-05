import { describe, expect, test } from "bun:test";

import { formatReceivedAt, headerText } from "./format";

const at = (y: number, m: number, d: number, h: number, min: number): number =>
  new Date(y, m - 1, d, h, min).getTime();

describe("formatReceivedAt", () => {
  test("always carries the date, so the reading never depends on when you read it", () => {
    expect(formatReceivedAt(at(2026, 9, 5, 14, 32))).toBe("9/5 14:32");
  });

  test("pads the time but not the date", () => {
    expect(formatReceivedAt(at(2026, 9, 5, 9, 5))).toBe("9/5 09:05");
  });

  test("a message from another day reads the same way", () => {
    // 控えは消さないので、昨日以前の発言が残っていることがある
    expect(formatReceivedAt(at(2026, 9, 4, 23, 10))).toBe("9/4 23:10");
    expect(formatReceivedAt(at(2025, 12, 31, 8, 0))).toBe("12/31 08:00");
  });
});

describe("headerText", () => {
  const base = {
    text: "x",
    session_label: "myproject",
    received_at_ms: at(2026, 9, 5, 14, 32),
    truncated: false,
  };

  test("names the session and when it spoke", () => {
    expect(headerText(base)).toBe("myproject · 9/5 14:32");
  });

  test("says so when the body was capped", () => {
    expect(headerText({ ...base, truncated: true })).toBe(
      "myproject · 9/5 14:32 · 長すぎるためここで切れています",
    );
  });
});
