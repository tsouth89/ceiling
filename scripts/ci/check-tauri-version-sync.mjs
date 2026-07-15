import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..", "..");
const packageJson = JSON.parse(
  readFileSync(resolve(root, "apps", "desktop-tauri", "package.json"), "utf8"),
);
const cargoLock = readFileSync(resolve(root, "Cargo.lock"), "utf8");
const rustMatch = cargoLock.match(
  /\[\[package\]\]\r?\nname = "tauri"\r?\nversion = "([^"]+)"/,
);

if (!rustMatch) {
  throw new Error("Could not find the resolved tauri crate version in Cargo.lock.");
}

const versions = {
  "tauri (Rust)": rustMatch[1],
  "@tauri-apps/api": packageJson.dependencies?.["@tauri-apps/api"],
  "@tauri-apps/cli": packageJson.devDependencies?.["@tauri-apps/cli"],
};

function majorMinor(name, version) {
  const match = /^(\d+)\.(\d+)/.exec(version ?? "");
  if (!match) {
    throw new Error(`${name} must use an explicit numeric version; received ${version ?? "missing"}.`);
  }
  return `${match[1]}.${match[2]}`;
}

const expected = majorMinor("tauri (Rust)", versions["tauri (Rust)"]);
const mismatches = Object.entries(versions).filter(
  ([name, version]) => majorMinor(name, version) !== expected,
);

if (mismatches.length > 0) {
  const summary = Object.entries(versions)
    .map(([name, version]) => `${name}=${version}`)
    .join(", ");
  throw new Error(`Tauri packages must share major/minor ${expected}: ${summary}`);
}

console.log(
  `[check-tauri-version-sync] OK - Rust, API, and CLI use Tauri ${expected}.x`,
);
