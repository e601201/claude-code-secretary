// 吹き出し。スナップショットの message / message_kind を受け取り、種別に応じた見た目と時間で表示する。
// 何を話すかは Rust 側が決める。ここでは「どう見せるか」だけを扱う。

import type { SecretarySnapshot } from "../generated/SecretarySnapshot";
import type { SpeechKind } from "../generated/SpeechKind";

/** 表示時間(ミリ秒)。0 は「状態が変わるまで出しっぱなし」 */
export function durationFor(text: string, kind: SpeechKind): number {
  switch (kind) {
    case "permission":
      return 0;
    case "system":
      return 6000;
    case "reply":
    case "assistant": {
      const chars = [...text].length;
      return Math.min(12000, Math.max(4000, 3000 + chars * 80));
    }
  }
}

export class SpeechBubble {
  private timer: ReturnType<typeof setTimeout> | null = null;
  /** 直近に表示した文言の識別子。同じ文言のスナップショットが続いても再表示しない */
  private lastKey: string | null = null;

  constructor(
    private readonly root: HTMLElement,
    private readonly textEl: HTMLElement,
    private readonly log: (message: string) => void = () => {},
  ) {}

  update(snapshot: SecretarySnapshot): void {
    const text = snapshot.message;
    if (!text) {
      this.lastKey = null;
      this.hide();
      return;
    }
    const kind: SpeechKind = snapshot.message_kind ?? "assistant";
    const key = `${kind}:${text}`;
    if (key === this.lastKey) return;
    this.lastKey = key;
    this.show(text, kind, durationFor(text, kind));
  }

  show(text: string, kind: SpeechKind, durationMs: number): void {
    this.clearTimer();
    this.textEl.textContent = text;
    this.root.dataset.kind = kind;
    // 連続表示でも入場アニメーションをやり直す
    this.root.classList.remove("visible");
    void this.root.offsetWidth;
    this.root.classList.add("visible");
    this.log(`bubble show kind=${kind} duration=${durationMs} text=${JSON.stringify(text)}`);
    if (durationMs > 0) {
      this.timer = setTimeout(() => this.hide(), durationMs);
    }
  }

  hide(): void {
    this.clearTimer();
    if (this.root.classList.contains("visible")) {
      this.root.classList.remove("visible");
      this.log("bubble hide");
    }
  }

  private clearTimer(): void {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
  }
}
