#!/usr/bin/env node

const { spawnSync } = require("node:child_process");
const path = require("node:path");

const executable = path.join(__dirname, "ohmyherdr-native");
const result = spawnSync(executable, process.argv.slice(2), { stdio: "inherit" });

if (result.error) {
  console.error(`OhMyHerdr's native binary is unavailable: ${result.error.message}`);
  console.error("Reinstall with: npm install --global ohmyherdr");
  process.exit(1);
}

process.exit(result.status ?? 1);
