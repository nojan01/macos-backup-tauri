// Mirrors the bounds enforced by the Rust backend (throttle.rs).
export const THROTTLE_MIN_MB_PER_S = 1;
export const THROTTLE_MAX_MB_PER_S = 5000;
export const THROTTLE_DEFAULT_MB_PER_S = 80;

/// Turns a raw settings-field value into a valid whole-number MB/s limit.
/// Non-numeric input falls back to `fallback`; out-of-range values are clamped.
export function normalizeThrottleMbPerS(
  raw: string | number | null | undefined,
  fallback: number = THROTTLE_DEFAULT_MB_PER_S,
): number {
  const parsed = typeof raw === "number" ? raw : Number.parseFloat(String(raw ?? "").trim());
  if (!Number.isFinite(parsed)) return fallback;
  const rounded = Math.round(parsed);
  return Math.min(THROTTLE_MAX_MB_PER_S, Math.max(THROTTLE_MIN_MB_PER_S, rounded));
}
