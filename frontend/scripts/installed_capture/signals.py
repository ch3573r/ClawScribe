"""Virtual-route probe and marker analysis for the installed Windows capture smoke."""

import argparse
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager, nullcontext
import ctypes
import json
import os
from pathlib import Path
import sys
from threading import Barrier

import numpy as np


RATE = 48000
# Each recording has its own pair. The microphone driver may shift its pitch.
TONES = ((617, 1153), (823, 1471))


class Refused(RuntimeError):
    """A fixed, public-safe test failure."""


def require(condition, message):
    if not condition:
        raise Refused(message)


def interactive_windows():
    require(sys.platform == "win32", "The live harness requires Windows.")
    session = ctypes.c_ulong()
    ok = ctypes.windll.kernel32.ProcessIdToSessionId(os.getpid(), ctypes.byref(session))
    require(ok and session.value and not os.environ.get("CI") and not os.environ.get("GITHUB_ACTIONS"),
            "Use the interactive acceptance desktop outside CI and service sessions.")


def resolve_routes(config, microphones, speakers):
    require(config.get("isolated_virtual_routes") is True,
            "Provision and confirm the isolated virtual routes in the private configuration first.")
    inputs = {d.id: d for d in microphones}
    outputs = {d.id: d for d in speakers}
    mic, feed, system = (config[k] for k in ("microphone", "microphone_feed", "system_output"))
    require(len({mic, feed, system}) == 3 and set(inputs) == {mic} and set(outputs) == {feed, system},
            "Active WASAPI endpoints differ from the three provisioned virtual endpoints.")
    require(outputs[system].name not in outputs[feed].name and outputs[feed].name not in outputs[system].name,
            "The two render names are ambiguous to the application's device matcher.")
    return inputs[mic], outputs[feed], outputs[system]


def tone(frequency, rate=RATE):
    t = np.arange(3 * rate) / rate
    # Pulses survive a short endpoint wake-up and make a steady hum a poor match.
    phase = t % 0.75
    envelope = np.minimum(np.clip(phase / 0.015, 0, 1), np.clip((0.55 - phase) / 0.015, 0, 1))
    return (0.24 * envelope * np.sin(2 * np.pi * frequency * t)).astype("float32")


def peak_track(samples, rate):
    """Dominant frequency per 125 ms, requiring audible, narrow-band energy."""
    n = rate // 8
    blocks = np.asarray(samples[:len(samples) // n * n], dtype="float32").reshape(-1, n)
    if not len(blocks):
        return np.array([])
    spectrum = abs(np.fft.rfft(blocks * np.hanning(n), axis=1)) ** 2
    peaks = spectrum.argmax(axis=1)
    strongest = spectrum[np.arange(len(blocks)), peaks]
    tonal = (strongest > 0.25 * spectrum.sum(axis=1)) & (np.sqrt(strongest) > n * 0.0004)
    return np.where(tonal, peaks * rate / n, 0)


def measured_marker(active, quiet, rate):
    peaks = peak_track(active, rate)
    valid = peaks[(peaks >= 120) & (peaks <= 3500)]
    require(len(valid) > 0, "No tonal marker reached the selected virtual endpoint.")
    bins, counts = np.unique(valid, return_counts=True)
    hz = float(bins[counts.argmax()])
    require(np.count_nonzero(abs(peaks - hz) <= 12) >= 8,
            "The upstream marker was too short or unstable.")
    require(np.count_nonzero(peak_track(quiet, rate)) < 3,
            "Unexpected audio reached the other virtual endpoint during calibration.")
    require(np.sqrt(np.mean(np.square(quiet))) < 0.003,
            "Unexpected noise reached the other virtual endpoint during calibration.")
    return hz


def verify_mix(samples, rate, expected):
    track = peak_track(samples, rate)
    positions = [np.flatnonzero(abs(track - hz) <= 16) for hz in expected]
    require(all(len(p) >= 6 for p in positions), "A sustained source marker is missing from saved audio.")
    require(positions[0][-1] < positions[1][0], "Saved source markers are reordered or overlap.")


@contextmanager
def com_thread():
    # Each worker opens, uses and closes its own WASAPI client on the same MTA thread.
    ole = ctypes.windll.ole32
    require(ole.CoInitializeEx(None, 0) in (0, 1), "Could not initialize a virtual audio worker.")
    try:
        yield
    finally:
        ole.CoUninitialize()


def emit(mic, feed, system, frequencies, probe, loopback, worker_context=nullcontext):
    result = []
    for leg, hz in enumerate(frequencies):
        wave = np.pad(tone(hz), (RATE // 2, RATE // 2))
        gate = Barrier(4 if probe else 2, timeout=5)

        def render(device, data):
            with worker_context(), device.player(RATE, blocksize=4800) as player:
                gate.wait()
                player.play(data)

        def capture(device):
            # Use all channels. Single-channel WASAPI capture can return corrupt samples.
            with worker_context(), device.recorder(RATE, blocksize=4800) as reader:
                gate.wait()
                return reader.record(len(wave)).mean(axis=1)

        # Service each hardware clock independently. Sequential blocking writes
        # to two outputs can underrun the quieter route and corrupt calibration.
        with ThreadPoolExecutor(max_workers=gate.parties) as pool:
            plays = [pool.submit(render, d, wave if i == leg else np.zeros_like(wave))
                     for i, d in enumerate((feed, system))]
            reads = [pool.submit(capture, d) for d in (mic, loopback)] if probe else []
            for play in plays:
                play.result()
            if probe:
                result.append(measured_marker(reads[leg].result(), reads[1-leg].result(), RATE))
    if probe:
        require(abs(result[1] - frequencies[1]) <= 16 and abs(result[0] - result[1]) > 64,
                "The measured system loopback or microphone calibration is ambiguous.")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["devices", "check", "probe", "play"])
    parser.add_argument("--config", type=Path)
    parser.add_argument("--round", type=int, choices=[1, 2], default=1)
    args = parser.parse_args()
    interactive_windows()
    import soundcard as sc
    inputs, outputs = sc.all_microphones(), sc.all_speakers()
    if args.action == "devices":
        # Local provisioning command only. The acceptance report omits these.
        print(json.dumps({key: [{"name": d.name, "id": d.id} for d in devices]
                          for key, devices in (("capture", inputs), ("render", outputs))}, indent=2))
        return
    config = json.loads(args.config.read_text(encoding="utf-8-sig"))
    mic, feed, system = resolve_routes(config, inputs, outputs)
    if args.action == "check":
        print(json.dumps([mic.name, system.name]))
        return
    loopback = next(d for d in sc.all_microphones(include_loopback=True)
                    if d.isloopback and d.id == system.id)
    print(json.dumps(emit(mic, feed, system, TONES[args.round-1], args.action == "probe", loopback, com_thread)))


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        sys.exit(str(error) if isinstance(error, Refused) else
                 f"Virtual audio operation failed ({type(error).__name__}).")
