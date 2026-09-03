// 吹き出し。スナップショットの message / message_kind を受け取り、種別に応じた見た目と時間で表示する。
// 何を話すかは Rust 側が決める。ここでは「どう見せるか」だけを扱う。
// Phase 10: channel 経由で中継された権限要求(relayed_permission)があれば、内容と許可 / 拒否ボタンも出す。

import type { RelayedPermission } from "../generated/RelayedPermission";
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

/** input_preview(JSON 風の文字列)を「キー: 値」の行に崩す。JSON でなければそのまま 1 行 */
export function previewLines(raw: string): string[] {
  const trimmed = raw.trim();
  if (!trimmed) return [];
  try {
    const parsed: unknown = JSON.parse(trimmed);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      return Object.entries(parsed as Record<string, unknown>).map(
        ([key, value]) => `${key}: ${typeof value === "string" ? value : JSON.stringify(value)}`,
      );
    }
  } catch {
    // JSON ではない(Claude Code は表示用の整形済み文字列を送ることもある)
  }
  return [trimmed];
}

/** ボタンの上に出す説明。説明文と引数のプレビューを合わせ、長ければ末尾を省く */
export function permissionDetail(permission: RelayedPermission, maxChars = 160): string {
  const lines: string[] = [];
  const description = permission.description.trim();
  if (description) lines.push(description);
  lines.push(...previewLines(permission.input_preview));
  const joined = lines.join("\n");
  const chars = [...joined];
  return chars.length <= maxChars ? joined : `${chars.slice(0, maxChars - 1).join("")}…`;
}

export interface PermissionControls {
  detail: HTMLElement;
  actions: HTMLElement;
  allow: HTMLButtonElement;
  deny: HTMLButtonElement;
}

export type PermissionResponder = (
  allow: boolean,
  permission: RelayedPermission,
  sessionId: string | null,
) => Promise<void>;

export class SpeechBubble {
  private timer: ReturnType<typeof setTimeout> | null = null;
  /** 直近に表示した文言の識別子。同じ文言のスナップショットが続いても再表示しない */
  private lastKey: string | null = null;
  private permission: RelayedPermission | null = null;
  private sessionId: string | null = null;
  private responder: PermissionResponder | null = null;

  constructor(
    private readonly root: HTMLElement,
    private readonly textEl: HTMLElement,
    private readonly log: (message: string) => void = () => {},
    private readonly controls: PermissionControls | null = null,
  ) {
    if (controls) {
      controls.allow.addEventListener("click", () => void this.respond(true));
      controls.deny.addEventListener("click", () => void this.respond(false));
    }
  }

  /** 許可 / 拒否が押されたときの処理。Rust 側へ渡す */
  onPermission(responder: PermissionResponder): void {
    this.responder = responder;
  }

  /** ボタンを出している間 true。ウィンドウの当たり判定を広げる目安になる */
  get hasActions(): boolean {
    return this.permission !== null;
  }

  update(snapshot: SecretarySnapshot): void {
    const text = snapshot.message;
    const permission = snapshot.relayed_permission ?? null;
    if (!text) {
      this.lastKey = null;
      this.setPermission(null, null);
      this.hide();
      return;
    }
    const kind: SpeechKind = snapshot.message_kind ?? "assistant";
    const key = `${kind}:${text}:${permission?.request_id ?? ""}`;
    if (key === this.lastKey) return;
    this.lastKey = key;
    this.setPermission(permission, snapshot.session_id ?? null);
    this.show(text, kind, permission ? 0 : durationFor(text, kind));
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

  private setPermission(permission: RelayedPermission | null, sessionId: string | null): void {
    this.permission = permission;
    this.sessionId = sessionId;
    const c = this.controls;
    if (!c) return;
    if (permission) {
      c.detail.textContent = permissionDetail(permission);
      c.detail.hidden = false;
      c.actions.hidden = false;
      c.allow.disabled = false;
      c.deny.disabled = false;
    } else {
      c.detail.hidden = true;
      c.actions.hidden = true;
    }
  }

  private async respond(allow: boolean): Promise<void> {
    const permission = this.permission;
    const c = this.controls;
    if (!permission || !c || !this.responder) return;
    // 二度押しを防ぐ。次のスナップショットでボタンごと消える
    c.allow.disabled = true;
    c.deny.disabled = true;
    this.log(`permission ${allow ? "allow" : "deny"} id=${permission.request_id}`);
    try {
      await this.responder(allow, permission, this.sessionId);
    } catch (e) {
      this.log(`permission respond failed: ${String(e)}`);
      c.allow.disabled = false;
      c.deny.disabled = false;
    }
  }

  private clearTimer(): void {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
  }
}
