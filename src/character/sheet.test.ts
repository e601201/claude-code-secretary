import { describe, expect, it } from "bun:test";

import type { SheetStatus } from "../generated/SheetStatus";
import {
  COLS,
  containSize,
  cycleSeconds,
  frameSize,
  MOTIONS,
  PLACEHOLDER_FRAME,
  PLACEHOLDER_URL,
  ROWS,
  rowPosition,
  sheetUrl,
} from "./sheet";

const usable: SheetStatus = {
  path: "/tmp/character.png",
  usable: true,
  reason: null,
  frame_width: 480,
  frame_height: 640,
  width: 1920,
  height: 3840,
  version: 42,
};

const missing: SheetStatus = {
  path: "/tmp/character.png",
  usable: false,
  reason: "まだ置かれていません",
  frame_width: null,
  frame_height: null,
  width: null,
  height: null,
  version: 0,
};

describe("所作の定義", () => {
  it("6 つの状態が 6 行に 1 つずつ割り当たっている", () => {
    const rows = Object.values(MOTIONS).map((m) => m.row);
    expect(rows.length).toBe(ROWS);
    expect([...new Set(rows)].sort()).toEqual([0, 1, 2, 3, 4, 5]);
  });

  it("行の順序は state-machine.md の状態表と同じ", () => {
    expect(Object.keys(MOTIONS)).toEqual([
      "idle",
      "thinking",
      "working",
      "waiting",
      "success",
      "error",
    ]);
  });

  it("一時状態(success / error)だけが 1 周で止まる", () => {
    const oneShot = Object.entries(MOTIONS)
      .filter(([, m]) => !m.loop)
      .map(([state]) => state);
    expect(oneShot).toEqual(["success", "error"]);
  });

  it("1 巡の秒数はコマ数を fps で割ったもの", () => {
    expect(cycleSeconds(MOTIONS.idle)).toBeCloseTo(1.0);
    expect(cycleSeconds(MOTIONS.working)).toBeCloseTo(0.4);
    expect(cycleSeconds({ row: 0, fps: COLS, loop: true })).toBe(1);
  });
});

describe("行の位置", () => {
  it("最初の行が 0%、最後の行が 100%", () => {
    expect(rowPosition(0)).toBe(0);
    expect(rowPosition(ROWS - 1)).toBe(100);
  });

  it("行は等間隔に並ぶ", () => {
    expect(rowPosition(1)).toBeCloseTo(20);
    expect(rowPosition(3)).toBeCloseTo(60);
  });
});

describe("枠に収める", () => {
  it("推奨どおり 3:4 なら枠いっぱいになる", () => {
    expect(containSize({ width: 480, height: 640 })).toEqual({ width: 240, height: 320 });
  });

  it("縦長のコマは高さで決まり、横に余白ができる", () => {
    expect(containSize({ width: 100, height: 400 })).toEqual({ width: 80, height: 320 });
  });

  it("横長のコマは幅で決まり、縦に余白ができる", () => {
    expect(containSize({ width: 400, height: 100 })).toEqual({ width: 240, height: 60 });
  });

  it("寸法が壊れていても枠の大きさを返す(0 で割らない)", () => {
    expect(containSize({ width: 0, height: 0 })).toEqual({ width: 240, height: 320 });
  });
});

describe("立ち絵の解決順", () => {
  it("使えるシートがあればそれを使い、版を付ける", () => {
    expect(sheetUrl(usable, (p) => `asset://${p}`)).toBe("asset:///tmp/character.png?v=42");
    expect(frameSize(usable)).toEqual({ width: 480, height: 640 });
  });

  it("シートが無ければプレースホルダーに落ちる", () => {
    expect(sheetUrl(missing, (p) => `asset://${p}`)).toBe(PLACEHOLDER_URL);
    expect(frameSize(missing)).toEqual(PLACEHOLDER_FRAME);
  });

  it("まだ何も受け取っていないときもプレースホルダー", () => {
    expect(sheetUrl(null, (p) => `asset://${p}`)).toBe(PLACEHOLDER_URL);
    expect(frameSize(null)).toEqual(PLACEHOLDER_FRAME);
  });

  it("寸法が合わないシートは、読めていてもプレースホルダーに落ちる", () => {
    const odd: SheetStatus = { ...missing, reason: "幅が割り切れません", width: 1921, height: 3840 };
    expect(sheetUrl(odd, (p) => `asset://${p}`)).toBe(PLACEHOLDER_URL);
  });
});
