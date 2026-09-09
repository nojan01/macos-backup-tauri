/** Unknown totals stay visibly active without inventing a completion percentage. */
export class ProgressIndicator {
  private busy = false;
  private value: number | null = null;

  constructor(
    private fill: HTMLDivElement,
    private bar: HTMLElement,
    private label: HTMLElement,
    private labels: () => { unknown: string; overall: string },
  ) {}

  setBusy(busy: boolean): void {
    if (busy && !this.busy) this.value = null;
    this.busy = busy;
    this.render();
  }

  reset(): void {
    this.value = null;
    this.render();
  }

  update(value: number): void {
    if (!Number.isFinite(value)) return;
    const bounded = Math.max(0, Math.min(100, value));
    // A zero event during preparation does not mean a known, empty workload.
    // Backend phases may also report a smaller coarse value than an earlier one.
    if (bounded > 0) this.value = Math.max(this.value ?? 0, bounded);
    this.render();
  }

  refresh(): void { this.render(); }

  private render(): void {
    const unknown = this.busy && this.value === null;
    const labels = this.labels();
    this.fill.classList.toggle("indeterminate", unknown);
    this.fill.classList.toggle("animating", this.busy && !unknown && (this.value ?? 0) < 100);
    this.fill.style.width = unknown ? "32%" : `${this.value ?? 0}%`;
    this.bar.setAttribute("role", "progressbar");
    this.bar.setAttribute("aria-valuemin", "0");
    this.bar.setAttribute("aria-valuemax", "100");
    this.bar.setAttribute("aria-busy", String(this.busy));
    this.label.textContent = unknown ? labels.unknown : this.value === null ? "" : `${labels.overall}: ${Math.round(this.value)} %`;
    if (this.value === null) this.bar.removeAttribute("aria-valuenow");
    else this.bar.setAttribute("aria-valuenow", String(this.value));
    this.bar.setAttribute("aria-valuetext", this.label.textContent);
  }
}
