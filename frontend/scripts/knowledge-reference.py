"""Independent PyTorch/model-card reference. Public synthetic inputs only.
Run only through the manually dispatched trusted-runner acceptance script.
All downloads use the candidate's immutable revision and SHA-256 pins.
"""
import hashlib
import json
import os
from pathlib import Path
import sys
import urllib.request

revision = "614241f622f53c4eeff9890bdc4f31cfecc418b3"
reference = Path(sys.argv[1])
reference.mkdir(parents=True, exist_ok=True)
files = {
    "model.safetensors": "1a55775f53449dac10a2bcbc312469fac40b96d53198c407081a831f81c98477",
    "config.json": "69137736cab8b8903a07fe8afaafdda25aac55415a12a55d1bffa9f581abf959",
    "tokenizer.json": "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39",
    "tokenizer_config.json": "a1d6bc8734a6f635dc158508bef000f8e2e5a759c7d92f984b2c86e5ff53425b",
    "special_tokens_map.json": "d05497f1da52c5e09554c0cd874037a083e1dc1b9cfd48034d1c717f1afc07a7",
}
def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()
for name, sha in files.items():
    path = reference / name
    if not path.exists() or digest(path) != sha:
        partial = reference / (name + ".partial")
        urllib.request.urlretrieve(f"https://huggingface.co/intfloat/multilingual-e5-small/resolve/{revision}/{name}", partial)
        if digest(partial) != sha:
            raise RuntimeError("Pinned reference artifact integrity failure")
        os.replace(partial, path)

import torch
from transformers import AutoModel, AutoTokenizer

torch.set_num_threads(2)
torch.set_num_interop_threads(1)
model = AutoModel.from_pretrained(reference, local_files_only=True).eval()
tokenizer = AutoTokenizer.from_pretrained(reference, local_files_only=True)
fixtures = json.loads(Path("frontend/src-tauri/tests/fixtures/knowledge/inputs.json").read_text(encoding="utf-8"))
results = []
with torch.inference_mode():
    for item in fixtures:
        encoded = tokenizer(item["purpose"] + ": " + item["text"], max_length=512, truncation=False, return_tensors="pt")
        hidden = model(**encoded).last_hidden_state
        mask = encoded["attention_mask"].unsqueeze(-1)
        pooled = (hidden * mask).sum(1) / mask.sum(1)
        vector = torch.nn.functional.normalize(pooled, dim=1)[0]
        results.append({**item, "ids": encoded["input_ids"][0].tolist(), "vector": vector.tolist()})
Path(sys.argv[2]).write_text(json.dumps(results), encoding="utf-8")
print(f"Independent PyTorch reference: {len(results)} synthetic inputs")
