// 立ち絵のシートと所作の定義。仕様は docs/character-sheet.md。
// 行の順序・行数・列数は Rust 側 sprite.rs(ROWS / COLS)と揃える。
// ここは DOM に触らない純粋な計算だけを置く(character.ts が使い、sheet.test.ts が検める)。

import type { AssistantState } from "../generated/AssistantState";
import type { SheetStatus } from "../generated/SheetStatus";

/** 行 = 状態。Rust 側 sprite.rs の ROWS と一致させる */
export const ROWS = 6;
/** 列 = コマ。Rust 側 sprite.rs の COLS と一致させる */
export const COLS = 4;

/** 素材が無いときに使う、アプリ同梱のシート */
export const PLACEHOLDER_URL = "/placeholder.png";
/** プレースホルダーの 1 コマの寸法(public/placeholder.png と一致させる) */
export const PLACEHOLDER_FRAME: Size = { width: 240, height: 320 };

/** 立ち絵を収める枠(styles.css の #figure と一致させる) */
export const FIGURE: Size = { width: 240, height: 320 };

export interface Size {
  width: number;
  height: number;
}

/** 1 つの状態に対応する所作 */
export interface Motion {
  /** シートの何行目か(0 始まり) */
  row: number;
  fps: number;
  /** 持続する状態はループ、一時状態は 1 周で止めて最終コマを保つ */
  loop: boolean;
}

/**
 * 状態ごとの所作。速さはアプリが決め打ちで、シートからは変えられない。
 * success と error を 1 周で止めるのは、「一度起きたこと」を繰り返さないため。
 */
export const MOTIONS: Record<AssistantState, Motion> = {
  idle: { row: 0, fps: 4, loop: true },
  thinking: { row: 1, fps: 6, loop: true },
  working: { row: 2, fps: 10, loop: true },
  waiting: { row: 3, fps: 4, loop: true },
  success: { row: 4, fps: 12, loop: false },
  error: { row: 5, fps: 12, loop: false },
};

/** 1 巡にかかる秒数 */
export function cycleSeconds(motion: Motion): number {
  return COLS / motion.fps;
}

/**
 * 行の縦位置(%)。background-size が縦 ROWS×100% のとき、r 行目は r/(ROWS-1)×100% になる
 * (百分率の位置指定は「画像の p% の点を枠の p% の点に合わせる」ため)。
 */
export function rowPosition(row: number): number {
  return (row / (ROWS - 1)) * 100;
}

/**
 * 1 コマを枠に contain で収めたときの寸法。
 *
 * 背景画像は枠いっぱいに引き伸ばされてしまうので、枠のほうを 1 コマの縦横比に合わせる。
 * 推奨どおり 3:4 のシートなら #figure と同じ 240x320 になり、外れていれば小さく収まる。
 */
export function containSize(frame: Size, box: Size = FIGURE): Size {
  if (frame.width <= 0 || frame.height <= 0) return { ...box };
  const scale = Math.min(box.width / frame.width, box.height / frame.height);
  return { width: frame.width * scale, height: frame.height * scale };
}

/** 使えるシートがあればその 1 コマの寸法、無ければプレースホルダーのもの */
export function frameSize(status: SheetStatus | null): Size {
  if (!status?.usable || !status.frame_width || !status.frame_height) return PLACEHOLDER_FRAME;
  return { width: status.frame_width, height: status.frame_height };
}

/**
 * 使えるシートがあればその URL、無ければプレースホルダー。
 * 差し替えても webview が古い画像を出し続けないよう、版を付ける。
 */
export function sheetUrl(status: SheetStatus | null, toAssetUrl: (path: string) => string): string {
  if (!status?.usable) return PLACEHOLDER_URL;
  return `${toAssetUrl(status.path)}?v=${status.version}`;
}
