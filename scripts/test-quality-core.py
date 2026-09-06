from pathlib import Path
import subprocess
import tempfile

root = Path.cwd()
with tempfile.TemporaryDirectory(prefix='clawscribe-core-') as temp:
    out = Path(temp)
    manifest = '''[package]
name = "clawscribe-quality-core"
version = "0.0.0"
edition = "2021"
[workspace]
[dependencies]
tokio = { version = "1", features = ["full"] }
once_cell = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "chrono"] }
uuid = { version = "1", features = ["v4"] }
'''
    modules = {
        'memory_queue': 'frontend/src-tauri/src/audio/memory_queue.rs',
        'process_runner': 'frontend/src-tauri/src/audio/process_runner.rs',
        'chat_context': 'frontend/src-tauri/src/summary/chat_context.rs',
        'chat_guard': 'frontend/src-tauri/src/summary/chat_guard.rs',
        'local_budget': 'llama-helper/src/budget.rs',
    }
    for name, source in modules.items():
        manifest += f'\n[[test]]\nname = "{name}"\npath = "{root / source}"\n'
    manifest += '\n[[test]]\nname = "chat_database"\npath = "database.rs"\n'
    # Use the real model definitions and repository methods with real SQLite.
    # Only omit unrelated model declarations that depend on the full desktop crate.
    models = (root / 'frontend/src-tauri/src/database/models.rs').read_text()
    start = models.index('pub struct SummaryProcess')
    end = models.rfind('#[derive', 0, start)
    (out / 'models.rs').write_text(models[:end])
    (out / 'database.rs').write_text(f'''pub mod database {{
    #[path = "{out / 'models.rs'}"] pub mod models;
    #[path = "{root / 'frontend/src-tauri/src/database/repositories/ai_chat.rs'}"] pub mod ai_chat;
}}
''')
    (out / 'Cargo.toml').write_text(manifest)
    subprocess.run(['cargo', 'test', '--manifest-path', str(out / 'Cargo.toml'), '--', '--nocapture'], check=True)
