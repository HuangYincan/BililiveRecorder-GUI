#!/usr/bin/env python3
"""Check the *embedded* signed CLI entitlements, not just our source plist."""
import plistlib
import sys

expected = {"com.apple.security.cs.allow-jit": True}
if len(sys.argv) != 2:
    raise SystemExit("Usage: verify-macos-cli-entitlements.py <codesign-export.plist>")
try:
    with open(sys.argv[1], "rb") as source:
        actual = plistlib.load(source)
except (OSError, ValueError, plistlib.InvalidFileException) as error:
    raise SystemExit(f"Cannot inspect embedded CLI entitlements: {error}") from error
if actual != expected:
    raise SystemExit("CLI must have only allow-jit; missing JIT or unsafe extra entitlements")
print("CLI embedded entitlements: only com.apple.security.cs.allow-jit=true")
