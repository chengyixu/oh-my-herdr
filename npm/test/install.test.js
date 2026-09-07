const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const test = require("node:test");

const { productAsset, verifyChecksum } = require("../scripts/install.js");

const checksum = crypto.createHash("sha256").update("OhMyHerdr").digest("hex");
const manifest = {
  version: "0.8.5",
  assets: {
    "macos-aarch64": {
      url: "https://example.test/ohmyherdr-macos-aarch64",
      sha256: checksum
    }
  }
};

test("selects the version-pinned Apple Silicon asset", () => {
  assert.deepEqual(productAsset(manifest, "0.8.5"), manifest.assets["macos-aarch64"]);
});

test("rejects a different release version", () => {
  assert.throws(() => productAsset(manifest, "0.8.6"), /expected OhMyHerdr 0.8.6/);
});

test("rejects a manifest without a checksum", () => {
  assert.throws(
    () => productAsset({ version: "0.8.5", assets: { "macos-aarch64": {} } }, "0.8.5"),
    /missing a valid macOS ARM64 asset/
  );
});

test("verifies the downloaded binary checksum", () => {
  verifyChecksum(Buffer.from("OhMyHerdr"), checksum);
  assert.throws(() => verifyChecksum(Buffer.from("wrong"), checksum), /checksum verification failed/);
});
