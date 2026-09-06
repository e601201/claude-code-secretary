// 立ち絵の表示。状態を受け取り、シートの行を選び、コマ送りの速さと回数を決める。
// コマを送るのは styles.css の #character(steps() の CSS アニメーション)で、
// ここが渡すのは「どの行を」「どれくらいの速さで」「何周するか」の 3 つだけ。

import { convertFileSrc } from "@tauri-apps/api/core";

import type { AssistantState } from "../generated/AssistantState";
import type { SheetStatus } from "../generated/SheetStatus";
import {
  containSize,
  cycleSeconds,
  frameSize,
  MOTIONS,
  PLACEHOLDER_URL,
  rowPosition,
  sheetUrl,
} from "./sheet";

/** 頭上に出す状態バッジ。idle は何も出さない */
const BADGES: Record<AssistantState, string> = {
  idle: "",
  thinking: "💭",
  working: "⚙️",
  waiting: "🔐",
  success: "✨",
  error: "⚠️",
};

/** Tauri の外(素のブラウザ)で開いたときは、プレースホルダーだけで動かす */
function assetUrl(path: string): string {
  try {
    return convertFileSrc(path);
  } catch {
    return PLACEHOLDER_URL;
  }
}

export class Character {
  private state: AssistantState = "idle";

  constructor(
    private readonly stage: HTMLElement,
    /** 背景としてシートを持つ要素(#character) */
    private readonly figure: HTMLElement,
    private readonly badge: HTMLElement,
    private readonly log: (message: string) => void = () => {},
  ) {}

  /**
   * 立ち絵を差し替える。使えるシートが無ければアプリ同梱のプレースホルダーで動かす
   * (判定は Rust 側の sprite.rs が済ませてある)。
   */
  setSheet(status: SheetStatus | null): void {
    const url = sheetUrl(status, assetUrl);
    // 背景は枠いっぱいに引き伸ばされるので、枠のほうを 1 コマの縦横比に合わせる
    const size = containSize(frameSize(status));
    this.figure.style.setProperty("--sheet", `url("${url}")`);
    this.figure.style.width = `${size.width}px`;
    this.figure.style.height = `${size.height}px`;
    this.log(
      status?.usable
        ? `sheet: ${status.path} (1 コマ ${status.frame_width}x${status.frame_height})`
        : `sheet: placeholder (${status?.reason ?? "まだ取得していません"})`,
    );
    this.applyVisual();
  }

  setState(state: AssistantState): void {
    if (state === this.state) return;
    this.state = state;
    this.applyVisual();
  }

  private applyVisual(): void {
    this.stage.dataset.state = this.state;
    this.badge.textContent = BADGES[this.state];

    const motion = MOTIONS[this.state];
    const style = this.figure.style;
    style.setProperty("--row", `${rowPosition(motion.row)}%`);
    style.setProperty("--cycle", `${cycleSeconds(motion)}s`);
    // 一時状態(success / error)は 1 周で止まり、最終コマのまま残る
    style.setProperty("--iterations", motion.loop ? "infinite" : "1");

    // 状態が変わったらコマ位置を 1 コマ目に戻す。CSS 変数を書き換えるだけでは
    // 走っているアニメーションは巻き戻らないので、いったん外して掛け直す。
    style.animationName = "none";
    void this.figure.offsetWidth;
    style.animationName = "";
  }
}
