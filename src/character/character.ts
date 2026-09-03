// キャラクターの表示と動き。状態名を受け取り、画像・バッジ・CSS アニメーションを切り替える。
// 動き自体は styles.css の `#stage[data-state=...]` に書いてある。

import type { AssistantState } from "../generated/AssistantState";

export const ALL_STATES: readonly AssistantState[] = [
  "idle",
  "thinking",
  "working",
  "waiting",
  "success",
  "error",
];

/** 頭上に出す状態バッジ。idle は何も出さない */
const BADGES: Record<AssistantState, string> = {
  idle: "",
  thinking: "💭",
  working: "⚙️",
  waiting: "🔐",
  success: "✨",
  error: "⚠️",
};

const BASE_IMAGE = "/character/base.png";
const PLACEHOLDER_IMAGE = "/character/placeholder.svg";

/** 画像 URL が実際に画像として読めるか(存在しない場合は dev サーバーが HTML を返すので decode で落ちる) */
function probeImage(url: string): Promise<boolean> {
  return new Promise((resolve) => {
    const img = new Image();
    img.onload = () => resolve(img.naturalWidth > 0);
    img.onerror = () => resolve(false);
    img.src = url;
  });
}

export class Character {
  private state: AssistantState = "idle";
  private baseSrc = PLACEHOLDER_IMAGE;
  /** 状態ごとの差分画像(public/character/<state>.png)。無ければ base を使う */
  private variants = new Map<AssistantState, string>();

  constructor(
    private readonly stage: HTMLElement,
    private readonly img: HTMLImageElement,
    private readonly badge: HTMLElement,
    private readonly log: (message: string) => void = () => {},
  ) {}

  /** 画像を解決して表示する。差分画像の探索は表示をブロックしない */
  async mount(): Promise<void> {
    this.baseSrc = (await probeImage(BASE_IMAGE)) ? BASE_IMAGE : PLACEHOLDER_IMAGE;
    this.img.src = this.baseSrc;
    this.log(`character base: ${this.baseSrc}`);
    this.applyVisual();

    void Promise.all(
      ALL_STATES.filter((s) => s !== "idle").map(async (s) => {
        const url = `/character/${s}.png`;
        if (await probeImage(url)) this.variants.set(s, url);
      }),
    ).then(() => {
      if (this.variants.size > 0) {
        this.log(`character variants: ${[...this.variants.keys()].join(", ")}`);
        this.applyVisual();
      }
    });
  }

  current(): AssistantState {
    return this.state;
  }

  setState(state: AssistantState): void {
    if (state === this.state) return;
    this.state = state;
    this.applyVisual();
  }

  private applyVisual(): void {
    this.stage.dataset.state = this.state;
    this.badge.textContent = BADGES[this.state];
    const src = this.variants.get(this.state) ?? this.baseSrc;
    if (!this.img.src.endsWith(src)) this.img.src = src;
  }
}
