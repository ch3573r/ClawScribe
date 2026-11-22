"""Generate the fictional short replies for the opt-in capture benchmark."""

import argparse
import struct
import subprocess
import tempfile
import wave
from pathlib import Path

REPLIES = (("de", "ja"), ("en-us", "yes"), ("de", "genau"), ("en-us", "right"))


def pcm_reply(path):
    with wave.open(str(path), "rb") as wav:
        if (wav.getnchannels(), wav.getsampwidth()) != (1, 2):
            raise ValueError("Expected mono PCM16 from eSpeak NG")
        rate = wav.getframerate()
        samples = [item[0] for item in struct.iter_unpack("<h", wav.readframes(wav.getnframes()))]

    # Remove synthesizer silence while retaining the beginnings and ends of words.
    first = next(index for index, sample in enumerate(samples) if abs(sample) > 100)
    last = next(index for index, sample in enumerate(reversed(samples)) if abs(sample) > 100)
    speech = samples[max(0, first - rate // 50) : len(samples) - last + rate // 100]
    return b"".join(
        struct.pack("<h", speech[index * rate // 16_000])
        for index in range(len(speech) * 16_000 // rate)
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--espeak", default="espeak-ng", help="eSpeak NG executable")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory() as temp:
        wav = Path(temp) / "reply.wav"
        for voice, reply in REPLIES:
            subprocess.run(
                [args.espeak, "-v", voice, "-s", "165", "-w", str(wav), reply], check=True
            )
            with (args.output / f"{reply}.s16le").open("xb") as output:
                output.write(pcm_reply(wav))


if __name__ == "__main__":
    main()
