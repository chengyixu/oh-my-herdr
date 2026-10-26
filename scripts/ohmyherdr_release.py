"""Build, verify and publish OhMyHerdr product releases from a local checkout.

OhMyHerdr does not use hosted CI, so this module is the only publication path.
Every artifact is compiled with the product identity and is refused unless the
binary itself proves it: `--version` must report `<version>-ohmyherdr.<version>`
and its server socket must live under the ohmyherdr configuration directory,
isolated from stable Herdr.

    python3 -m scripts.ohmyherdr_release build 0.9.9
    python3 -m scripts.ohmyherdr_release verify path/to/binary 0.9.9
    python3 -m scripts.ohmyherdr_release publish 0.9.9
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
import urllib.request
from pathlib import Path
from typing import Any, Callable, TypeVar

from scripts.changelog import (
    ChangelogError,
    extract_section_body,
    read_endpoint_protocol_generation,
    read_protocol_version,
)

ROOT = Path(__file__).resolve().parent.parent
REPOSITORY = "chengyixu/oh-my-herdr"
ASSET = "ohmyherdr-macos-aarch64"
MANIFEST = "ohmyherdr-latest.json"
RUST_TARGET = "aarch64-apple-darwin"
LATEST_MANIFEST_URL = f"https://github.com/{REPOSITORY}/releases/latest/download/{MANIFEST}"

T = TypeVar("T")


class ReleaseError(RuntimeError):
    pass


def product_build_env(version: str) -> dict[str, str]:
    return {
        "OHMYHERDR_BUILD": "1",
        "HERDR_BUILD_CHANNEL": "ohmyherdr",
        "HERDR_BUILD_ID": version,
        "LIBGHOSTTY_VT_OPTIMIZE": "ReleaseFast",
        "LIBGHOSTTY_VT_SIMD": "true",
    }


def identity_errors(version: str, version_output: str, status_output: str, home: Path) -> list[str]:
    """Explain why a binary is not this OhMyHerdr release; empty means it is."""
    errors = []
    expected = f"ohmyherdr {version}-ohmyherdr.{version}"
    if version_output.strip() != expected:
        errors.append(f"--version reported {version_output.strip()!r}, expected {expected!r}")
    socket = re.search(r"^\s*socket:\s*(\S+)\s*$", status_output, re.MULTILINE)
    product_dir = home / ".config" / "ohmyherdr"
    if socket is None:
        errors.append(f"status reported no server socket: {status_output.strip()!r}")
    elif Path(socket.group(1)).parent != product_dir:
        errors.append(f"server socket {socket.group(1)} is outside {product_dir}")
    return errors


def verify_binary(binary: Path, version: str) -> None:
    """Run the binary in an empty HOME so it can never reach a live runtime.

    The HOME stays short: macOS rejects socket paths over 104 bytes.
    """
    with tempfile.TemporaryDirectory(prefix="omh-", dir="/tmp") as home:
        env = {"HOME": home, "PATH": "/usr/bin:/bin"}

        def run(*args: str) -> str:
            return subprocess.run(
                [str(binary), *args],
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                timeout=60,
            ).stdout

        errors = identity_errors(version, run("--version"), run("status"), Path(home))
    if errors:
        raise ReleaseError(f"{binary} is not OhMyHerdr {version}: " + "; ".join(errors))


def source_version_errors(version: str, root: Path = ROOT) -> list[str]:
    errors = []
    found = {
        "Cargo.toml": tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"],
        "npm/package.json": json.loads((root / "npm" / "package.json").read_text())["version"],
    }
    for label, value in found.items():
        if value != version:
            errors.append(f"{label} version is {value}, expected {version}")
    try:
        extract_section_body((root / "CHANGELOG.md").read_text(), version)
    except ChangelogError as error:
        errors.append(f"CHANGELOG.md: {error}")
    return errors


def release_notes(version: str, changes: str, protocol: int, generation: int, commit: str) -> str:
    return (
        f"{changes.rstrip()}\n\n"
        "### Distribution\n"
        f"- Apple Silicon macOS binary: {ASSET}.\n"
        f"- Product identity: {version}-ohmyherdr.{version}.\n"
        f"- Private protocol: {protocol}; stable endpoint generation: {generation}.\n"
        f"- Source commit: {commit}.\n\n"
        "### Updating\n"
        "- Publishing this release does not install it or restart any existing runtime.\n"
        "- Run `ohmyherdr update` when ready, then follow the updater's runtime migration guidance.\n"
        "- Ohmyherdr updates use merged fork builds; raw upstream Herdr binaries do not replace the fork.\n"
    )


def release_manifest(
    version: str, protocol: int, generation: int, commit: str, notes: str, sha256: str
) -> dict[str, Any]:
    return {
        "version": version,
        "protocol": protocol,
        "endpoint_generation": generation,
        "source_commit": commit,
        "notes": notes,
        "assets": {
            "macos-aarch64": {
                "url": f"https://github.com/{REPOSITORY}/releases/download/ohmyherdr-v{version}/{ASSET}",
                "sha256": sha256,
            }
        },
    }


def build(version: str, output_dir: Path) -> Path:
    errors = source_version_errors(version)
    if errors:
        raise ReleaseError("; ".join(errors))
    env = {**os.environ, **product_build_env(version)}
    subprocess.run(
        ["cargo", "build", "--release", "--locked", "--target", RUST_TARGET],
        cwd=ROOT,
        env=env,
        check=True,
    )
    built = Path(ROOT, env.get("CARGO_TARGET_DIR", "target"), RUST_TARGET, "release", "herdr")
    output_dir.mkdir(parents=True, exist_ok=True)
    artifact = output_dir / ASSET
    shutil.copy2(built, artifact)
    artifact.chmod(0o755)
    verify_binary(artifact, version)
    return artifact


def git(*args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def fetch(url: str) -> bytes:
    with urllib.request.urlopen(url, timeout=120) as response:
        return response.read()


def retry(action: Callable[[], T], attempts: int = 10, delay: float = 6.0) -> T:
    """Public release and registry endpoints become consistent shortly after publication."""
    for attempt in range(attempts):
        try:
            return action()
        except Exception:
            if attempt == attempts - 1:
                raise
            time.sleep(delay)
    raise AssertionError("unreachable")


def verify_published_release(version: str, digest: str) -> None:
    def check() -> None:
        published = json.loads(fetch(LATEST_MANIFEST_URL))
        asset = published["assets"]["macos-aarch64"]
        if published["version"] != version or asset["sha256"] != digest:
            raise ReleaseError(f"latest manifest advertises {published['version']} {asset['sha256']}")
        with tempfile.TemporaryDirectory(prefix="ohmyherdr-published-") as directory:
            binary = Path(directory, ASSET)
            binary.write_bytes(fetch(asset["url"]))
            if sha256(binary.read_bytes()) != digest:
                raise ReleaseError("published asset checksum differs from the verified build")
            binary.chmod(0o755)
            verify_binary(binary, version)

    retry(check)


def verify_npm_package(version: str) -> None:
    """A new npm version can take minutes to become installable; never trust npm's cached metadata."""

    def check() -> None:
        with tempfile.TemporaryDirectory(prefix="ohmyherdr-npm-") as prefix:
            subprocess.run(
                ["npm", "install", "--prefer-online", "--prefix", prefix, "--no-audit", "--no-fund",
                 f"ohmyherdr@{version}"],
                check=True,
            )
            verify_binary(Path(prefix, "node_modules", "ohmyherdr", "bin", "ohmyherdr-native"), version)

    retry(check, attempts=30, delay=10.0)


def publish(version: str) -> None:
    commit = git("rev-parse", "HEAD")
    if git("status", "--porcelain"):
        raise ReleaseError("working tree is not clean")
    remote_main = git("ls-remote", "origin", "refs/heads/main").split()
    if not remote_main or remote_main[0] != commit:
        raise ReleaseError("publish only the pushed origin/main commit")
    tag = f"ohmyherdr-v{version}"
    if git("ls-remote", "origin", f"refs/tags/{tag}"):
        raise ReleaseError(f"{tag} already exists")

    protocol = read_protocol_version(ROOT / "src" / "protocol" / "wire.rs")
    generation = read_endpoint_protocol_generation(ROOT / "src" / "protocol" / "endpoint.rs")
    changes = extract_section_body((ROOT / "CHANGELOG.md").read_text(), version)
    with tempfile.TemporaryDirectory(prefix="ohmyherdr-release-") as directory:
        out = Path(directory)
        artifact = build(version, out)
        digest = sha256(artifact.read_bytes())
        notes = release_notes(version, changes, protocol, generation, commit)
        (out / "release-notes.md").write_text(notes)
        manifest = release_manifest(version, protocol, generation, commit, notes, digest)
        (out / MANIFEST).write_text(json.dumps(manifest, indent=2) + "\n")
        gh = ["gh", "release", "-R", REPOSITORY]
        subprocess.run(
            [*gh, "create", tag, str(artifact), str(out / MANIFEST), "--draft", "--target", commit,
             "--title", f"OhMyHerdr v{version}", "--notes-file", str(out / "release-notes.md")],
            check=True,
        )
        subprocess.run([*gh, "edit", tag, "--draft=false", "--latest"], check=True)
    verify_published_release(version, digest)
    subprocess.run(["npm", "publish"], cwd=ROOT / "npm", check=True)
    verify_npm_package(version)
    print(f"published OhMyHerdr {version} ({digest}) to GitHub Releases and npm")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    build_parser = commands.add_parser("build", help="build and verify a product artifact")
    build_parser.add_argument("version")
    build_parser.add_argument("--output", type=Path, default=ROOT / ".local" / "ohmyherdr-release")
    verify_parser = commands.add_parser("verify", help="verify an existing binary's product identity")
    verify_parser.add_argument("binary", type=Path)
    verify_parser.add_argument("version")
    publish_parser = commands.add_parser("publish", help="build, verify and publish to GitHub Releases and npm")
    publish_parser.add_argument("version")
    args = parser.parse_args(argv)

    try:
        if args.command == "build":
            print(build(args.version, args.output))
        elif args.command == "verify":
            verify_binary(args.binary, args.version)
            print(f"{args.binary} is OhMyHerdr {args.version}")
        else:
            publish(args.version)
    except (ReleaseError, ChangelogError, subprocess.CalledProcessError) as error:
        print(f"ohmyherdr release: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
