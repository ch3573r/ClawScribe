"""Virtual-route probe and marker analysis for the installed Windows capture smoke."""

import argparse
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager, ExitStack
import ctypes
import json
import os
from pathlib import Path
import sys
import subprocess
from types import SimpleNamespace

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


def windows_endpoint_names():
    # Join the current Windows endpoint IDs to the WASAPI catalog before streaming.
    command = ("[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); "
               "ConvertTo-Json -Compress -InputObject @(Get-PnpDevice -PresentOnly -Class AudioEndpoint "
               "-Status OK -ErrorAction Stop | Select-Object InstanceId,FriendlyName)")
    result = subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", command],
                            capture_output=True, timeout=15, check=True,
                            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    return {row["InstanceId"].rsplit(chr(92), 1)[-1].casefold(): row["FriendlyName"]
            for row in json.loads(result.stdout.decode("utf-8-sig"))}


def resolve_routes(config, microphones, speakers):
    require(config.get("isolated_virtual_routes") is True,
            "Provision and confirm the isolated virtual routes in the private configuration first.")
    inputs = {d.id.casefold(): d for d in microphones}
    outputs = {d.id.casefold(): d for d in speakers}
    mic, feed, system = (config[k].casefold() for k in ("microphone", "microphone_feed", "system_output"))
    require(len({mic, feed, system}) == 3 and mic in inputs and feed in outputs and system in outputs,
            "One of the provisioned virtual endpoints is missing or shares an ID.")
    for selected, devices in ((inputs[mic], microphones), (outputs[system], speakers), (outputs[feed], speakers)):
        require(sum(d.name == selected.name for d in devices) == 1,
                "A selected endpoint name is ambiguous in Windows Sound settings.")
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


class Endpoint:
    """A named WASAPI endpoint. PortAudio opens the exact enumerated index."""
    def __init__(self, host, info, endpoint_id):
        self.host = host
        self.info = info
        self.id = endpoint_id
        self.name = info["name"]

    @contextmanager
    def player(self, rate, blocksize):
        import pyaudiowpatch as pa
        channels = min(2, self.info["maxOutputChannels"])
        native = int(self.info["defaultSampleRate"])
        stream = self.host.open(format=pa.paInt16, channels=channels, rate=native,
                                output=True, output_device_index=self.info["index"],
                                frames_per_buffer=blocksize)
        def play(wave):
            if native != rate:
                wave = np.interp(np.arange(round(len(wave)*native/rate))*rate/native,
                                 np.arange(len(wave)), wave)
            pcm = np.rint(np.clip(wave, -1, 1) * 32767).astype("<i2")
            stream.write(np.repeat(pcm, channels).tobytes())
        try:
            yield SimpleNamespace(play=play)
        finally:
            stream.close()

    @contextmanager
    def recorder(self, rate, blocksize):
        import pyaudiowpatch as pa
        channels = min(2, self.info["maxInputChannels"])
        native = int(self.info["defaultSampleRate"])
        stream = self.host.open(format=pa.paFloat32, channels=channels, rate=native,
                                input=True, input_device_index=self.info["index"],
                                frames_per_buffer=blocksize)
        def record(length):
            raw = stream.read(round(length*native/rate), exception_on_overflow=False)
            mono = np.frombuffer(raw, dtype="float32").reshape(-1, channels).mean(axis=1)
            if native != rate:
                mono = np.interp(np.arange(length)*native/rate, np.arange(len(mono)), mono)
            return mono[:, None]
        try:
            yield SimpleNamespace(record=record)
        finally:
            stream.close()


class MicrophoneProbe:
    """Read the named virtual input through an independent DirectShow client."""
    def __init__(self, ffmpeg, name):
        self.ffmpeg = ffmpeg
        self.name = name

    @contextmanager
    def recorder(self, rate, blocksize):
        def record(length):
            seconds = length / rate
            args = [self.ffmpeg, "-nostdin", "-hide_banner", "-loglevel", "error",
                    "-f", "dshow", "-i", "audio=" + self.name, "-t", str(seconds),
                    "-ac", "1", "-ar", str(rate), "-f", "f32le", "pipe:1"]
            raw = subprocess.run(args, capture_output=True, timeout=seconds+8, check=True).stdout
            return np.frombuffer(raw, dtype="<f4")[:, None]
        yield SimpleNamespace(record=record)


def emit(mic, feed, system, frequencies, probe, loopback):
    result = []
    for leg, hz in enumerate(frequencies):
        wave = np.pad(tone(hz), (RATE * 3 // 2, RATE // 2))
        with ExitStack() as stack:
            players = [stack.enter_context(d.player(RATE, blocksize=4800)) for d in (feed, system)]
            readers = [stack.enter_context(d.recorder(RATE, blocksize=4800)) for d in (mic, loopback)] if probe else []
            with ThreadPoolExecutor(max_workers=len(players)+len(readers)) as pool:
                reads = [pool.submit(reader.record, len(wave)) for reader in readers]
                plays = [pool.submit(player.play, wave if i == leg else np.zeros_like(wave))
                         for i, player in enumerate(players)]
                for play in plays:
                    play.result()
                if probe:
                    result.append(measured_marker(reads[leg].result().mean(axis=1),
                                                  reads[1-leg].result().mean(axis=1), RATE))
    if probe:
        require(abs(result[1] - frequencies[1]) <= 16 and abs(result[0] - result[1]) > 64,
                "The measured system loopback or microphone calibration is ambiguous.")
    return result


def wasapi_routes(host, pa):
    names = windows_endpoint_names()
    api = host.get_host_api_info_by_type(pa.paWASAPI)["index"]
    devices = [d for d in host.get_device_info_generator() if d["hostApi"] == api]
    inputs, outputs = [], []
    for key, label in names.items():
        capture = key.startswith("{0.0.1.")
        render = key.startswith("{0.0.0.")
        if not capture and not render:
            continue
        matches = [d for d in devices if not d["isLoopbackDevice"] and d["name"] == label
                   and d["maxInputChannels" if capture else "maxOutputChannels"] > 0]
        if len(matches) == 1:
            (inputs if capture else outputs).append(Endpoint(host, matches[0], key))
    return inputs, outputs, devices


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["devices", "check", "probe", "play"])
    parser.add_argument("--config", type=Path)
    parser.add_argument("--ffmpeg", default="ffmpeg.exe")
    parser.add_argument("--round", type=int, choices=[1, 2], default=1)
    args = parser.parse_args()
    interactive_windows()
    import pyaudiowpatch as pa
    with pa.PyAudio() as host:
        inputs, outputs, devices = wasapi_routes(host, pa)
        if args.action == "devices":
            print(json.dumps({key: [{"name": d.name, "id": d.id} for d in items]
                              for key, items in (("capture", inputs), ("render", outputs))}, indent=2))
            return
        config = json.loads(args.config.read_text(encoding="utf-8-sig"))
        mic, feed, system = resolve_routes(config, inputs, outputs)
        if args.action == "check":
            print(json.dumps([mic.name, system.name]))
            return
        loops = [d for d in devices if d["isLoopbackDevice"] and d["name"] == system.name + " [Loopback]"]
        require(len(loops) == 1, "No unique loopback input exists for the selected output.")
        loopback = Endpoint(host, loops[0], system.id)
        meter = MicrophoneProbe(args.ffmpeg, mic.name)
        print(json.dumps(emit(meter, feed, system, TONES[args.round-1], args.action == "probe", loopback)))


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        sys.exit(str(error) if isinstance(error, Refused) else
                 f"Virtual audio operation failed ({type(error).__name__}).")
