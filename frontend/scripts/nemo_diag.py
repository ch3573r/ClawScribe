#!/usr/bin/env python3
"""
Nemotron DirectML vs CPU per-node diff for the encoder.

Pins ONE fp16 encoder + ONE input, exposes every internal tensor as a graph
output, runs the SAME input on two execution providers, and reports the FIRST
node (topological order) whose output diverges beyond tolerance. Also dumps
provider placement so we know DML is actually executing the encoder (vs silent
CPU fallback at partition boundaries).

Windows usage (on the 9070 XT box):
    pip install onnxruntime-directml onnx numpy soundfile librosa
    python nemo_diag.py <model_dir> [wav]   # model_dir holds encoder.onnx(+.data)

It compares CPUExecutionProvider vs DmlExecutionProvider by default. On a box
without DML it falls back to CPU-vs-CPU (machinery sanity check; all diffs ~0).
"""
import sys, os, json, numpy as np

MELS, NFFT, HOP, WIN, FMAX, PRE, SR = 128, 512, 160, 400, 8000.0, 0.97, 16000
LOGF, CHUNK, NUMPROMPTS = 1.0 / (1 << 24), 32, 128

def make_input(model_dir, wav):
    import onnxruntime as ort
    # mel for window 0 (librosa, matches the verified pipeline) or synthetic.
    if wav and os.path.exists(wav):
        import librosa
        y, _ = librosa.load(wav, sr=SR, mono=True)
        emph = np.copy(y); emph[1:] = y[1:] - PRE * y[:-1]
        S = librosa.feature.melspectrogram(y=emph, sr=SR, n_fft=NFFT, hop_length=HOP,
            win_length=WIN, window='hann', center=True, n_mels=MELS, fmin=0, fmax=FMAX,
            power=2.0, htk=False, norm='slaney')
        mel = np.log(S + LOGF).astype(np.float32)[:, :CHUNK]
        if mel.shape[1] < CHUNK:
            mel = np.pad(mel, ((0, 0), (0, CHUNK - mel.shape[1])))
        a = mel[None, :, :]
    else:
        a = (np.sin(np.arange(MELS)[:, None] * 0.13 + np.arange(CHUNK)[None, :] * 0.31) * 3.0)[None].astype(np.float32)
    # match encoder input dtypes
    enc0 = ort.InferenceSession(os.path.join(model_dir, "encoder.onnx"),
                                providers=['CPUExecutionProvider'])
    et = {i.name: i.type for i in enc0.get_inputs()}
    ii = lambda n, v: (np.array(v, np.int32) if 'int32' in et.get(n, '') else np.array(v, np.int64))
    lang = np.zeros((1, NUMPROMPTS), np.float32); lang[0, 0] = 1.0
    feeds = {
        "audio_signal": a.astype(np.float32),
        "audio_length": ii("audio_length", [CHUNK]),
        "language_mask": lang,
        "pre_cache": np.zeros((1, MELS, 9), np.float32),
        "cache_last_channel": np.zeros((24, 1, 56, 1024), np.float32),
        "cache_last_time": np.zeros((24, 1, 1024, 8), np.float32),
        "cache_last_channel_len": ii("cache_last_channel_len", [0]),
    }
    return {k: v for k, v in feeds.items() if k in et}

def expose_all_outputs(src, dst):
    import onnx
    m = onnx.load(src)  # external data loads alongside
    existing = {o.name for o in m.graph.output}
    names = []
    for node in m.graph.node:
        for o in node.output:
            if o and o not in existing:
                names.append(o)
    for n in names:
        m.graph.output.extend([onnx.helper.make_empty_tensor_value_info(n)])
    onnx.save(m, dst, save_as_external_data=False)
    return names

def run(model_dir, wav):
    import onnxruntime as ort
    feeds = make_input(model_dir, wav)
    avail = ort.get_available_providers()
    print("available providers:", avail)
    pa = 'CPUExecutionProvider'
    pb = 'DmlExecutionProvider' if 'DmlExecutionProvider' in avail else 'CPUExecutionProvider'
    print(f"comparing A={pa}  vs  B={pb}")

    exposed = os.path.join(model_dir, "_encoder_allout.onnx")
    names = expose_all_outputs(os.path.join(model_dir, "encoder.onnx"), exposed)
    print(f"exposed {len(names)} intermediate tensors")

    # provider placement: run with VERBOSE logging once and grep stderr for
    # "Node placements" / "placed on" to confirm DML actually executes the
    # encoder (vs silent CPU fallback at partition boundaries).
    so = ort.SessionOptions()
    so.log_severity_level = 0  # VERBOSE — node placement is logged here
    sa = ort.InferenceSession(exposed, providers=[pa])
    sb = ort.InferenceSession(exposed, sess_options=so, providers=[pb])
    print("B session providers:", sb.get_providers())
    out_names = [o.name for o in sa.get_outputs()]
    ra = dict(zip(out_names, sa.run(out_names, feeds)))
    rb = dict(zip(out_names, sb.run(out_names, feeds)))

    print(f"\n{'node output':50} {'max_abs_err':>12} {'cosine':>8} {'A|max|':>8} {'B|max|':>8}")
    first = None
    for n in out_names:
        x, y = ra[n], rb[n]
        if x.shape != y.shape or x.dtype.kind != 'f':
            continue
        xf, yf = x.astype(np.float64).ravel(), y.astype(np.float64).ravel()
        mae = float(np.abs(xf - yf).max()) if xf.size else 0.0
        denom = (np.linalg.norm(xf) * np.linalg.norm(yf)) or 1.0
        cos = float(np.dot(xf, yf) / denom)
        amax, bmax = float(np.abs(xf).max() or 0), float(np.abs(yf).max() or 0)
        flag = (mae > 1e-2 and cos < 0.99)
        if flag and first is None:
            first = n
        if flag or n == out_names[-1]:
            print(f"{n[:50]:50} {mae:12.4f} {cos:8.4f} {amax:8.3f} {bmax:8.3f}{'  <-- FIRST DIVERGENCE' if n==first else ''}")
    print(f"\nFIRST DIVERGENT NODE: {first}")

if __name__ == "__main__":
    run(sys.argv[1], sys.argv[2] if len(sys.argv) > 2 else None)
