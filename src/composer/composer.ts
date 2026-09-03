// 秘書に話しかける入力欄(Phase 10)。Enter で送り、Esc で閉じる。
// 送り先の選択や channel への配送は Rust 側(server::Core::send_prompt)が行う。

export class Composer {
  constructor(
    private readonly form: HTMLFormElement,
    private readonly input: HTMLInputElement,
    private readonly onSubmit: (text: string) => Promise<void>,
    private readonly onToggle: (open: boolean) => void = () => {},
  ) {
    form.addEventListener("submit", (e) => {
      e.preventDefault();
      void this.submit();
    });
    input.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        this.close();
      }
    });
  }

  get isOpen(): boolean {
    return !this.form.hidden;
  }

  open(): void {
    if (!this.isOpen) {
      this.form.hidden = false;
      this.onToggle(true);
    }
    this.input.focus();
  }

  close(): void {
    if (!this.isOpen) return;
    this.form.hidden = true;
    this.input.blur();
    this.onToggle(false);
  }

  toggle(): void {
    if (this.isOpen) this.close();
    else this.open();
  }

  private async submit(): Promise<void> {
    const text = this.input.value.trim();
    if (!text || this.input.disabled) return;
    this.input.disabled = true;
    try {
      await this.onSubmit(text);
      this.input.value = "";
      this.close();
    } catch {
      // 失敗の案内は呼び出し側が吹き出しに出す。文面は残して、直して再送できるようにする
    } finally {
      this.input.disabled = false;
      if (this.isOpen) this.input.focus();
    }
  }
}
