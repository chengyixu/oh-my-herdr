const crypto = require("node:crypto");
const fs = require("node:fs");
const https = require("node:https");
const path = require("node:path");

const packageJson = require("../package.json");
const releaseBaseUrl = "https://github.com/chengyixu/oh-my-herdr/releases/download";
const manifestUrl = `${releaseBaseUrl}/ohmyherdr-v${packageJson.version}/ohmyherdr-latest.json`;
const nativePath = path.join(__dirname, "..", "bin", "ohmyherdr-native");

function download(url, redirects = 0) {
  return new Promise((resolve, reject) => {
    https
      .get(url, (response) => {
        if (
          response.statusCode >= 300 &&
          response.statusCode < 400 &&
          response.headers.location
        ) {
          response.resume();
          if (redirects >= 5) {
            reject(new Error("too many redirects while downloading OhMyHerdr"));
            return;
          }
          resolve(download(new URL(response.headers.location, url), redirects + 1));
          return;
        }

        if (response.statusCode !== 200) {
          response.resume();
          reject(new Error(`download failed with HTTP ${response.statusCode}`));
          return;
        }

        const chunks = [];
        response.on("data", (chunk) => chunks.push(chunk));
        response.on("end", () => resolve(Buffer.concat(chunks)));
        response.on("error", reject);
      })
      .on("error", reject);
  });
}

function productAsset(manifest, version) {
  if (manifest.version !== version) {
    throw new Error(`expected OhMyHerdr ${version}, received ${manifest.version}`);
  }

  const asset = manifest.assets?.["macos-aarch64"];
  if (!asset?.url || !/^[a-f0-9]{64}$/i.test(asset.sha256)) {
    throw new Error("OhMyHerdr release manifest is missing a valid macOS ARM64 asset");
  }

  return asset;
}

function verifyChecksum(binary, expected) {
  const actual = crypto.createHash("sha256").update(binary).digest("hex");
  if (actual.toLowerCase() !== expected.toLowerCase()) {
    throw new Error("OhMyHerdr download checksum verification failed");
  }
}

async function install() {
  if (process.platform !== "darwin" || process.arch !== "arm64") {
    throw new Error("ohmyherdr currently supports Apple Silicon macOS only");
  }

  const manifest = JSON.parse((await download(manifestUrl)).toString("utf8"));
  const asset = productAsset(manifest, packageJson.version);
  const binary = await download(asset.url);
  verifyChecksum(binary, asset.sha256);

  const temporaryPath = `${nativePath}.${process.pid}.tmp`;
  fs.writeFileSync(temporaryPath, binary, { mode: 0o755 });
  fs.renameSync(temporaryPath, nativePath);
  console.log(`installed OhMyHerdr ${packageJson.version}`);
}

if (require.main === module) {
  install().catch((error) => {
    console.error(`OhMyHerdr installation failed: ${error.message}`);
    process.exitCode = 1;
  });
}

module.exports = { productAsset, verifyChecksum };
