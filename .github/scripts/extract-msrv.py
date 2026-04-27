#!/usr/bin/env python3
"""Print the rust-version (MSRV) declared by the qorrection package.

Reads `cargo metadata --no-deps --format-version 1` JSON from stdin
and prints the `rust_version` field of the package named
`qorrection`. Exits with a clear diagnostic if the package is missing
or has no MSRV declared.

This file lives in `.github/scripts/` so the CI workflow does not
have to embed Python source inside a YAML run step (which forces a
fragile dependency on the surrounding YAML indentation).
"""

from __future__ import annotations

import json
import sys


def main() -> int:
    data = json.load(sys.stdin)
    pkg = next(
        (p for p in data.get("packages", []) if p.get("name") == "qorrection"),
        None,
    )
    if pkg is None:
        print(
            "error: package 'qorrection' not found in cargo metadata output",
            file=sys.stderr,
        )
        return 1
    rv = pkg.get("rust_version")
    if not rv:
        print(
            "error: package 'qorrection' has no rust-version field",
            file=sys.stderr,
        )
        return 1
    print(rv)
    return 0


if __name__ == "__main__":
    sys.exit(main())
