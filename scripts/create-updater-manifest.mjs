import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { basename, resolve } from "node:path";

const [version, platform, artifact, output = "latest.json"] = process.argv.slice(2);

if (!version || !platform || !artifact) {
  console.error("Aufruf: npm run make-updater-manifest -- <version> <plattform> <archiv> [ausgabe]");
  process.exit(1);
}
if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`Keine gültige semantische Version: ${version}`);
  process.exit(1);
}

const artifactPath = resolve(artifact);
const signaturePath = `${artifactPath}.sig`;
if (!existsSync(artifactPath) || !existsSync(signaturePath)) {
  console.error("Updater-Archiv oder dessen Signaturdatei fehlt. Wurde mit TAURI_SIGNING_PRIVATE_KEY gebaut?");
  process.exit(1);
}
const signature = readFileSync(signaturePath, "utf8").trim();
if (!signature) {
  console.error("Die Updater-Signatur ist leer.");
  process.exit(1);
}

const assetName = basename(artifactPath);
const manifest = {
  version,
  notes: `macOS Backup Suite ${version}`,
  pub_date: new Date().toISOString(),
  platforms: {
    [platform]: {
      signature,
      url: `https://github.com/nojan01/macos-backup-tauri/releases/download/v${version}/${encodeURIComponent(assetName)}`,
    },
  },
};

const outputPath = resolve(output);
writeFileSync(outputPath, `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`Updater-Manifest erstellt: ${outputPath}`);
