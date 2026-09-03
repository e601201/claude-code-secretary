// ステータスパネル。追跡中のセッション、状態、直近の依頼、実行中ツールを足元に出す。
// 作業中・許可待ちなどのときだけ展開し、待機に戻ってしばらくしたら畳む。

import type { AssistantState } from "../generated/AssistantState";
import type { SecretarySnapshot } from "../generated/SecretarySnapshot";

export const STATUS_LABELS: Record<AssistantState, string> = {
  idle: "待機中",
  thinking: "考え中",
  working: "作業中",
  waiting: "許可待ち",
  success: "完了",
  error: "エラー",
};

export type PanelMode = "show" | "linger" | "hide";

/** 表示するか。idle は「しばらく残してから畳む」 */
export function panelMode(snapshot: SecretarySnapshot): PanelMode {
  if (snapshot.tracked_sessions === 0 || !snapshot.session_id) return "hide";
  return snapshot.status === "idle" ? "linger" : "show";
}

/** 3 行目に出す文言。許可待ちを優先し、次に実行中ツール */
export function toolLine(snapshot: SecretarySnapshot): string {
  if (snapshot.status === "waiting" && snapshot.pending_permission) {
    return `許可待ち: ${snapshot.pending_permission}`;
  }
  return snapshot.current_tool ?? "";
}

export class StatusPanel {
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    private readonly root: HTMLElement,
    private readonly els: {
      status: HTMLElement;
      session: HTMLElement;
      task: HTMLElement;
      tool: HTMLElement;
    },
    private readonly lingerMs = 4000,
    private readonly log: (message: string) => void = () => {},
  ) {}

  update(snapshot: SecretarySnapshot): void {
    this.render(snapshot);
    switch (panelMode(snapshot)) {
      case "show":
        this.clearTimer();
        this.setVisible(true);
        break;
      case "linger":
        if (this.root.classList.contains("visible") && this.timer === null) {
          this.timer = setTimeout(() => {
            this.timer = null;
            this.setVisible(false);
          }, this.lingerMs);
        }
        break;
      case "hide":
        this.clearTimer();
        this.setVisible(false);
        break;
    }
  }

  private render(snapshot: SecretarySnapshot): void {
    this.root.dataset.status = snapshot.status;
    this.els.status.textContent = STATUS_LABELS[snapshot.status];
    this.els.session.textContent = snapshot.session_label ?? "";
    this.els.task.textContent = snapshot.task_summary ?? "";
    this.els.tool.textContent = toolLine(snapshot);
    this.els.task.hidden = !snapshot.task_summary;
    this.els.tool.hidden = !this.els.tool.textContent;
  }

  private setVisible(visible: boolean): void {
    if (this.root.classList.contains("visible") === visible) return;
    this.root.classList.toggle("visible", visible);
    this.log(`panel ${visible ? "show" : "hide"}`);
  }

  private clearTimer(): void {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
  }
}
