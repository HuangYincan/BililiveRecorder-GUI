#!/usr/bin/env python3
"""Disposable-runner-only old-name -> new-name Tauri updater integration fixture.

No production signing key, endpoint, app, or user data is used. The old app's
only source change is explicit test consent in place of its interactive dialog;
the real plugin check, signature-verified download/install and restart remain.
"""
import hashlib
import http.server
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import threading
import time
from urllib.parse import quote


def run(command, *, cwd=None, env=None, log=None):
    with open(log, "a", encoding="utf-8") as out:
        outcome = subprocess.run(command, cwd=cwd, env=env, stdout=out, stderr=out, check=False)
    if outcome.returncode:
        raise RuntimeError(f"Fixture command {command[0]} failed ({outcome.returncode}); private runner log withheld")


def verify(new_app, archive, signature, old_source, fixture_dir, public_key, version, target):
    fixture_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    app_dir = fixture_dir / "Applications"
    app_dir.mkdir(exist_ok=True)
    trace = fixture_dir / "lifecycle.log"
    trace.touch()
    data = fixture_dir / "recordings-fixture"
    data.mkdir()
    marker = data / "fixture-only.txt"
    marker.write_bytes(b"isolated updater path preservation fixture\n")
    original_hash = hashlib.sha256(marker.read_bytes()).hexdigest()
    log = fixture_dir / "fixture-build.log"
    log.touch(mode=0o600)

    class QuietFiles(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=str(fixture_dir), **kwargs)

        def log_message(self, *_args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), QuietFiles)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    port = server.server_address[1]
    try:
        manifest = {
            "version": version,
            "platforms": {
                f"darwin-{target}": {
                    "signature": signature.read_text(encoding="utf-8").strip(),
                    "url": f"http://127.0.0.1:{port}/{quote(archive.name)}",
                }
            },
        }
        (fixture_dir / "latest.json").write_text(json.dumps(manifest), encoding="utf-8")

        config_path = old_source / "src-tauri" / "tauri.conf.json"
        config = json.loads(config_path.read_text(encoding="utf-8"))
        config["plugins"]["updater"] = {
            "pubkey": public_key.read_text(encoding="utf-8").strip(),
            "endpoints": [f"http://127.0.0.1:{port}/latest.json"],
            "dangerousInsecureTransportProtocol": True,
            "requireSignedVersion": True,
        }
        config_path.write_text(json.dumps(config, ensure_ascii=False, indent=2), encoding="utf-8")
        source_path = old_source / "src-tauri" / "src" / "lib.rs"
        source = source_path.read_text(encoding="utf-8")
        start = source.index("    let accepted = app\n        .dialog()")
        end = source.index("    if !accepted {", start)
        source = source[:start] + "    let accepted = true; // disposable runner test consent only\n" + source[end:]
        needle = "        Ok(()) => {\n            // Hand the backend over before restarting"
        assert source.count(needle) == 1, "Old updater flow changed; fixture patch no longer safe"
        source = source.replace(needle, '        Ok(()) => {\n            artifact_probe::record("updater-fixture-installed");\n            // Hand the backend over before restarting')
        source_path.write_text(source, encoding="utf-8")

        run(["npm", "ci"], cwd=old_source, log=log)
        run(["npm", "run", "sidecar:prepare"], cwd=old_source, log=log)
        run(["npm", "run", "tauri", "--", "build", "--target", f"{target}-apple-darwin", "--bundles", "app", "--config", '{"version":"2.20.0","bundle":{"createUpdaterArtifacts":false}}'],
            cwd=old_source, log=log)
        old_built = old_source / "src-tauri" / "target" / f"{target}-apple-darwin" / "release" / "bundle" / "macos" / "BililiveRecorder GUI.app"
        old_installed = app_dir / old_built.name
        run(["ditto", str(old_built), str(old_installed)], log=log)
        assert old_installed.is_dir() and new_app.is_dir()
        executable = old_installed / "Contents" / "MacOS" / "bililive-recorder-gui"
        environment = dict(os.environ,
                           BILILIVE_RECORDER_GUI_WORKDIR=str(data),
                           BILILIVE_ARTIFACT_OK="1",
                           BILILIVE_ARTIFACT_TRACE=str(trace))
        with open(fixture_dir / "old-app.log", "wb") as app_log:
            old_process = subprocess.Popen([str(executable)], env=environment, stdout=app_log, stderr=subprocess.STDOUT)
            deadline = time.monotonic() + 150
            ready_new = False
            installed = False
            while time.monotonic() < deadline:
                entries = trace.read_text(encoding="utf-8").splitlines()
                installed = f"{old_process.pid} updater-fixture-installed" in entries
                ready_new = any(line.endswith(" main-window-ready") and line.split(" ", 1)[0] != str(old_process.pid)
                                for line in entries)
                if installed and ready_new:
                    break
                if old_process.poll() is not None and not installed:
                    raise AssertionError("Old fixture exited without real updater installation")
                time.sleep(0.3)
            if not installed or not ready_new:
                raise AssertionError(f"Updater/restart did not complete within 150s: installed={installed}, new-ready={ready_new}")

        old_process.wait(timeout=30)
        assert old_installed.is_dir(), "Updater lost the old installation path"
        assert not (app_dir / "Mikufans录播姬.app").exists(), "Updater unexpectedly moved the bundle path"
        info = plistlib.loads((old_installed / "Contents" / "Info.plist").read_bytes())
        assert info["CFBundleName"] == "Mikufans录播姬", info
        assert info["CFBundleIdentifier"] == "org.danmuji.bililiverecorder-gui"
        assert info["CFBundleShortVersionString"] == version
        installed_icon = old_installed / "Contents" / "Resources" / "icon.icns"
        new_icon = new_app / "Contents" / "Resources" / "icon.icns"
        assert installed_icon.is_file() and installed_icon.read_bytes() == new_icon.read_bytes(), "New app icon not installed"
        assert marker.is_file() and hashlib.sha256(marker.read_bytes()).hexdigest() == original_hash
        print("Real fixture updater: signed local download/install/restart observed; old .app path retained; new bundle name/icon/version/identifier and fixture data verified")
    finally:
        server.shutdown()
        subprocess.run(["osascript", "-e", 'tell application id "org.danmuji.bililiverecorder-gui" to quit'],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False, timeout=10)


if __name__ == "__main__":
    verify(Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3]), Path(sys.argv[4]),
           Path(sys.argv[5]), Path(sys.argv[6]), sys.argv[7], sys.argv[8])
