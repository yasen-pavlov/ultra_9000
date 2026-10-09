// Stamps a release version into every file the build reads. Release CI runs it on the
// tagged checkout before building, so the tag is the only place a version is decided;
// the committed files keep the 0.0.0 development placeholder.
//
//   bun scripts/set-version.ts 1.2.3
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const version = process.argv[2] ?? "";
if (!/^\d+\.\d+\.\d+$/.test(version)) {
  console.error(`usage: bun scripts/set-version.ts X.Y.Z (got "${version}")`);
  process.exit(1);
}

const root = join(import.meta.dir, "..");

function rewrite(path: string, pattern: RegExp, replacement: string): void {
  const file = join(root, path);
  const text = readFileSync(file, "utf8");
  const matches = text.match(new RegExp(pattern.source, pattern.flags.replace("g", "") + "g"));
  if (matches?.length !== 1) {
    throw new Error(`${path}: expected exactly one match for ${pattern}, found ${matches?.length ?? 0}`);
  }
  writeFileSync(file, text.replace(pattern, replacement));
}

// package.json: tauri.conf.json reads the version from here ("version": "../package.json").
rewrite("package.json", /^ {2}"version": "[^"]*",$/m, `  "version": "${version}",`);
// src-tauri/Cargo.toml: the [package] version of the app crate.
rewrite("src-tauri/Cargo.toml", /^(\[package\]\nname = "ultra9000"\nversion = )"[^"]*"/m, `$1"${version}"`);
// src-tauri/Cargo.lock: the app crate's entry, so `--locked`/`--frozen` builds accept it.
rewrite("src-tauri/Cargo.lock", /^(name = "ultra9000"\nversion = )"[^"]*"/m, `$1"${version}"`);
// packaging/arch/PKGBUILD: pkgver() reads package.json; keep the static value consistent.
rewrite("packaging/arch/PKGBUILD", /^pkgver=.*$/m, `pkgver=${version}`);

console.log(`Stamped version ${version}`);
