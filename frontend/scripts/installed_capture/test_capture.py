"""Offline acceptance-harness tests. No application, installer or audio endpoint is opened."""

from contextlib import closing, nullcontext
import json
from pathlib import Path
import shutil
import sqlite3
import subprocess
import tempfile
from threading import Event
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

import numpy as np

import run
import signals


class Fixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name).resolve()
        self.root = self.base / "recordings"
        self.root.mkdir()
        self.db = self.base / "fixture.sqlite"
        with closing(sqlite3.connect(self.db)) as conn:
            for sql in sorted((run.REPO / "frontend/src-tauri/migrations").glob("*.sql")):
                conn.executescript(sql.read_text(encoding="utf-8"))
            conn.execute("INSERT INTO meetings(id,title,created_at,updated_at) VALUES('before','Synthetic',0,0)")
            conn.commit()
        self.baseline = run.library(self.db)

    def start_fixture(self, number):
        folder = self.root / f"fixture-{number}"
        folder.mkdir()
        key = f"meeting-00000000-0000-4000-8000-{number:012d}"
        meta = {"recording_id": key, "meeting_id": None, "status": "recording", "recording_mode": "audio_only",
                "devices": {"microphone": "virtual in", "system_audio": "virtual out"}, "audio_file": "audio.mp4"}
        marker = folder / "metadata.json"
        marker.write_text(json.dumps(meta))
        self.assertEqual((key, folder), run.folder_started(self.root, set(self.root.iterdir()) - {folder},
                                                           self.baseline, ["virtual in", "virtual out"]))
        self.assertIsNone(run.saved(self.db, key, folder))
        return key, folder

    def stop_fixture(self, key, folder):
        marker = folder / "metadata.json"
        meta = run.json_file(marker)
        meta.update(status="completed", meeting_id=key)
        marker.write_text(json.dumps(meta))
        (folder / "recording-outcome.json").write_text(json.dumps(dict.fromkeys([
            "capture_incomplete", "transcription_incomplete", "recording_files_incomplete",
            "audio_save_failed", "recovery_files_elsewhere"], False)))
        with closing(sqlite3.connect(self.db)) as conn:
            conn.execute("INSERT INTO meetings(id,title,created_at,updated_at,folder_path,recording_source,"
                         "recording_duration_s) VALUES(?, 'Synthetic',0,0,?,'recording',8)", (key, str(folder)))
            conn.execute("INSERT INTO recording_outcomes(meeting_id) VALUES(?)", (key,))
            conn.commit()

    def save_fixture(self, number):
        key, folder = self.start_fixture(number)
        self.stop_fixture(key, folder)
        return key, folder

    def test_two_durable_rows_cleanup_and_prior_library(self):
        owned = [self.save_fixture(n) for n in (1, 2)]
        for key, folder in owned:
            self.assertEqual(8, run.saved(self.db, key, folder))
        run.remove_owned(self.db, self.root, self.baseline, owned)
        self.assertEqual(self.baseline, run.library(self.db))
        self.assertEqual([], list(self.root.iterdir()))
        with closing(sqlite3.connect(self.db)) as conn:
            self.assertEqual([], conn.execute("PRAGMA foreign_key_check").fetchall())
            self.assertEqual([], conn.execute("SELECT * FROM recording_library_deletions").fetchall())
            self.assertEqual([("before",)], conn.execute("SELECT meeting_id FROM knowledge_sources").fetchall())

    def test_cleanup_refuses_unknown_file_row_or_identity_before_deleting_anything(self):
        owned = [self.save_fixture(n) for n in (1, 2)]
        key, folder = owned[1]
        foreign = folder / "keep.txt"
        foreign.write_text("synthetic sentinel")
        with self.assertRaises(run.Refused):
            run.remove_owned(self.db, self.root, self.baseline, owned)
        self.assertEqual("synthetic sentinel", foreign.read_text())
        self.assertEqual(3, len(run.library(self.db)))
        foreign.unlink()
        with self.assertRaises(run.Refused):
            run.remove_owned(self.db, self.root, {**self.baseline, key: str(folder)}, owned)
        marker = folder / "metadata.json"
        marker.write_text(json.dumps(dict(run.json_file(marker), meeting_id="before")))
        with self.assertRaises(run.Refused):
            run.remove_owned(self.db, self.root, self.baseline, owned)
        self.assertEqual(3, len(run.library(self.db)))

    def test_outcome_gap_and_wrong_sqlite_folder_refused(self):
        key, folder = self.save_fixture(1)
        outcome = folder / "recording-outcome.json"
        original = outcome.read_bytes()
        outcome.write_text(json.dumps(dict(run.json_file(outcome), capture_incomplete=True)))
        with self.assertRaises(run.Refused):
            run.saved(self.db, key, folder)
        outcome.write_bytes(original)
        with closing(sqlite3.connect(self.db)) as conn:
            conn.execute("UPDATE meetings SET folder_path=? WHERE id=?", (str(self.base), key))
            conn.commit()
        with self.assertRaises(run.Refused):
            run.saved(self.db, key, folder)

    def test_failure_restores_original_preference_bytes_and_absence(self):
        old = self.base / run.PREFS[0]
        raw = b'{"preferences":{"auto_save":false}, "extra":true}\r\n'
        old.write_bytes(raw)
        with self.assertRaisesRegex(ValueError, "fixture"):
            with run.capture_preferences(self.base, self.root, ["virtual in", "virtual out"]):
                staged = run.json_file(old)
                self.assertTrue(staged["extra"])
                self.assertEqual("virtual in (input)", staged["preferences"]["preferred_mic_device"])
                self.assertEqual({"mode": "audio_only"}, run.json_file(self.base / run.PREFS[1]))
                raise ValueError("fixture")
        self.assertEqual(raw, old.read_bytes())
        self.assertFalse((self.base / run.PREFS[1]).exists())
        self.assertFalse((self.base / "installed-capture-preferences").exists())

    def test_expired_stop_never_counts_as_a_save(self):
        with patch.object(run.time, "monotonic", side_effect=[0., 0.1, 2., 2.]), patch.object(run.time, "sleep"):
            with self.assertRaisesRegex(run.Refused, "late"):
                run.timeout_until(1, lambda: True, "late")

    def test_cli_stop_sent_once_if_marker_playback_fails(self):
        names = ["virtual in", "virtual out"]
        app = Mock()
        app.poll.return_value = None
        with patch.object(run, "audio_step", side_effect=[names, run.Refused("injection failed")]), \
             patch.object(run, "folder_started", return_value=("synthetic", self.root / "fixture")), \
             patch.object(run, "run_cmd") as cli:
            with self.assertRaisesRegex(run.Refused, "injection failed"):
                run.one_recording(1, app, Path("app"), None, self.root, self.db, names, self.baseline, [], [], 20)
        self.assertEqual(["--start-recording", "--stop-recording"], [call.args[0][1] for call in cli.call_args_list])

    def test_two_cli_rounds_end_to_end_with_fixture_processes(self):
        # Only substitute external boundaries. Exercise the production folder
        # discovery, deadlines, SQLite checks, complete decode and cleanup.
        ffmpeg = shutil.which("ffmpeg")
        self.assertIsNotNone(ffmpeg)
        app = Mock()
        app.poll.return_value = None
        names = ["virtual in", "virtual out"]
        owned, commands = [], []
        real_cmd = run.run_cmd
        for number, pair in enumerate(signals.TONES, 1):
            fixture = None
            def simulated_cli(command, timeout, **kwargs):
                nonlocal fixture
                if command[1] == "--start-recording":
                    commands.append(command[1])
                    fixture = self.start_fixture(number)
                elif command[1] == "--stop-recording":
                    commands.append(command[1])
                    self.stop_fixture(*fixture)
                else:
                    return real_cmd([ffmpeg, *command[1:]], timeout, **kwargs)

            def simulated_audio(config, action, round_number=1):
                if action == "check":
                    return names
                self.assertEqual(("play", number), (action, round_number))
                data = np.concatenate([signals.tone(pair[0], 16000), np.zeros(16000),
                                       signals.tone(pair[1], 16000), np.zeros(16000)]).astype("<f4")
                subprocess.run([ffmpeg, "-nostdin", "-v", "error", "-f", "f32le", "-ar", "16000", "-ac", "1",
                                "-i", "pipe:0", "-c:a", "aac", self.root / f"fixture-{number}" / "audio.mp4"],
                               input=data.tobytes(), check=True)

            with patch.object(run, "run_cmd", side_effect=simulated_cli), patch.object(run, "audio_step", side_effect=simulated_audio):
                row = run.one_recording(number, app, self.base / "fixture.exe", None, self.root, self.db,
                                        names, self.baseline, owned, pair, 5)
                self.assertEqual(number, row["round"])
                self.assertLess(row["stop_to_save_s"], 5)
        self.assertEqual(["--start-recording", "--stop-recording"] * 2, commands)
        self.assertEqual(2, len(set(key for key, _ in owned)))
        run.remove_owned(self.db, self.root, self.baseline, owned)

    def test_release_commit_and_installer_hash(self):
        version = json.loads((run.REPO / "frontend/package.json").read_text())["version"]
        commit = subprocess.check_output(["git", "-C", run.REPO, "rev-parse", "HEAD"], text=True).strip()
        installer = self.base / f"ClawScribe_{version}_x64-setup.exe"
        installer.write_bytes(b"non-executable fixture")
        metadata = self.base / "build.txt"
        metadata.write_text(f"product=ClawScribe\nversion={version}\nbuild_commit={commit}\n")
        args = SimpleNamespace(installer=installer, version=version, commit=commit, metadata=metadata,
                               installer_sha256=run.sha256(installer))
        self.assertRegex(run.release_identity(args)["cpal_commit"], "^[a-f0-9]{40}$")
        args.commit = "0" * 40
        with self.assertRaisesRegex(run.Refused, "metadata"):
            run.release_identity(args)
        installer.write_bytes(b"changed")
        with self.assertRaisesRegex(run.Refused, "SHA-256"):
            run.release_identity(args)

    def test_decode_real_aac_fixture_with_measured_microphone_pitch(self):
        ffmpeg = shutil.which("ffmpeg")
        self.assertIsNotNone(ffmpeg, "Put FFmpeg on PATH for the offline codec fixture.")
        mic, sys_audio = 312, 1153
        quiet = np.zeros(16000)
        pcm = np.concatenate([signals.tone(mic, 16000), quiet, signals.tone(sys_audio, 16000), quiet]).astype("<f4")
        subprocess.run([ffmpeg, "-nostdin", "-v", "error", "-f", "f32le", "-ar", "16000", "-ac", "1",
                        "-i", "pipe:0", "-c:a", "aac", self.base / "audio.mp4"], input=pcm.tobytes(), check=True)
        measured = signals.measured_marker(pcm[:48000], quiet, 16000)
        self.assertAlmostEqual(8, run.decode(ffmpeg, self.base, 8, [measured, sys_audio]), delta=0.1)
        (self.base / "audio.mp4").write_bytes(b"corrupt")
        with self.assertRaises(subprocess.CalledProcessError):
            run.decode(ffmpeg, self.base, 8, [measured, sys_audio])


class SignalTests(unittest.TestCase):
    def test_clocked_virtual_fixture_calibrates_distinct_paths(self):
        class Wire:
            def __init__(self):
                self.written = Event()

            def player(self, rate, blocksize):
                self.written.clear()
                return nullcontext(self)

            def play(self, block):
                self.block = block
                self.written.set()

            def recorder(self, rate, blocksize):
                return nullcontext(self)

            def record(self, length):
                if not self.written.wait(2):
                    raise AssertionError("fixture render did not run")
                return np.column_stack([self.block, self.block])

        feed, system = Wire(), Wire()
        pair = signals.TONES[0]
        calibrated = signals.emit(feed, feed, system, pair, True, system)
        np.testing.assert_allclose(pair, calibrated, atol=4)

    def test_missing_stale_reversed_and_silent_markers(self):
        mic, system = (signals.tone(hz) for hz in signals.TONES[0])
        expected = signals.TONES[0]
        signals.verify_mix(np.concatenate([mic, system]), signals.RATE, expected)
        for data in [mic, system, np.zeros(signals.RATE), np.concatenate([system, mic]),
                     np.concatenate([signals.tone(hz) for hz in signals.TONES[1]])]:
            with self.subTest(length=len(data)), self.assertRaises(signals.Refused):
                signals.verify_mix(data, signals.RATE, expected)
        with self.assertRaisesRegex(signals.Refused, "other virtual endpoint"):
            signals.measured_marker(mic, mic, signals.RATE)

    def test_refuse_unconfigured_endpoint_and_shared_render_route(self):
        mic, feed, system, extra = [SimpleNamespace(id=n, name=n) for n in ("mic", "feed", "system", "extra")]
        config = dict(isolated_virtual_routes=True, microphone="mic", microphone_feed="feed", system_output="system")
        self.assertEqual((mic, feed, system), signals.resolve_routes(config, [mic], [feed, system]))
        self.assertEqual((mic, feed, system), signals.resolve_routes(config, [extra, mic], [extra, feed, system]))
        duplicate = SimpleNamespace(id="second", name="mic")
        for inputs, outputs in [([duplicate, mic], [feed, system]), ([mic], [feed]), ([extra], [feed, system])]:
            with self.assertRaises(signals.Refused):
                signals.resolve_routes(config, inputs, outputs)
        with self.assertRaises(signals.Refused):
            signals.resolve_routes(dict(config, system_output="feed"), [mic], [feed, system])


if __name__ == "__main__":
    unittest.main()
