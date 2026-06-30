# Hosted Transcription Smoke Test

ClawScribe has an ignored live smoke test for the beta hosted transcription
providers. It calls the production provider implementations directly, using a
short local audio file and credentials supplied through environment variables.

The test is intentionally not part of normal CI because it reaches external
APIs and requires real credentials. It does not print API keys.

The installed app also exposes an interactive version under **Settings >
Transcription**. Select Hosted Whisper or MAI-Transcribe, enter the provider
settings, then use **Test** to pick a short audio file and run the provider
against that file.

## What It Verifies

- Hosted Whisper returns non-empty segments and real word timestamps.
- Hosted Whisper word timestamps map to `timestamp_source: Real`.
- MAI-Transcribe returns non-empty segments without word timestamps.
- MAI-Transcribe output maps to `word_timestamps: None`.
- MAI collapsed output is reported through `requires_local_timing_grid=true`
  so reviewers can see whether Azure returned usable phrase segmentation.
- Hosted API errors are reported with the app's fallback category, such as
  `auth_config`, `transient`, or `upload_too_large`.

## Inputs

Use a short audio file with clear speech. Keep OpenAI-hosted Whisper uploads
below 25 MB.

Common optional input:

| Variable | Purpose |
| --- | --- |
| `CLAWSCRIBE_SMOKE_AUDIO` | Required path to the local test audio file. |
| `CLAWSCRIBE_SMOKE_LANGUAGE` | Optional language hint such as `en` or `en-US`. |

Hosted Whisper input:

| Variable | Purpose |
| --- | --- |
| `CLAWSCRIBE_SMOKE_OPENAI_API_KEY` | Enables the Hosted Whisper smoke path. |
| `CLAWSCRIBE_SMOKE_OPENAI_BASE_URL` | Optional OpenAI-compatible base URL. Defaults to `https://api.openai.com/v1`. |
| `CLAWSCRIBE_SMOKE_OPENAI_MODEL` | Optional model. Defaults to `whisper-1`. |

MAI-Transcribe input:

| Variable | Purpose |
| --- | --- |
| `CLAWSCRIBE_SMOKE_MAI_ENDPOINT` | Azure Speech endpoint, for example `https://example.cognitiveservices.azure.com`. |
| `CLAWSCRIBE_SMOKE_MAI_API_KEY` | Azure Speech resource key. |
| `CLAWSCRIBE_SMOKE_MAI_MODEL` | Optional model. Defaults to `mai-transcribe-1.5`. |

Configure either Hosted Whisper, MAI-Transcribe, or both. MAI requires both the
endpoint and key.

## PowerShell Example

From the repository root:

```powershell
cd frontend

$env:CLAWSCRIBE_SMOKE_AUDIO = "C:\path\to\short-speech.wav"
$env:CLAWSCRIBE_SMOKE_LANGUAGE = "en"

# Hosted Whisper
$env:CLAWSCRIBE_SMOKE_OPENAI_API_KEY = "<openai-or-compatible-key>"
# Optional:
# $env:CLAWSCRIBE_SMOKE_OPENAI_BASE_URL = "https://api.openai.com/v1"
# $env:CLAWSCRIBE_SMOKE_OPENAI_MODEL = "whisper-1"

# MAI-Transcribe
$env:CLAWSCRIBE_SMOKE_MAI_ENDPOINT = "https://example.cognitiveservices.azure.com"
$env:CLAWSCRIBE_SMOKE_MAI_API_KEY = "<azure-speech-key>"
# Optional:
# $env:CLAWSCRIBE_SMOKE_MAI_MODEL = "mai-transcribe-1.5"

pnpm run test:cloud-live
```

Expected output includes one summary line for each configured provider:

```text
Hosted Whisper: segments=..., word_timestamps=..., requires_local_timing_grid=false
MAI-Transcribe: segments=..., word_timestamps=0, requires_local_timing_grid=...
```

If MAI reports `requires_local_timing_grid=true`, Azure returned collapsed or
single-phrase output. That is not automatically a failed smoke test, because
ClawScribe can remap the cloud text to the local VAD timing grid, but it should
be called out in release review.

## Direct Cargo Command

The package script wraps this command:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --lib audio::transcription::cloud::live_smoke::hosted_api_transcription_live_smoke -- --ignored --nocapture --test-threads=1
```
