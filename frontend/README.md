# ClawScribe Frontend

The Next.js UI and Tauri desktop shell for ClawScribe `0.5.32`.
ClawScribe records, transcribes, summarizes, and exports meetings from the
local desktop app.

## Features

- Real-time audio recording from both microphone and system audio
- Local transcription using Whisper, Parakeet, or Nemotron engines depending on
  the selected model
- Beta cloud retranscription through Hosted Whisper or Azure Speech
  MAI-Transcribe when explicitly enabled
- Native desktop integration using Tauri
- Speaker diarization support
- Rich text editor for note-taking
- Privacy-focused defaults: recording and transcription are local unless a
  cloud transcription beta provider, external summary provider, export, or
  OpenClaw provider is configured

## Prerequisites

### For macOS:
- Node.js (v20 recommended)
- Rust (latest stable)
- pnpm (v10 recommended)
- [Xcode Command Line Tools](https://developer.apple.com/download/all/?q=xcode)

### For Windows:
- Node.js (v20 recommended)
- Rust (latest stable)
- pnpm (v10 recommended)
- Visual Studio Build Tools with C++ development tools
- Windows 10 or later


## Project Structure

```
/frontend
├── src/                   # Next.js frontend code
├── src-tauri/             # Rust/Tauri app core
├── public/                # Static assets
└── package.json           # Project dependencies
```

## Installation

### For macOS:

1. Install prerequisites:
   ```bash
   # Install Homebrew if not already installed
   /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
   
   # Install Node.js
   brew install node
   
   # Install Rust
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   
   # Install pnpm
   npm install -g pnpm
   
   # Install Xcode Command Line Tools
   xcode-select --install
   ```

2. Clone the repository and navigate to the frontend directory:
   ```bash
   git clone https://github.com/ch3573r/ClawScribe
   cd ClawScribe/frontend
   ```
  

3. Install dependencies:
   ```bash
   pnpm install
   ```

### For Windows:

1. Install prerequisites:
   - Install [Node.js](https://nodejs.org/) (v18 or later)
   - Install [Rust](https://www.rust-lang.org/tools/install)
   - Install pnpm: `npm install -g pnpm`
   - Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with C++ development tools

2. Clone the repository and navigate to the frontend directory:
   ```cmd
   git clone https://github.com/ch3573r/ClawScribe
   cd ClawScribe/frontend
   ```

3. Install dependencies:
   ```cmd
   pnpm install
   ```

## Running the App

### For macOS:

Use the package scripts to run the app in development mode:
```bash
pnpm run tauri:dev
```

To build a production version:
```bash
pnpm run tauri:build
```

Legacy helper scripts such as `clean_run.sh` and `clean_build.sh` are still in
the tree, but the package scripts are the current documented path.

GPU-specific helpers are also available:

```bash
./dev-gpu.sh
./build-gpu.sh
```

### For Windows:

Use the package scripts to run the app in development mode:
```cmd
pnpm run tauri:dev
```

To build a production version:
```cmd
pnpm run tauri:build
```

Windows GPU release-parity builds use the `windows-gpu` feature set:

```cmd
pnpm run tauri:dev:windows-gpu
pnpm run tauri:build:windows-gpu
```

Legacy helper scripts such as `clean_run_windows.bat` and
`clean_build_windows.bat` are still in the tree, but the package scripts are
the current documented path.

## Local Transcription

Current ClawScribe does not require a separate FastAPI service, Docker backend, or manually started whisper-server process. Local transcription is handled by the Rust/Tauri desktop app.

## Cloud Transcription Beta

Cloud retranscription providers are opt-in and beta-gated. Hosted Whisper uses
OpenAI-compatible file transcription and can provide real word timestamps. The
OpenAI-hosted endpoint has a 25 MB upload limit; larger recordings fall back to
local transcription.

MAI-Transcribe uses Azure Speech Fast Transcription with separate Cognitive
Services credentials. It returns sentence-level timing only, so ClawScribe does
not fabricate word timestamps. Collapsed MAI output can be remapped to the
local VAD timing grid for readable rows, but the timing is approximate and
speaker diarization remains conservative.

For live hosted-provider verification with real credentials and a short audio
file:

```bash
pnpm run test:cloud-live
```

See [Hosted Transcription Smoke Test](../docs/hosted-transcription-smoke.md)
for the required environment variables.

For build and acceleration details, see:

- [Building from Source](../docs/BUILDING.md)
- [GPU Acceleration](../docs/GPU_ACCELERATION.md)
- [Architecture](../docs/architecture.md)

## Development

### Frontend (Next.js)
- The frontend is built with Next.js and Tailwind CSS
- Source code is in the `src/` directory
- To run only the frontend: `pnpm run dev`

### Backend (Tauri)
- The Rust/Tauri app core is in the `src-tauri/` directory
- Handles audio capture, file system access, transcription, storage, and native integrations
- To run only the Tauri development server: `pnpm run tauri:dev`

## Troubleshooting

### Common Issues on macOS
- If you encounter permission issues with scripts, make them executable:
  ```bash
  chmod +x clean_run.sh clean_build.sh
  ```
- For microphone access issues, ensure the app has microphone permissions in System Preferences

### Common Issues on Windows
- If you encounter build errors, ensure Visual Studio Build Tools are properly installed
- For audio capture issues, check Windows privacy settings for microphone access
- If the app fails to start, try running Command Prompt as administrator

## Contributing

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add some amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## License

This project is licensed under the MIT License - see the LICENSE file for details.
