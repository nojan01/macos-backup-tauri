export function createRestoreRow(path: string, icon: string, size: string, doc: Document = document): HTMLDivElement {
  const row = doc.createElement("div");
  row.className = "restore-item";
  const checkbox = doc.createElement("input");
  checkbox.type = "checkbox";
  checkbox.className = "restore-checkbox";
  checkbox.value = path;
  checkbox.checked = true;
  const symbol = doc.createElement("span");
  symbol.className = "restore-item-icon";
  symbol.textContent = icon;
  const info = doc.createElement("div");
  info.className = "restore-item-info";
  const label = doc.createElement("div");
  label.className = "restore-item-path";
  label.textContent = path;
  const detail = doc.createElement("div");
  detail.className = "restore-item-size";
  detail.textContent = size;
  info.append(label, detail);
  row.append(checkbox, symbol, info);
  return row;
}

export type BrowserRestoreGroup = "chrome" | "firefox";

/**
 * Browser settings are stored as individual, verified archives. Group them in
 * the restore UI so users can select a browser as one meaningful choice while
 * retaining the precise archive paths internally.
 */
export function browserRestoreGroup(path: string): BrowserRestoreGroup | null {
  const relative = path.startsWith("~/") ? path.slice(2) : path;
  if (relative.startsWith("Library/Application Support/Google/Chrome/")) return "chrome";
  if (relative.startsWith("Library/Application Support/Firefox/")) return "firefox";
  return null;
}

export function restoreStatusKey(result: { error_count: number; restored_count: number }): "restoreWithErrors" | "restoreNothingChanged" | "restoreComplete" {
  if (result.error_count > 0) return "restoreWithErrors";
  return result.restored_count > 0 ? "restoreComplete" : "restoreNothingChanged";
}
