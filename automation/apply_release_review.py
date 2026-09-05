from pathlib import Path
import shutil

root = Path.cwd()
def replace(path, old, new):
    p = root / path
    s = p.read_text()
    if old not in s:
        raise RuntimeError(f"Expected source did not match: {path}")
    p.write_text(s.replace(old, new, 1))

f = 'frontend/src-tauri/src/audio/hardware_detector.rs'
replace(f, '''        // Simple memory detection - could be enhanced with system-specific calls
        match std::env::var("MEMORY_GB") {
            Ok(mem_str) => mem_str.parse().unwrap_or(8),
            Err(_) => {
                // Default estimates based on common configurations
                8 // Conservative default
            }
        }
''', '''        let override_value = std::env::var("MEMORY_GB").ok();
        let mut system = sysinfo::System::new();
        // Refresh only memory; enumerating all processes here is unnecessary.
        system.refresh_memory();
        Self::resolve_memory_gb(override_value.as_deref(), system.total_memory())
    }

    fn resolve_memory_gb(override_value: Option<&str>, total_bytes: u64) -> u8 {
        if let Some(value) = override_value
            .and_then(|value| value.trim().parse::<u8>().ok())
            .filter(|value| *value > 0)
        {
            return value;
        }
        if total_bytes == 0 {
            // Unknown hardware must not be treated as a high-memory system.
            return 4;
        }
        (total_bytes / (1024 * 1024 * 1024)).clamp(1, u8::MAX as u64) as u8
    }

    fn meeting_thread_limit(cpu_cores: u8, memory_gb: u8) -> usize {
        // Leave capacity for the call, audio capture, and UI. This is a
        // conservative starting policy, not a hardware performance guarantee.
        let limit = if memory_gb <= 8 { 4 } else { 8 };
        usize::from(cpu_cores.saturating_sub(1).max(1)).min(limit)
''')
replace(f, 'max_threads: Some(self.cpu_cores.min(8) as usize),', 'max_threads: Some(Self::meeting_thread_limit(self.cpu_cores, self.memory_gb)),')
replace(f, '    /// Detect available system memory in GB', '    /// Detect total physical system memory in GiB (cached with the profile)')
replace(f, '    #[test]\n    fn test_hardware_detection()', '''    #[test]
    fn memory_detection_uses_measured_bytes_and_valid_overrides() {
        let gib = 1024 * 1024 * 1024;
        assert_eq!(HardwareProfile::resolve_memory_gb(None, 8 * gib), 8);
        assert_eq!(HardwareProfile::resolve_memory_gb(None, 32 * gib), 32);
        assert_eq!(HardwareProfile::resolve_memory_gb(Some("16"), 8 * gib), 16);
        for invalid in ["0", "-1", "bad", "256", ""] {
            assert_eq!(HardwareProfile::resolve_memory_gb(Some(invalid), 8 * gib), 8);
        }
        assert_eq!(HardwareProfile::resolve_memory_gb(None, 0), 4);
        assert_eq!(HardwareProfile::resolve_memory_gb(None, u64::MAX), 255);
    }

    #[test]
    fn notebook_threads_leave_room_for_the_meeting() {
        assert_eq!(HardwareProfile::meeting_thread_limit(12, 8), 4);
        assert_eq!(HardwareProfile::meeting_thread_limit(12, 16), 8);
        assert_eq!(HardwareProfile::meeting_thread_limit(2, 8), 1);
        assert_eq!(HardwareProfile::meeting_thread_limit(1, 8), 1);
        assert_eq!(HardwareProfile::meeting_thread_limit(0, 0), 1);
    }

    #[test]
    fn test_hardware_detection()''')

f = 'frontend/src/components/VirtualizedTranscriptView.tsx'
replace(f, 'useState, memo, useMemo }', 'useState, memo, useMemo, useId }')
replace(f, 'import { ConfidenceIndicator }', 'import { toast } from "sonner";\nimport { ConfidenceIndicator }')
replace(f, "    if (seconds === undefined) return '[--:--]';", "    if (seconds === undefined || !Number.isFinite(seconds) || seconds < 0) return '[--:--]';")
replace(f, '    const [isSaving, setIsSaving] = useState(false);', '''    const [isSaving, setIsSaving] = useState(false);
    const savingRef = useRef(false);
    const speakerInputId = useId();''')
replace(f, '    const canSeek = Boolean(onSeekToTime && Number.isFinite(timestamp));', '    const canSeek = Boolean(onSeekToTime && Number.isFinite(timestamp) && timestamp >= 0);')
p = root / f
s = p.read_text()
a = s.index('    const saveSpeaker = async')
b = s.index('    const speakerClass =', a)
s = s[:a] + '''    const persistSpeakerChange = async (operation: () => Promise<unknown> | unknown): Promise<boolean> => {
        if (savingRef.current) return false;
        savingRef.current = true;
        setIsSaving(true);
        try {
            await operation();
            return true;
        } catch {
            toast.error("Could not save the speaker label. Please try again.");
            return false;
        } finally {
            savingRef.current = false;
            setIsSaving(false);
        }
    };

    const saveSpeaker = (nextSpeaker: string | null): Promise<boolean> => {
        if (!onSpeakerChange) return Promise.resolve(false);
        return persistSpeakerChange(() => onSpeakerChange(id, nextSpeaker));
    };

    const replaceSpeaker = (nextSpeaker: string | null): Promise<boolean> => {
        if (canReplaceMatching && onApplySpeakerToMatching) {
            return persistSpeakerChange(() => onApplySpeakerToMatching(currentSpeaker, nextSpeaker));
        }
        return saveSpeaker(nextSpeaker);
    };

    const saveCustomSpeaker = async () => {
        const nextSpeaker = customSpeaker.trim().replace(/\\s+/g, " ");
        if (!nextSpeaker) return;
        if (await replaceSpeaker(nextSpeaker)) setCustomSpeaker("");
    };

''' + s[b:]
a = s.index('            role={canSeek ?')
b = s.index('        >\n            <div className="flex items-start', a)
s = s[:a] + '''            className={`mb-4 rounded-[4px] px-1 py-0.5 transition-colors motion-reduce:transition-none ${isActive ? "bg-accent/10 ring-1 ring-accent/30" : ""}`}
''' + s[b:]
a = s.index('                    <TooltipTrigger>')
b = s.index('                    <TooltipContent>', a)
s = s[:a] + '''                    <TooltipTrigger asChild>
                        <button
                            type="button"
                            disabled={!canSeek}
                            onClick={seekToSegment}
                            aria-label={`Play recording from ${formatRecordingTime(timestamp)}`}
                            className="flex min-h-8 min-w-[3.5rem] flex-shrink-0 items-center rounded font-mono text-[11px] tabular-nums text-muted-foreground enabled:hover:bg-muted enabled:hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-default"
                        >
                            {formatRecordingTime(timestamp)}
                        </button>
                    </TooltipTrigger>
''' + s[b:]
s = s.replace('<label className="text-xs font-medium text-muted-foreground">', '<label htmlFor={speakerInputId} className="text-xs font-medium text-muted-foreground">', 1)
s = s.replace('                                        <input\n                                            value={customSpeaker}', '                                        <input\n                                            id={speakerInputId}\n                                            disabled={isSaving}\n                                            value={customSpeaker}', 1)
s = s.replace('disabled={!customSpeaker.trim()}', 'disabled={isSaving || !customSpeaker.trim()}', 1)
s = s.replace('    useAutoScroll({', '    const { autoScroll, scrollToBottom } = useAutoScroll({', 1)
i = s.rindex('            </div>\n        </div>\n    );')
s = s[:i] + '''            </div>
            {isRecording && !disableAutoScroll && !autoScroll && segments.length > 0 && (
                <div className="sticky bottom-3 z-20 flex justify-end pt-2 pointer-events-none">
                    <button
                        type="button"
                        onClick={scrollToBottom}
                        className="pointer-events-auto min-h-9 rounded-full border border-border bg-card px-4 text-sm font-medium text-foreground shadow-sm hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                    >
                        Jump to live transcript
                    </button>
                </div>
            )}
''' + s[i + len('            </div>\n'):]
p.write_text(s)

f = 'frontend/scripts/build-windows-release.ps1'
replace(f, 'Assert-Command "cargo"', 'Assert-Command "cargo"\nAssert-Command "git"')
replace(f, '    pnpm install --frozen-lockfile', '    pnpm install --frozen-lockfile\n    if ($LASTEXITCODE -ne 0) { throw "Frontend dependency installation failed." }')
replace(f, '        cargo check @featureArgs', '        cargo check --locked @featureArgs\n        if ($LASTEXITCODE -ne 0) { throw "Native Rust validation failed." }')
replace(f, '    pnpm exec tsc --noEmit\n    exit 0', '    pnpm exec tsc --noEmit\n    if ($LASTEXITCODE -ne 0) { throw "Frontend typecheck failed." }\n    pnpm test\n    if ($LASTEXITCODE -ne 0) { throw "Frontend regression tests failed." }\n    exit 0')
replace(f, 'pnpm build\n', 'pnpm exec tsc --noEmit\nif ($LASTEXITCODE -ne 0) { throw "Frontend typecheck failed." }\npnpm test\nif ($LASTEXITCODE -ne 0) { throw "Frontend regression tests failed." }\npnpm build\nif ($LASTEXITCODE -ne 0) { throw "Frontend production build failed." }\n')
replace(f, '    pnpm exec tauri build\n', '    pnpm exec tauri build -- --locked\n')
replace(f, '    pnpm exec tauri build -- @featureArgs\n', '    pnpm exec tauri build -- @featureArgs --locked\n')
replace(f, '\n$sourceVersion = (node -p', '\nif ($LASTEXITCODE -ne 0) { throw "Windows installer build failed." }\n\n$sourceVersion = (node -p')
replace(f, '''if ($artifactFiles.Count -eq 0) {
    throw "No Windows release artifacts were found under '$bundleRoot'."
}''', '''$expectedMsi = "ClawScribe_${sourceVersion}_x64_en-US.msi"
$expectedNsis = "ClawScribe_${sourceVersion}_x64-setup.exe"
foreach ($expected in @($expectedMsi, $expectedNsis)) {
    if (-not ($artifactFiles | Where-Object { $_.Name -eq $expected -and $_.Length -gt 0 })) {
        throw "Required current-version installer '$expected' is missing or empty."
    }
}''')
replace(f, '$relativePath = [System.IO.Path]::GetRelativePath($resolvedBundleRoot, $artifact.FullName).Replace("\\", "/")', '$relativePath = $artifact.FullName.Substring($resolvedBundleRoot.Length).TrimStart([char]92, [char]47).Replace("\\", "/")')
for f in ['frontend/package.json', 'frontend/src-tauri/tauri.conf.json']:
    replace(f, '"version": "0.5.35"', '"version": "0.5.36"')
replace('frontend/src-tauri/Cargo.toml', 'version = "0.5.35"', 'version = "0.5.36"')
replace('Cargo.lock', 'name = "clawscribe"\nversion = "0.5.35"', 'name = "clawscribe"\nversion = "0.5.36"')
replace('frontend/package.json', 'tests/lib/transcript-display.test.mjs"', 'tests/lib/transcript-display.test.mjs tests/lib/transcript-interaction.test.mjs"')
shutil.copyfile(Path(__file__).parent / 'transcript-interaction.test.mjs', root / 'frontend/tests/lib/transcript-interaction.test.mjs')

p = root / 'frontend/src-tauri/src/summary/processor.rs'
s = p.read_text()
needle = '    info!(\n        "Starting summary generation with provider: {:?}, model: {}",'
s = s.replace(needle, '''    if text.trim().is_empty() {
        return Err("No transcript is available to summarize. Record or import speech first.".to_string());
    }
''' + needle, 1)
needle = '    #[test]\n    fn failed_summary_chunk_is_not_silently_omitted()'
s = s.replace(needle, '''    #[tokio::test]
    async fn empty_transcript_is_rejected_before_any_provider_request() {
        let template = Template {
            name: "Test".into(), description: "Test".into(), sections: vec![],
        };
        let result = generate_meeting_summary(
            &Client::new(), &LLMProvider::Ollama, "test", "", "  \\n", "", "test",
            &template, 4000, None, None, None, None, None, None, None,
            Some("en"), Some("en"), None,
        ).await;
        assert!(result.unwrap_err().starts_with("No transcript is available"));
    }

''' + needle, 1)
p.write_text(s)
