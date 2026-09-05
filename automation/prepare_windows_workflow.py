from pathlib import Path
r = Path.cwd()
p = r / '.github/workflows/clawscribe-windows-release.yml'
s = p.read_text()
call = '''  workflow_call:
    inputs:
      feature:
        type: string
        default: windows-gpu
      check-only:
        type: boolean
        default: false
      build-ref:
        type: string
        default: ""
      directml:
        type: boolean
        default: false
      publish:
        type: boolean
        default: false
      draft-release:
        type: boolean
        default: false
      capture-smoke-confirmed:
        type: boolean
        default: false
    secrets:
      TAURI_SIGNING_PRIVATE_KEY:
        required: false
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD:
        required: false
'''
s = s.replace('on:\n', 'on:\n' + call, 1)
s = s.replace('      capture-smoke-confirmed:\n        description:', '''      draft-release:
        description: Stage a draft release for testing without publishing or changing the stable updater.
        required: false
        type: boolean
        default: false
      capture-smoke-confirmed:
        description:''', 1)
s = s.replace('    permissions:\n', '    timeout-minutes: 90\n    permissions:\n', 1)
s = s.replace('        run: |\n', '        run: |\n          $ErrorActionPreference = "Stop"\n          $PSNativeCommandUseErrorActionPreference = $true\n')
marker = '      - name: Require dual-source capture smoke before publishing\n'
assert marker in s
s = s.replace(marker, '''      - name: Validate release mode and identify exact build commit
        id: build_identity
        shell: pwsh
        run: |
          $ErrorActionPreference = "Stop"
          $PSNativeCommandUseErrorActionPreference = $true
          if ("${{ inputs.publish }}" -eq "true" -and "${{ inputs['draft-release'] }}" -eq "true") {
            throw "Select either publication or a draft, not both."
          }
          $commit = (git rev-parse HEAD).Trim()
          "commit=$commit" | Out-File -FilePath $env:GITHUB_OUTPUT -Encoding utf8 -Append

''' + marker, 1)
s = s.replace("inputs.publish && !inputs['check-only']", "(inputs.publish || inputs['draft-release']) && !inputs['check-only']")
s = s.replace("!inputs['check-only'] && !inputs.publish", "!inputs['check-only'] && !inputs.publish && !inputs['draft-release']")
# Public releases still require the original real dual-source recording gate.
s = s.replace('          cargo build -p llama-helper --release --target x86_64-pc-windows-msvc', '          cargo build -p llama-helper --release --locked --target x86_64-pc-windows-msvc')
s = s.replace('          pnpm install --frozen-lockfile\n          pnpm build\n', '          pnpm install --frozen-lockfile\n          pnpm typecheck\n          pnpm test\n')
s = s.replace('            pnpm exec tauri build -- --features $features', '            pnpm exec tauri build -- --locked --features $features')
s = s.replace('            pnpm exec tauri build\n', '            pnpm exec tauri build -- --locked\n')
s = s.replace('      - name: Write build metadata\n', '''      - name: Run native summary and resource regression tests
        if: ${{ !inputs['check-only'] }}
        shell: pwsh
        run: |
          $ErrorActionPreference = "Stop"
          $PSNativeCommandUseErrorActionPreference = $true
          $featureList = @()
          if ("${{ inputs.feature }}" -ne "cpu") { $featureList += "${{ inputs.feature }}" }
          if ("${{ inputs.directml }}" -eq "true" -and "${{ inputs.feature }}" -notin @("windows-gpu", "directml")) { $featureList += "directml" }
          $featureArgs = @()
          if ($featureList.Count -gt 0) { $featureArgs = @("--features", ($featureList -join ",")) }
          foreach ($filter in @("summary::processor::tests", "summary::chunking::tests", "audio::async_logger::tests", "audio::hardware_detector::tests")) {
            cargo test --locked --manifest-path frontend/src-tauri/Cargo.toml @featureArgs --lib $filter -- --test-threads=1
          }

      - name: Write build metadata
''', 1)
s = s.replace('          # The app updater points at /releases/latest/download/latest.json.\n', '          target_commitish: ${{ steps.build_identity.outputs.commit }}\n          draft: ${{ inputs[\'draft-release\'] }}\n          # Drafts never advance the installed stable updater.\n          # The app updater points at /releases/latest/download/latest.json.\n', 1)
s = s.replace('          make_latest: true', '          make_latest: ${{ inputs[\'draft-release\'] && \'false\' || \'true\' }}')
s = s.replace('      - name: Publish GitHub Release\n', '''      - name: Refuse to turn a published version into a draft
        if: ${{ inputs['draft-release'] && !inputs['check-only'] }}
        uses: actions/github-script@v7
        env:
          RELEASE_TAG: v${{ env.CLAWSCRIBE_SOURCE_VERSION }}
        with:
          script: |
            try {
              const { data: release } = await github.rest.repos.getReleaseByTag({
                ...context.repo, tag: process.env.RELEASE_TAG
              });
              if (!release.draft) core.setFailed('This version is already published. Bump the version; never replace a public release with a draft.');
            } catch (error) {
              if (error.status !== 404) throw error;
            }

      - name: Publish GitHub Release
''', 1)
s = s.replace('          $lines += $changelog\n', '''          if ("${{ inputs['draft-release'] }}" -eq "true") {
            $lines += "> DRAFT: native build and automated checks only. Real dual-source recording, installer interaction, and target-notebook performance must be verified before publication."
            $lines += ""
          }
          $lines += $changelog
''', 1)
p.write_text(s)
(r / '.github/workflows/windows-candidate.yml').write_text('''name: Stage Windows release candidate

on:
  push:
    branches: ['release/**']
  workflow_dispatch:

permissions:
  contents: write

concurrency:
  group: windows-candidate-${{ github.ref }}
  cancel-in-progress: true

jobs:
  validate:
    runs-on: ubuntu-latest
    timeout-minutes: 15
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v4
        with:
          version: 10
      - uses: actions/setup-node@v4
        with:
          node-version: 24
      - run: node scripts/verify-public-repo-safety.mjs
      - run: pnpm install --frozen-lockfile
        working-directory: frontend
      - run: pnpm typecheck && pnpm test
        working-directory: frontend
  stage:
    needs: validate
    uses: ./.github/workflows/clawscribe-windows-release.yml
    with:
      feature: windows-gpu
      build-ref: ${{ github.sha }}
      publish: false
      draft-release: true
      capture-smoke-confirmed: false
    secrets: inherit
''')
p = r / 'CHANGELOG.md'
s = p.read_text()
i = s.index('## ')
s = s[:i] + '''## 0.5.36

- Preserve all transcript content at summary chunk boundaries, including Unicode.
- Reject failed or empty summary chunks rather than silently publishing incomplete notes.
- Ground summaries in stated facts, distinguish proposals from decisions, and retain uncertainty.
- Preserve meaningful multilingual words in the transcript display.
- Improve live-follow scrolling, user interruption, cleanup, and reduced-motion navigation.
- Add a visible jump-to-live control and accessible timestamp playback buttons.
- Preserve custom speaker input on failed saves, show an actionable error, and prevent duplicate submissions.
- Detect physical RAM instead of assuming 8 GB; cap Windows Whisper threads conservatively on 8 GB notebooks.
- Bound diagnostic logging and flush quiet batches without dropping audio.
- Include the previously unreleased token-storage encryption and short-utterance preservation fixes.
- Make Windows build failures fatal, require both current-version installers, and run native regression tests before staging release assets.
- Support draft Windows releases without changing the stable updater or bypassing the real audio-capture publication gate.

''' + s[i:]
p.write_text(s)
