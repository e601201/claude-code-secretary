// 秘書に話しかける入力欄(Phase 10)。Enter で送り、Esc で閉じる。
// 話し相手(どのセッションに届くか)と、その文言・送ってよいかは Rust 側
// (server::Core::talk_partner)が決めて TalkPartner で流してくる。ここではそれを描き、
// 送るときは表示していた相手の session_id をそのまま渡すだけ。

import type { TalkPartner } from "../generated/TalkPartner";

export interface ComposerElements {
  form: HTMLFormElement;
  input: HTMLInputElement;
  /** 話し相手を出す行 */
  partner: HTMLElement;
  send: HTMLButtonElement;
}

export interface ComposerHooks {
  /** 送る。`sessionId` は表示していた話し相手 */
  onSubmit: (text: string, sessionId: string | null) => Promise<void>;
  /** 開いたとき。Rust 側が話し相手を決め、TalkPartner のイベントで流してくる */
  onOpen: () => void;
  /** 閉じたとき。Rust 側の話し相手を捨てる */
  onClose: () => void;
  onToggle?: (open: boolean) => void;
}

export class Composer {
  /** 表示中の話し相手。Rust から届くまでと閉じている間は null(送れない) */
  private partner: TalkPartner | null = null;

  constructor(
    private readonly els: ComposerElements,
    private readonly hooks: ComposerHooks,
  ) {
    els.form.addEventListener("submit", (e) => {
      e.preventDefault();
      void this.submit();
    });
    els.input.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        this.close();
      }
    });
  }

  get isOpen(): boolean {
    return !this.els.form.hidden;
  }

  open(): void {
    if (!this.isOpen) {
      this.els.form.hidden = false;
      this.render(null);
      this.hooks.onToggle?.(true);
      this.hooks.onOpen();
    }
    this.els.input.focus();
  }

  close(): void {
    if (!this.isOpen) return;
    this.els.form.hidden = true;
    this.els.input.blur();
    this.render(null);
    this.hooks.onClose();
    this.hooks.onToggle?.(false);
  }

  toggle(): void {
    if (this.isOpen) this.close();
    else this.open();
  }

  /** Rust から届いた話し相手を描く。閉じている間に届いたものは無視する(開き直せば選び直す) */
  setPartner(partner: TalkPartner): void {
    if (!this.isOpen) return;
    this.render(partner);
  }

  private render(partner: TalkPartner | null): void {
    this.partner = partner;
    this.els.partner.textContent = partner?.line ?? "";
    this.els.partner.dataset.state = partner?.can_send ? "ready" : "blocked";
    this.els.send.disabled = !partner?.can_send;
  }

  private async submit(): Promise<void> {
    const text = this.els.input.value.trim();
    if (!text || this.els.input.disabled || !this.partner?.can_send) return;
    this.els.input.disabled = true;
    try {
      await this.hooks.onSubmit(text, this.partner.session_id);
      this.els.input.value = "";
      this.close();
    } catch {
      // 失敗の案内は呼び出し側が吹き出しに出す。文面は残して、直して再送できるようにする
    } finally {
      this.els.input.disabled = false;
      if (this.isOpen) this.els.input.focus();
    }
  }
}
