#!/usr/bin/env python3
"""Check that a release tag, demo packages, and Android APK agree on a version."""

import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib


ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    if len(sys.argv) not in (2, 3):
        print("usage: check-release-version.py vMAJOR.MINOR.PATCH [apk]", file=sys.stderr)
        return 2

    tag = sys.argv[1]
    match = re.fullmatch(r"v(\d+)\.(\d+)\.(\d+)", tag)
    if match is None:
        print(f"invalid release tag: {tag}", file=sys.stderr)
        return 1
    major, minor, patch = map(int, match.groups())
    if minor >= 1000 or patch >= 1000:
        print("minor and patch must be below 1000 for Android versionCode", file=sys.stderr)
        return 1
    version = tag[1:]
    code = major * 1_000_000 + minor * 1_000 + patch
    if not 1 <= code <= 2_100_000_000:
        print(f"Android versionCode is out of range: {code}", file=sys.stderr)
        return 1

    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
        cwd=ROOT,
    ))
    packages = {p["name"]: p["version"] for p in metadata["packages"]}
    for name in ("charon-app", "charon-sandbox"):
        if packages.get(name) != version:
            print(f"{name}: expected {version}, got {packages.get(name)}", file=sys.stderr)
            return 1

    with (ROOT / "apps/sandbox/Cargo.toml").open("rb") as source:
        sandbox = tomllib.load(source)
    android = sandbox["package"]["metadata"]["android"]
    if android.get("version_name") != version or android.get("version_code") != code:
        print(f"Android version mismatch: expected {version} (code {code})", file=sys.stderr)
        return 1
    app_dependency = sandbox["dependencies"]["charon-app"]["version"]
    if app_dependency != version:
        print(f"charon-app dependency: expected {version}, got {app_dependency}", file=sys.stderr)
        return 1

    if len(sys.argv) == 3:
        sdk = os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT")
        if not sdk:
            print("ANDROID_HOME or ANDROID_SDK_ROOT is required for APK inspection", file=sys.stderr)
            return 1
        aapt = Path(sdk) / "build-tools/35.0.0/aapt"
        badging = subprocess.check_output([str(aapt), "dump", "badging", sys.argv[2]], text=True)
        package_line = next((line for line in badging.splitlines() if line.startswith("package: ")), "")
        if f"versionCode='{code}'" not in package_line or f"versionName='{version}'" not in package_line:
            print(f"APK version mismatch: {package_line}", file=sys.stderr)
            return 1

    print(f"release version matches {tag} (Android versionCode {code})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
