"""Exercise two recordings in a provisioned interactive Windows acceptance account."""

import argparse
from contextlib import closing, contextmanager
import ctypes
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import stat
import subprocess
import sys
import tempfile
import time
import tomllib
import uuid

import numpy as np

from signals import Refused, interactive_windows, require, verify_mix


HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
APP_ID = "net.rismondo.openclaw.clawscribe"
PREFS = ("recording_preferences.json", "recording_mode.json")
MEETING_FILES = {"metadata.json", "recording-outcome.json", "transcripts.json", "audio.mp4"}


def json_file(path):
    return json.loads(path.read_text(encoding="utf-8-sig"))


def run_cmd(command, timeout, *, output=False):
    return subprocess.run(command, timeout=timeout, check=True, stdin=subprocess.DEVNULL,
                          stdout=subprocess.PIPE if output else subprocess.DEVNULL,
                          stderr=subprocess.DEVNULL, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0)).stdout


def audio_step(config, action, number=1):
    command = [sys.executable, "-B", HERE / "signals.py", action, "--config", config, "--round", str(number)]
    if action == "probe":
        command.extend(["--ffmpeg", installation()[0].with_name("ffmpeg.exe")])
    result = subprocess.run(command, capture_output=True, timeout=40)
    if result.returncode:
        # Only show fixed failure categories, not raw audio backend diagnostics.
        raise Refused(f"Virtual route {action} failed for recording {number}. Check endpoint isolation and calibration.")
    return json.loads(result.stdout)


def safe_path(path):
    require(path.is_absolute() and not path.anchor.startswith("\\\\"),
            "Use an absolute local path.")
    for part in (path, *path.parents):
        if part.exists() or part.is_symlink():
            info = part.lstat()
            require(not part.is_symlink() and not getattr(info, "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT,
                    "Refusing a link or reparse point in an acceptance path.")
    require(".." not in path.parts, "Refusing a parent traversal in an acceptance path.")
    return path


def atomic_write(path, data):
    temp = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as f:
            temp = Path(f.name)
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
        temp.replace(path)
    finally:
        if temp is not None:
            temp.unlink(missing_ok=True)


@contextmanager
def capture_preferences(profile, root, names, before_restore=None):
    # The directory stays on disk after an interrupted run for manual recovery.
    backup = profile / "installed-capture-preferences"
    old = {}
    for name in PREFS:
        path = safe_path(profile / name)
        old[name] = path.read_bytes() if path.exists() else None
    values = [json.loads(raw.decode("utf-8-sig")) if raw is not None else {} for raw in old.values()]
    values[0].setdefault("preferences", {}).update(
        save_folder=str(root), auto_save=True, file_format="mp4",
        preferred_mic_device=names[0] + " (input)", preferred_system_device=names[1] + " (output)")
    values[1]["mode"] = "audio_only"
    backup.mkdir()  # Exclusive; a previous run's backups are never overwritten.
    for name, raw in old.items():
        if raw is not None:
            (backup / name).write_bytes(raw)
    (backup / "ready").touch()
    try:
        for name, value in zip(PREFS, values):
            atomic_write(profile / name, json.dumps(value).encode())
        yield
    finally:
        if before_restore:
            before_restore()
        for name, raw in old.items():
            path = safe_path(profile / name)
            if raw is None:
                path.unlink(missing_ok=True)
            else:
                atomic_write(path, raw)
            require((path.read_bytes() if path.exists() else None) == raw, "Recording preference restore failed.")
        for name in (*PREFS, "ready"):
            (backup / name).unlink(missing_ok=True)
        backup.rmdir()


def database(db, writable=False):
    return sqlite3.connect(safe_path(db).as_uri() + ("?mode=rw" if writable else "?mode=ro"),
                           uri=True, timeout=0.25)


def library(db):
    with closing(database(db)) as conn:
        return dict(conn.execute("SELECT id, folder_path FROM meetings"))


def timeout_until(seconds, check, label):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = check()
        if value and time.monotonic() <= deadline:
            return value
        time.sleep(0.1)
    raise Refused(label)


def folder_started(root, existing, baseline, names):
    entries = set(root.iterdir()) - existing
    if not entries:
        return None
    require(len(entries) == 1, "Unexpected concurrent files in the synthetic recordings folder.")
    folder = safe_path(entries.pop())
    marker = safe_path(folder / "metadata.json")
    if not marker.is_file():
        return None
    meta = json_file(marker)
    require(meta["status"] == "recording" and meta["recording_mode"] == "audio_only",
            "The new folder does not identify an active audio-only recording.")
    dev = meta["devices"]
    if dev["microphone"] is None or dev["system_audio"] is None:
        return None  # The recorder writes device info after its initial marker.
    require([dev["microphone"], dev["system_audio"]] == names, "The active recording names different endpoints.")
    key = meta["recording_id"]
    require(re.fullmatch(r"meeting-[a-f0-9-]{36}", key) and str(uuid.UUID(key[8:])) == key[8:]
            and key not in baseline, "The new recording lacks a fresh stable ID.")
    return key, folder


def saved(db, key, folder):
    with closing(database(db)) as conn:
        row = conn.execute(
            "SELECT folder_path, recording_source, recording_duration_s FROM meetings WHERE id=?", (key,)).fetchone()
        flags = conn.execute("SELECT audio_save_failed, transcription_incomplete, capture_incomplete, "
                             "recording_files_incomplete, recovery_files_elsewhere FROM recording_outcomes "
                             "WHERE meeting_id=?", (key,)).fetchone()
    meta = json_file(safe_path(folder / "metadata.json"))
    if row is None or meta.get("meeting_id") is None:
        return None
    require(Path(row[0]).resolve() == folder and row[1] == "recording" and (row[2] or 0) > 0,
            "SQLite did not persist this recording's folder, source and duration.")
    require(flags == (0, 0, 0, 0, 0), "SQLite reports incomplete capture or missing recording outcomes.")
    require(meta["recording_id"] == meta["meeting_id"] == key and meta["status"] == "completed"
            and meta["recording_mode"] == "audio_only" and meta["audio_file"] == "audio.mp4",
            "Final recording metadata failed validation.")
    outcome = json_file(safe_path(folder / "recording-outcome.json"))
    require(all(outcome.get(field) is False for field in ("capture_incomplete", "audio_save_failed",
                "recording_files_incomplete", "transcription_incomplete", "recovery_files_elsewhere")),
            "The recorder reported an incomplete save.")
    return row[2]


def remove_owned(db, root, baseline, owned):
    # The app is closed. Check every UUID, path and file BEFORE the transaction.
    expected = dict(baseline)
    with closing(database(db, writable=True)) as conn:
        conn.execute("PRAGMA foreign_keys=ON")
        conn.execute("BEGIN IMMEDIATE")
        current = dict(conn.execute("SELECT id, folder_path FROM meetings"))
        for key, folder in owned:
            require(key not in expected and key in current and folder.parent == root
                    and Path(current[key]).resolve() == safe_path(folder),
                    "Cleanup refused a changed or pre-existing meeting.")
            meta = json_file(safe_path(folder / "metadata.json"))
            require(meta["meeting_id"] == meta["recording_id"] == key,
                    "Cleanup refused a folder whose identity changed.")
            require(all(safe_path(p).is_file() and p.name in MEETING_FILES for p in folder.iterdir()),
                    "Cleanup found unrecognized files. Inspect the synthetic folder locally.")
            expected[key] = current[key]
        require(current == expected, "Meeting library changed outside the capture run.")
        for key, _ in owned:
            conn.execute("DELETE FROM meetings WHERE id=? AND folder_path=?", (key, current[key]))
            # The normal DELETE trigger records the ID to block delayed handoffs.
            # The process has exited and this fixture's files are removed below.
            conn.execute("DELETE FROM recording_library_deletions WHERE meeting_id=?", (key,))
        conn.commit()
    for _, folder in owned:
        for name in MEETING_FILES:
            safe_path(folder / name).unlink(missing_ok=True)
        folder.rmdir()
    require(library(db) == baseline, "The baseline meeting library was not restored.")


def decode(ffmpeg, folder, duration, markers):
    audio = safe_path(folder / "audio.mp4")
    require(audio.is_file() and 0 < audio.stat().st_size < 64 * 1024**2, "Missing or oversized saved MP4.")
    data = run_cmd([ffmpeg, "-nostdin", "-v", "error", "-xerror", "-protocol_whitelist", "file,pipe",
                    "-f", "mov", "-i", audio, "-map", "0:a:0", "-t", "65", "-ac", "1", "-ar", "16000",
                    "-f", "f32le", "pipe:1"], 20, output=True)
    pcm = np.frombuffer(data, dtype="<f4")
    seconds = len(pcm) / 16000
    require(5 < seconds < 65 and abs(seconds - duration) < 3, "Decoded duration disagrees with the saved meeting.")
    verify_mix(pcm, 16000, markers)
    return round(seconds, 2)


def sha256(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def installer_payload(seven_zip, installer):
    hashes = {}
    for name in ("clawscribe.exe", "ffmpeg.exe"):
        with tempfile.TemporaryFile() as payload:
            subprocess.run([seven_zip, "e", "-so", "-r", "--", installer.resolve(), name],
                           stdout=payload, stderr=subprocess.DEVNULL, timeout=180, check=True)
            require(payload.tell() > 0, "Missing executable in the candidate installer.")
            payload.seek(0)
            hashes[name] = hashlib.file_digest(payload, "sha256").hexdigest()
    return hashes


def release_identity(args):
    require(re.fullmatch(r"[a-f0-9]{40}", args.commit) and re.fullmatch(r"\d+\.\d+\.\d+(-[\w.-]+)?", args.version)
            and re.fullmatch(r"[a-fA-F0-9]{64}", args.installer_sha256), "Invalid expected release identity.")
    require(args.installer.name == f"ClawScribe_{args.version}_x64-setup.exe" and
            sha256(args.installer) == args.installer_sha256.lower(), "Candidate NSIS filename or SHA-256 mismatch.")
    meta = dict(line.split("=", 1) for line in args.metadata.read_text(encoding="utf-8-sig").splitlines() if line)
    require(meta.get("product") == "ClawScribe" and meta.get("version") == args.version
            and meta.get("build_commit") == args.commit, "Release build metadata mismatch.")
    def at_commit(path):
        return run_cmd(["git", "-C", REPO, "show", f"{args.commit}:{path}"], 10, output=True).decode()
    require(json.loads(at_commit("frontend/package.json"))["version"] == args.version, "Release source version mismatch.")
    packages = tomllib.loads(at_commit("Cargo.lock"))["package"]
    cpal, = [p["source"] for p in packages if p["name"] == "cpal"]
    locked = re.fullmatch(r"git\+.+#([0-9a-f]{40})", cpal)
    require(locked, "The release commit has no unambiguous locked cpal revision.")
    return {"version": args.version, "build_commit": args.commit, "cpal_commit": locked[1],
            "installer_sha256": args.installer_sha256.lower()}


def installation():
    import winreg
    uninstall = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\ClawScribe"
    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, uninstall) as k:
        location = winreg.QueryValueEx(k, "InstallLocation")[0].strip('"')
        version = winreg.QueryValueEx(k, "DisplayVersion")[0]
    return safe_path(Path(location) / "clawscribe.exe"), version


def require_app_closed():
    listing = run_cmd(["tasklist.exe", "/FI", "IMAGENAME eq clawscribe.exe", "/FO", "CSV", "/NH"], 10, output=True)
    require(b'"clawscribe.exe"' not in listing.lower(),
            "Quit ClawScribe from its tray menu before starting the harness.")


def reserve_session():
    # Keep the handle until process exit. Windows releases it even after a crash.
    from ctypes import wintypes
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.CreateMutexW.argtypes = [ctypes.c_void_p, wintypes.BOOL, wintypes.LPCWSTR]
    kernel.CreateMutexW.restype = wintypes.HANDLE
    ctypes.set_last_error(0)
    handle = kernel.CreateMutexW(None, False, r"Local\ClawScribeInstalledCaptureAcceptance")
    require(handle and ctypes.get_last_error() != 183, "Another installed capture harness owns this session.")


def one_recording(number, app, exe, config, root, db, names, baseline, owned, markers, stop_limit):
    known = set(root.iterdir())
    require(known == {folder for _, folder in owned}, "Unexpected contents in the dedicated capture folder.")
    require(audio_step(config, "check") == names, "Virtual audio routes changed.")
    def cli(flag, timeout):
        require(app.poll() is None, "The harness-launched application exited.")
        run_cmd([exe, flag], timeout)
    started = time.monotonic()
    try:
        cli("--start-recording", 5)
        previous_ids = set(baseline) | {key for key, _ in owned}
        key, folder = timeout_until(25, lambda: folder_started(root, known, previous_ids, names),
                                    "CLI Start did not create the expected active recording.")
        audio_step(config, "play", number)
    finally:
        stopped = time.monotonic()
        cli("--stop-recording", min(5, stop_limit))
    remaining = stop_limit - (time.monotonic() - stopped)
    duration = timeout_until(remaining, lambda: saved(db, key, folder), "CLI Stop exceeded its time to durable save.")
    elapsed = time.monotonic() - stopped
    seconds = decode(exe.with_name("ffmpeg.exe"), folder, duration, markers)
    # A foreign recording, partial save, or missing marker never grants deletion.
    owned.append((key, folder))
    return {"round": number, "stop_to_save_s": round(elapsed, 3), "decoded_s": seconds,
            "mic_hz": markers[0], "system_hz": markers[1], "round_s": round(time.monotonic() - started, 2)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("config", "installer", "metadata"):
        parser.add_argument("--" + name, required=True, type=Path)
    for name in ("version", "commit", "installer-sha256"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--install", action="store_true", help="Run the verified current-user NSIS installer before testing.")
    parser.add_argument("--upgrade-from", help="Require an older registered version and a retained baseline meeting.")
    parser.add_argument("--seven-zip", default="7z.exe")
    parser.add_argument("--stop-seconds", type=int, choices=range(5, 61), default=20, metavar="5..60")
    args = parser.parse_args()
    interactive_windows()
    reserve_session()
    require_app_closed()
    print("Checking release identity and the provisioned profile.", file=sys.stderr)
    report = release_identity(args)
    require(tuple(map(int, args.version.split("-")[0].split("."))) >= (0, 64, 1),
            "Use a candidate with exact Windows audio endpoint selection (0.64.1 or later).")
    payload = installer_payload(args.seven_zip, args.installer)
    config = json_file(args.config)
    root = safe_path(Path(config["recordings_root"]))
    require(root.is_dir() and not any(root.iterdir()), "Provision an empty, dedicated recordings folder.")
    profile = safe_path(Path(os.environ["APPDATA"]) / APP_ID)
    trusted = json_file(safe_path(profile / "trusted_folders.json")).get("trusted", [])
    require(root in map(Path, trusted), "Choose the dedicated folder once using ClawScribe's folder picker.")
    require(not (profile / "installed-capture-preferences").exists(), "Recover the previous preference backup first.")
    call_detection = profile.parent / "ClawScribe" / "call_detection.json"
    require(not call_detection.exists() or json_file(call_detection)["mode"] == "off", "Disable automatic call detection.")
    db = profile / "meeting_minutes.sqlite"
    baseline = library(db)
    runtime_version = args.version.split("-")[0]
    if args.upgrade_from:
        require(args.install and baseline and re.fullmatch(r"\d+\.\d+\.\d+", args.upgrade_from)
                and installation()[1] == args.upgrade_from
                and tuple(map(int, args.upgrade_from.split("."))) < tuple(map(int, runtime_version.split("."))),
                "Upgrade requires the named older installation and at least one pre-existing synthetic meeting.")
        report["upgrade_from"] = args.upgrade_from
    report["retained_meetings"] = len(baseline)
    names = audio_step(args.config, "check")
    print("Calibrating each virtual route independently for both recordings.", file=sys.stderr)
    markers = [audio_step(args.config, "probe", n) for n in (1, 2)]
    require(min(abs(a-b) for a in markers[0] for b in markers[1]) > 40, "The two rounds have ambiguous markers.")
    if args.install:
        print("Installing the verified current-user candidate.", file=sys.stderr)
        require_app_closed()
        require(sha256(args.installer) == report["installer_sha256"], "Installer changed during provisioning checks.")
        run_cmd([args.installer.resolve(), "/S"], 300)
    require_app_closed()
    print("Comparing the registered app and decoder with the installer payload.", file=sys.stderr)
    exe, version = installation()
    require(version in (runtime_version, args.version), "Registered product version mismatch.")
    for name, digest in payload.items():
        require(digest == sha256(exe.with_name(name)), "Registered installation differs from the candidate payload.")
    report["installed_exe_sha256"] = payload["clawscribe.exe"]
    owned, rounds = [], []
    require(audio_step(args.config, "check") == names, "Virtual routes changed after release verification.")
    with capture_preferences(profile, root, names, before_restore=require_app_closed):
        app = subprocess.Popen([exe], cwd=exe.parent, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            try:
                run_cmd(["powershell.exe", "-NoProfile", "-NonInteractive", "-File", HERE / "wait-ready.ps1",
                         str(app.pid)], 50)
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
                raise Refused("The installed Start recording control did not become ready.")
            require(library(db) == baseline, "Upgrade or startup changed pre-existing meeting IDs or folder bindings.")
            for number, pair in enumerate(markers, 1):
                print(f"Starting installed CLI recording {number}.", file=sys.stderr)
                rounds.append(one_recording(number, app, exe, args.config, root, db, names,
                                            baseline, owned, pair, args.stop_seconds))
        finally:
            print("Quit the harness-launched app from its tray menu. Waiting up to 30 seconds.", file=sys.stderr, flush=True)
            try:
                app.wait(timeout=30)
                require(app.returncode == 0, "The harness-launched application exited abnormally.")
                report["app_exit"] = "normal"
            except subprocess.TimeoutExpired:
                # Only our Popen handle, in the dedicated test account.
                app.kill()
                app.wait(timeout=10)
                report["app_exit"] = "ended by harness after tray quit timeout"
            require_app_closed()
            # Reopen SQLite after the owning process exits. Preserve ambiguous
            # or incomplete folders on failure for operator inspection.
            original_error = sys.exc_info()[1]
            try:
                for key, folder in owned:
                    require(saved(db, key, folder), "Saved meeting did not survive application exit.")
                remove_owned(db, root, baseline, owned)
            except Exception:
                if original_error is None:
                    raise
                print("Cleanup incomplete. Inspect the dedicated folder and database locally.", file=sys.stderr)
    require(not any(root.iterdir()), "An incomplete capture remains in the dedicated folder.")
    print(json.dumps(dict(report, result="passed", rounds=rounds, cleanup="verified"), indent=2))


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        sys.exit(str(error) if isinstance(error, Refused) else
                 f"Installed capture failed ({type(error).__name__}). Check the last step and local fixture state.")
