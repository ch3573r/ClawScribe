from pathlib import Path
import hashlib
import json
import subprocess
import sys

root = Path.cwd()
edit_root = Path(sys.argv[1]).resolve()
parts = sorted(edit_root.glob('*.json'))
if len(parts) != 7:
    raise SystemExit('Expected all seven reviewed edit groups')
seen = set()
for part in parts:
    for change in json.loads(part.read_text()):
        name = change['path']
        if name in seen or '..' in Path(name).parts or Path(name).is_absolute():
            raise SystemExit(f'Invalid or duplicate source path: {name}')
        seen.add(name)
        target = root / name
        original = target.read_bytes() if target.exists() else b''
        actual = hashlib.sha1(b'blob ' + str(len(original)).encode() + b'\0' + original).hexdigest() if target.exists() else None
        if actual != change['base']:
            raise SystemExit(f'Baseline differs from inspected source: {name}')
        lines = original.decode('utf-8').splitlines(keepends=True)
        for start, end, replacement in reversed(change['edits']):
            if not 0 <= start <= end <= len(lines):
                raise SystemExit(f'Invalid edit range: {name}')
            lines[start:end] = [replacement]
        result = ''.join(lines).encode('utf-8')
        digest = hashlib.sha256(result).hexdigest()
        if digest != change['sha256']:
            raise SystemExit(f'Reviewed-result hash mismatch: {name}: {digest}')
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(result)
        print(f'Verified reviewed bytes: {name}')
subprocess.run(['git', 'add', '--', *sorted(seen)], check=True)
print(f'Applied {len(seen)} exact reviewed files; validation follows separately.')
