import { execFileSync } from "node:child_process";
import fs from "node:fs";

const allowedLicenseFamilies = [
  "MIT", "Apache-2.0", "BSD", "ISC", "Zlib", "0BSD", "CC0-1.0",
  "Unlicense", "Unicode-3.0", "MPL-2.0", "LLVM-exception",
];
const incompatibleMarkers = ["AGPL", "GPL", "SSPL", "BUSL", "EUPL"];

function licenseIsAcceptable(license) {
  if (!license || license === "NOASSERTION") return false;
  const hasAllowedAlternative = allowedLicenseFamilies.some(family => license.includes(family));
  const hasOnlyIncompatible = incompatibleMarkers.some(marker => license.includes(marker)) && !hasAllowedAlternative;
  return !hasOnlyIncompatible;
}

const cargoMetadata = JSON.parse(execFileSync(
  "cargo",
  ["metadata", "--manifest-path", "src-tauri/Cargo.toml", "--format-version", "1", "--locked"],
  { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 },
));
const rustPackages = cargoMetadata.packages.filter(pkg => pkg.name !== "macos-backup-suite");
const rustProblems = rustPackages.filter(pkg => !licenseIsAcceptable(pkg.license));

const lock = JSON.parse(fs.readFileSync("package-lock.json", "utf8"));
const npmPackages = Object.entries(lock.packages)
  .filter(([path, pkg]) => path.startsWith("node_modules/") && !pkg.dev)
  .map(([path, pkg]) => {
    const manifestPath = `${path}/package.json`;
    const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
    return { name: pkg.name ?? path.slice("node_modules/".length), license: manifest.license };
  });
const npmProblems = npmPackages.filter(pkg => !licenseIsAcceptable(pkg.license));

if (rustProblems.length || npmProblems.length) {
  for (const pkg of rustProblems) console.error(`Rust license review required: ${pkg.name} ${pkg.version} (${pkg.license ?? "NOASSERTION"})`);
  for (const pkg of npmProblems) console.error(`npm license review required: ${pkg.name} (${pkg.license ?? "NOASSERTION"})`);
  process.exit(1);
}

console.log(`License audit passed: ${rustPackages.length} Rust packages and ${npmPackages.length} production npm packages.`);
