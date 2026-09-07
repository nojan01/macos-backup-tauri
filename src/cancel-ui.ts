export function renderCancelControl(
  button: Pick<HTMLButtonElement, "disabled" | "textContent" | "style">,
  busy: boolean,
  cancelling: boolean,
  labels: { cancel: string; cancelling: string },
): void {
  button.style.display = busy ? "block" : "none";
  button.disabled = !busy || cancelling;
  button.textContent = cancelling ? labels.cancelling : labels.cancel;
}
