import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";

const root = new URL("..", import.meta.url);
const read = (path) => fs.readFileSync(new URL(path, root), "utf8");

test("the updater uses a signed archive published with the GitHub release", () => {
  const config = JSON.parse(read("src-tauri/tauri.conf.json"));
  const updater = config.plugins?.updater;
  assert.equal(config.bundle.createUpdaterArtifacts, true);
  assert.match(updater.pubkey, /^dW50cnVzdGVkIGNvbW1lbnQ6/);
  assert.deepEqual(updater.endpoints, [
    "https://github.com/nojan01/macos-backup-tauri/releases/latest/download/latest.json",
  ]);

  const source = read("src/updater.ts");
  assert.match(source, /downloadAndInstall/);
  assert.match(source, /isBusy\(\)/);
  assert.match(source, /restart_app/);
});

test("the release manifest contains the archive signature and its versioned GitHub URL", () => {
  const source = read("scripts/create-updater-manifest.mjs");
  assert.match(source, /signaturePath/);
  assert.match(source, /releases\/download\/v\$\{version\}/);
  assert.match(source, /platforms/);
});
