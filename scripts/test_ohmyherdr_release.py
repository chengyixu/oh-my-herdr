import os
import tempfile
import unittest
from pathlib import Path

from scripts import ohmyherdr_release


def fake_binary(directory: Path, version_line: str, config_dir: str) -> Path:
    binary = directory / "fake-ohmyherdr"
    binary.write_text(
        "#!/bin/sh\n"
        'case "$1" in\n'
        f"  --version) echo '{version_line}' ;;\n"
        f"  status) printf 'server:\\n  status: not running\\n  socket: %s/.config/{config_dir}/{config_dir}.sock\\n' \"$HOME\" ;;\n"
        "esac\n"
    )
    binary.chmod(0o755)
    return binary


@unittest.skipIf(os.name == "nt", "OhMyHerdr releases are Apple Silicon macOS only")
class VerifyBinaryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ohmyherdr-release-test-")
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)

    def test_rejects_binary_built_without_product_identity(self):
        # The published 0.9.8 artifact: upstream identity, Herdr's socket and update channel.
        binary = fake_binary(self.directory, "herdr 0.9.8", "herdr")

        with self.assertRaises(ohmyherdr_release.ReleaseError) as raised:
            ohmyherdr_release.verify_binary(binary, "0.9.8")

        self.assertIn("expected 'ohmyherdr 0.9.8-ohmyherdr.0.9.8'", str(raised.exception))
        self.assertIn("/.config/herdr/herdr.sock is outside", str(raised.exception))

    def test_accepts_isolated_product_binary(self):
        binary = fake_binary(self.directory, "ohmyherdr 0.9.9-ohmyherdr.0.9.9", "ohmyherdr")

        ohmyherdr_release.verify_binary(binary, "0.9.9")


class SourceVersionTests(unittest.TestCase):
    def test_reports_every_unprepared_release_file(self):
        with tempfile.TemporaryDirectory(prefix="ohmyherdr-release-source-") as directory:
            root = Path(directory)
            (root / "npm").mkdir()
            (root / "Cargo.toml").write_text('[package]\nname = "herdr"\nversion = "0.9.9"\n')
            (root / "npm" / "package.json").write_text('{"version": "0.9.8"}')
            (root / "CHANGELOG.md").write_text("# Changelog\n\n## [0.9.8] - 2026-10-25\n\n- Old.\n")

            errors = ohmyherdr_release.source_version_errors("0.9.9", root)

        self.assertEqual(
            errors,
            [
                "npm/package.json version is 0.9.8, expected 0.9.9",
                "CHANGELOG.md: section not found: 0.9.9",
            ],
        )


if __name__ == "__main__":
    unittest.main()
