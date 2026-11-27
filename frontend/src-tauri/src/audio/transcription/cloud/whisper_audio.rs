use super::openai_whisper::OpenAiWhisperProvider;
use super::{
    check_import_upload_opt_in, cloud_cancelled, CloudTranscriptSegment, CloudTranscriptionError,
    CUSTOM_WHISPER_MAX_UPLOAD_BYTES,
};
use crate::audio::batch_audio::{audio_decoder_command, unless_cancelled};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use std::{fs, io};
use tauri::{AppHandle, Manager, Runtime};

// 32 kbps for 100 minutes is 24 MB. Leave room for encoder padding and
// multipart overhead, and check the actual size again before each request.
const SEGMENT_SECONDS: u32 = 6_000;
const PREPARE_ABOVE_BYTES: u64 = 24_000_000;
const MAX_AUDIO_SECONDS: u32 = 24 * 60 * 60;
const SCRATCH_JOB_PREFIX: &str = "whisper-job-";

pub(crate) fn whisper_scratch_root<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    app.path()
        .app_cache_dir()
        .ok()
        .map(|cache| cache.join("hosted-whisper"))
}

pub(crate) fn cleanup_abandoned_whisper_parts<R: Runtime>(app: &AppHandle<R>) {
    if let Some(root) = whisper_scratch_root(app) {
        if sweep_scratch_jobs(&root).is_err() {
            log::warn!("Could not remove all abandoned Hosted Whisper audio from the app cache");
        }
    }
}

fn create_scratch_job(root: &Path) -> io::Result<tempfile::TempDir> {
    fs::create_dir_all(root)?;
    check_scratch_root(root)?;
    tempfile::Builder::new()
        .prefix(SCRATCH_JOB_PREFIX)
        .tempdir_in(root)
}

fn check_scratch_root(root: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(root)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::other("Hosted Whisper cache is a reparse point"));
        }
    }
    if !metadata.is_dir() {
        return Err(io::Error::other("Hosted Whisper cache is not a directory"));
    }
    Ok(())
}

fn sweep_scratch_jobs(root: &Path) -> io::Result<()> {
    match check_scratch_root(root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        result => result?,
    }
    let mut last_error = None;
    for entry in fs::read_dir(root)? {
        let result = entry.and_then(|entry| {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(SCRATCH_JOB_PREFIX)
                && entry.file_type()?.is_dir()
            {
                fs::remove_dir_all(entry.path())?;
            }
            Ok(())
        });
        if let Err(error) = result {
            if error.kind() != io::ErrorKind::NotFound {
                last_error = Some(error);
            }
        }
    }
    last_error.map_or(Ok(()), Err)
}

fn should_prepare_audio(openai_hosted: bool, bytes: u64) -> bool {
    let threshold = if openai_hosted {
        PREPARE_ABOVE_BYTES
    } else {
        CUSTOM_WHISPER_MAX_UPLOAD_BYTES
    };
    bytes > threshold
}

pub(crate) async fn transcribe_whisper_file(
    client: &OpenAiWhisperProvider,
    audio_path: &Path,
    scratch_root: Option<&Path>,
    language: Option<&str>,
    cancelled: &AtomicBool,
    consent_required: bool,
    progress: impl Fn(u32, &str),
) -> Result<Vec<CloudTranscriptSegment>, CloudTranscriptionError> {
    check_import_upload_opt_in(consent_required)?;
    let bytes = tokio::fs::metadata(audio_path)
        .await
        .map_err(|_| CloudTranscriptionError::auth_config("Could not inspect audio file"))?
        .len();
    let segments = if should_prepare_audio(client.is_openai_hosted(), bytes) {
        progress(0, "Preparing compact audio for Hosted Whisper...");
        let root = scratch_root.ok_or_else(scratch_error)?;
        let scratch = create_scratch_job(root).map_err(|_| scratch_error())?;
        let parts = prepare_parts(audio_path, scratch.path(), SEGMENT_SECONDS, cancelled).await?;
        upload_parts(
            &parts,
            |path| upload(client, path, language, cancelled, consent_required),
            &progress,
        )
        .await?
    } else {
        upload(client, audio_path, language, cancelled, consent_required).await?
    };
    complete_transcript(segments, &progress)
}

async fn upload_parts<'a, F>(
    parts: &'a [AudioPart],
    mut upload_part: impl FnMut(&'a Path) -> F,
    progress: &impl Fn(u32, &str),
) -> Result<Vec<CloudTranscriptSegment>, CloudTranscriptionError>
where
    F: Future<Output = Result<Vec<CloudTranscriptSegment>, CloudTranscriptionError>>,
{
    let count = parts.len();
    let mut segments = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        progress(
            (100 * index / count) as u32,
            &format!("Transcribing audio part {} of {count}...", index + 1),
        );
        let mut result = upload_part(&part.path).await?;
        part.offset_segments(&mut result);
        segments.extend(result);
    }
    Ok(segments)
}

fn complete_transcript(
    mut segments: Vec<CloudTranscriptSegment>,
    progress: &impl Fn(u32, &str),
) -> Result<Vec<CloudTranscriptSegment>, CloudTranscriptionError> {
    segments.retain(|segment| !segment.text.trim().is_empty());
    if segments.is_empty() {
        return Err(CloudTranscriptionError::provider_output(
            "Hosted Whisper returned no transcript text",
        ));
    }
    progress(100, "Hosted Whisper transcription complete");
    Ok(segments)
}

async fn upload(
    client: &OpenAiWhisperProvider,
    path: &Path,
    language: Option<&str>,
    cancelled: &AtomicBool,
    consent_required: bool,
) -> Result<Vec<CloudTranscriptSegment>, CloudTranscriptionError> {
    unless_cancelled(
        client.transcribe_path(path, language, consent_required),
        cancelled,
    )
    .await
    .ok_or_else(cloud_cancelled)?
}

struct AudioPart {
    path: PathBuf,
    start: f64,
    end: f64,
}

impl AudioPart {
    fn offset_segments(&self, segments: &mut [CloudTranscriptSegment]) {
        let absolute = |relative: f64| (self.start + relative).clamp(self.start, self.end);
        for segment in segments {
            segment.start_seconds = absolute(segment.start_seconds);
            segment.end_seconds = if segment.requires_local_timing_grid {
                // Keep a usable part range if local VAD cannot find a grid.
                self.end
            } else {
                absolute(segment.end_seconds)
            };
            for word in segment.words.iter_mut().flatten() {
                word.start_seconds = absolute(word.start_seconds);
                word.end_seconds = absolute(word.end_seconds);
            }
        }
    }
}

async fn prepare_parts(
    source: &Path,
    directory: &Path,
    segment_seconds: u32,
    cancelled: &AtomicBool,
) -> Result<Vec<AudioPart>, CloudTranscriptionError> {
    let mut command = audio_decoder_command(source)
        .map_err(|error| CloudTranscriptionError::audio_preparation(error.to_string()))?;
    let manifest = directory.join("parts.csv");
    let mut child = command
        .args([
            "-map_metadata",
            "-1",
            "-map_chapters",
            "-1",
            "-c:a",
            "libmp3lame",
            "-b:a",
            "32k",
        ])
        .args(["-t", &MAX_AUDIO_SECONDS.to_string(), "-f", "segment"])
        .args([
            "-segment_time",
            &segment_seconds.to_string(),
            "-reset_timestamps",
            "1",
        ])
        .arg("-segment_list")
        .arg(&manifest)
        .arg(directory.join("part-%03d.mp3"))
        .spawn()
        .map_err(|_| preparation_error())?;
    match unless_cancelled(
        tokio::time::timeout(Duration::from_secs(30 * 60), child.wait()),
        cancelled,
    )
    .await
    {
        Some(Ok(Ok(status))) if status.success() => {}
        result => {
            let _ = child.kill().await;
            return Err(match result {
                None => cloud_cancelled(),
                Some(Err(_)) => CloudTranscriptionError::audio_preparation(
                    "Hosted Whisper audio preparation timed out",
                ),
                _ => preparation_error(),
            });
        }
    }
    let contents = tokio::fs::read_to_string(manifest)
        .await
        .map_err(|_| preparation_error())?;
    parse_parts(directory, &contents)
}

fn parse_parts(
    directory: &Path,
    manifest: &str,
) -> Result<Vec<AudioPart>, CloudTranscriptionError> {
    let mut parts: Vec<AudioPart> = Vec::new();
    for (index, line) in manifest.lines().enumerate() {
        let name = format!("part-{index:03}.mp3");
        let (start, end) = line
            .strip_prefix(&name)
            .and_then(|times| times.strip_prefix(','))
            .and_then(|times| times.split_once(','))
            .and_then(|(start, end)| Some((start.parse::<f64>().ok()?, end.parse::<f64>().ok()?)))
            .filter(|(start, end)| {
                start.is_finite() && end.is_finite() && *start >= 0.0 && end > start
            })
            .ok_or_else(preparation_error)?;
        if end >= f64::from(MAX_AUDIO_SECONDS) {
            return Err(CloudTranscriptionError::upload_too_large(
                "Hosted Whisper audio must be shorter than 24 hours. Choose a shorter file or use local transcription.",
            ));
        }
        parts.push(AudioPart {
            path: directory.join(name),
            start,
            end,
        });
    }
    if parts.is_empty() {
        return Err(preparation_error());
    }
    Ok(parts)
}

fn preparation_error() -> CloudTranscriptionError {
    CloudTranscriptionError::audio_preparation(
        "Could not prepare audio for Hosted Whisper. Check the audio format and available disk space.",
    )
}

fn scratch_error() -> CloudTranscriptionError {
    CloudTranscriptionError::audio_preparation(
        "Could not create temporary audio in the ClawScribe cache. Check available disk space and permissions.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::transcription::cloud::CloudTranscriptWord;
    use std::{cell::RefCell, future::ready};

    #[test]
    fn custom_uploads_above_the_raw_limit_are_prepared() {
        assert!(!should_prepare_audio(false, 500_000_000));
        assert!(should_prepare_audio(false, 500_000_001));
        assert!(!should_prepare_audio(true, 24_000_000));
        assert!(should_prepare_audio(true, 24_000_001));
    }

    #[test]
    fn scratch_parts_drop_after_use_and_only_abandoned_jobs_are_swept() {
        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path().join("hosted-whisper");
        let unrelated = directory.path().join("other-cache");
        std::fs::create_dir(&unrelated).unwrap();

        let finished = create_scratch_job(&cache).unwrap();
        let finished_path = finished.path().to_owned();
        std::fs::write(finished.path().join("part-000.mp3"), b"synthetic").unwrap();
        assert_eq!(finished.path().parent(), Some(cache.as_path()));
        drop(finished);
        assert!(!finished_path.exists());

        let abandoned = create_scratch_job(&cache).unwrap().keep();
        std::fs::write(abandoned.join("part-000.mp3"), b"synthetic").unwrap();
        let unrelated_entry = cache.join("unrelated");
        std::fs::create_dir(&unrelated_entry).unwrap();
        sweep_scratch_jobs(&cache).unwrap();
        assert!(!abandoned.exists());
        assert!(unrelated.exists());
        assert!(unrelated_entry.exists());
    }

    #[tokio::test]
    async fn uploads_parts_in_order_and_offsets_segments_and_words_across_silence() {
        let parts = parse_parts(
            Path::new("scratch"),
            "part-000.mp3,0,10\npart-001.mp3,10,20\npart-002.mp3,20,30\n",
        )
        .unwrap();
        let segment = |text: &str| CloudTranscriptSegment {
            text: text.to_string(),
            start_seconds: 0.5,
            end_seconds: 2.0,
            words: Some(vec![CloudTranscriptWord {
                text: text.to_string(),
                start_seconds: 0.75,
                end_seconds: 1.25,
            }]),
            requires_local_timing_grid: false,
        };
        let mut calls = Vec::new();
        let mut responses = [vec![segment("first")], vec![], vec![segment("last")]].into_iter();
        let progress = RefCell::new(Vec::new());
        let merged = upload_parts(
            &parts,
            |path| {
                calls.push(path.file_name().unwrap().to_owned());
                ready(Ok(responses.next().unwrap()))
            },
            &|percent, _| progress.borrow_mut().push(percent),
        )
        .await
        .unwrap();
        let segments =
            complete_transcript(merged, &|percent, _| progress.borrow_mut().push(percent)).unwrap();
        assert_eq!(calls, ["part-000.mp3", "part-001.mp3", "part-002.mp3"]);
        assert_eq!(*progress.borrow(), [0, 33, 66, 100]);
        assert_eq!(segments.len(), 2);
        assert_eq!(
            (segments[0].text.as_str(), segments[0].start_seconds),
            ("first", 0.5)
        );
        assert_eq!(
            (segments[1].text.as_str(), segments[1].start_seconds),
            ("last", 20.5)
        );
        assert_eq!(segments[1].end_seconds, 22.0);
        let word = &segments[1].words.as_ref().unwrap()[0];
        assert_eq!((word.start_seconds, word.end_seconds), (20.75, 21.25));

        let silent = upload_parts(&parts, |_| ready(Ok(Vec::new())), &|_, _| {})
            .await
            .unwrap();
        assert_eq!(
            complete_transcript(silent, &|_, _| {})
                .unwrap_err()
                .category(),
            super::super::CloudFallbackReasonCategory::ProviderOutput
        );
    }

    #[test]
    fn segment_and_word_offsets_use_the_encoder_cut_points() {
        let parts = parse_parts(
            Path::new("scratch"),
            "part-000.mp3,0.000000,6000.012000\npart-001.mp3,6000.012000,7200.000000\n",
        )
        .unwrap();
        let mut segments = vec![CloudTranscriptSegment {
            text: "short segment".into(),
            start_seconds: 0.5,
            end_seconds: 2.0,
            words: Some(vec![CloudTranscriptWord {
                text: "short".into(),
                start_seconds: 0.5,
                end_seconds: 0.75,
            }]),
            requires_local_timing_grid: false,
        }];
        parts[1].offset_segments(&mut segments);
        assert_eq!(segments[0].start_seconds, 6000.512);
        assert_eq!(segments[0].end_seconds, 6002.012);
        let word = &segments[0].words.as_ref().unwrap()[0];
        assert_eq!(word.start_seconds, 6000.512);
        assert_eq!(word.end_seconds, 6000.762);
    }

    #[tokio::test]
    #[ignore = "requires the app's FFmpeg runtime"]
    async fn ffmpeg_creates_compact_parts_and_reports_actual_boundaries() {
        use std::io::Write;

        let source = tempfile::tempdir().unwrap();
        let path = source.path().join("synthetic.wav");
        for (seconds, expected_parts) in [(20 * 60, 1), (2 * 60 * 60, 2)] {
            let pcm_bytes = seconds * 32_000;
            let mut file = std::fs::File::create(&path).unwrap();
            let header = super::super::pcm16_wav_header(pcm_bytes);
            file.write_all(&header).unwrap();
            file.set_len(header.len() as u64 + u64::from(pcm_bytes))
                .unwrap();
            drop(file);
            assert!(u64::from(pcm_bytes) > PREPARE_ABOVE_BYTES);

            let scratch = tempfile::tempdir().unwrap();
            let parts = prepare_parts(
                &path,
                scratch.path(),
                SEGMENT_SECONDS,
                &AtomicBool::new(false),
            )
            .await
            .unwrap();
            assert_eq!(parts.len(), expected_parts);
            assert_eq!(parts[0].start, 0.0);
            assert!((parts.last().unwrap().end - f64::from(seconds)).abs() < 0.15);
            if expected_parts == 2 {
                assert_eq!(parts[0].end, parts[1].start);
                assert!((parts[1].start - f64::from(SEGMENT_SECONDS)).abs() < 0.1);
            }
            for part in parts {
                let bytes = std::fs::metadata(part.path).unwrap().len();
                assert!((1..super::super::OPENAI_HOSTED_WHISPER_MAX_UPLOAD_BYTES).contains(&bytes));
            }
        }
    }
}
