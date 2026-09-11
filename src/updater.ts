/**
 * Signed application updates via the Tauri updater.
 *
 * The updater downloads only a release archive whose detached signature
 * matches the public key embedded in tauri.conf.json. The GitHub release is
 * therefore a transport location, not a trust decision.
 */
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ask } from "@tauri-apps/plugin-dialog";
import { check } from "@tauri-apps/plugin-updater";

export type Translate = (key: string, ...values: (string | number)[]) => string;

export interface UpdateOptions {
  interactive?: boolean;
  isBusy: () => boolean;
  log: (message: string) => void;
  t: Translate;
}

let running = false;

async function titleReporter(appName: string): Promise<{
  show: (text: string) => void;
  reset: () => void;
}> {
  const win = getCurrentWindow();
  let original = appName;
  try {
    original = await win.title();
  } catch {
    // The updater can still work if the platform refuses the title lookup.
  }
  return {
    show: (text) => void win.setTitle(text).catch(() => {}),
    reset: () => void win.setTitle(original).catch(() => {}),
  };
}

/** Check, download, verify and install a new release after user consent. */
export async function checkForUpdates(options: UpdateOptions): Promise<void> {
  const { interactive = false, isBusy, log, t } = options;
  if (running) return;
  if (isBusy()) {
    if (interactive) log(t("updateBusy"));
    return;
  }
  running = true;

  const title = t("updateTitle");
  const reporter = await titleReporter("macOS Backup Suite");
  try {
    const update = await check();
    if (!update) {
      if (interactive) {
        const version = await getVersion();
        await ask(t("updateUpToDate", version), {
          title,
          kind: "info",
          okLabel: t("close"),
        });
      }
      return;
    }

    // The initial check is intentionally silent. A new release must never
    // interrupt a backup which may have started while the network request ran.
    if (isBusy()) {
      if (interactive) log(t("updateBusy"));
      return;
    }
    const install = await ask(t("updateAvailable", update.version, update.currentVersion), {
      title,
      kind: "info",
      okLabel: t("updateInstall"),
      cancelLabel: t("updateLater"),
    });
    if (!install || isBusy()) {
      if (isBusy()) log(t("updateBusy"));
      return;
    }

    let total = 0;
    let loaded = 0;
    reporter.show(t("updatePreparing"));
    await update.downloadAndInstall((event) => {
      switch (event.event) {
        case "Started":
          total = event.data.contentLength ?? 0;
          break;
        case "Progress":
          loaded += event.data.chunkLength;
          reporter.show(
            total > 0
              ? t("updateDownloading", Math.round((loaded / total) * 100))
              : t("updatePreparing"),
          );
          break;
        case "Finished":
          reporter.show(t("updateInstalling"));
          break;
      }
    });

    reporter.reset();
    await ask(t("updateDone", update.version), {
      title,
      kind: "info",
      okLabel: t("updateRestart"),
    });
    await invoke("restart_app");
  } catch (error) {
    reporter.reset();
    const detail = error instanceof Error ? error.message : String(error);
    console.error("Update check failed:", detail);
    if (interactive) {
      await ask(`${t("updateFailed")}\n\n${detail}`, {
        title,
        kind: "warning",
        okLabel: t("close"),
      });
    }
  } finally {
    reporter.reset();
    running = false;
  }
}
