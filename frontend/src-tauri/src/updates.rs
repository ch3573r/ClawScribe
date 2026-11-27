//! Channel selection stays local; downloads and signature verification remain
//! owned by Tauri's updater. Never accept an endpoint supplied by the webview.
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{sync::Mutex, time::Duration};
use tauri::{ipc::Channel, AppHandle, Manager, ResourceId, Runtime, Webview};
use tauri_plugin_store::StoreExt;
use tauri_plugin_updater::{Update, UpdaterExt};

const RELEASES_API: &str = "https://api.github.com/repos/ch3573r/ClawScribe/releases?per_page=100";
const DOWNLOAD_ROOT: &str = "https://github.com/ch3573r/ClawScribe/releases/download/";
const MAX_RELEASE_BYTES: usize = 4 * 1024 * 1024;
static PREFERENCE_LOCK: Mutex<()> = Mutex::new(());

// Queued imports, summaries, saved answers and outbound deliveries can run beside
// each other and beside audio work. They share this gate; installation needs
// the exclusive side plus the native job slot (recording, transcription, import,
// library backup). Rust owns every permit below. Nothing is parked in a
// webview resource, so a page reload can neither strand nor release one.
static WORK_INSTALL_GATE: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());
const BUSY_MESSAGE: &str = "Wait for recording, transcription, imports, summaries, answers, or exports to finish before installing.";

/// Admission for durable or outbound work that an installer restart would cut off.
pub(crate) fn admit_work() -> Result<tokio::sync::RwLockReadGuard<'static, ()>, String> {
    WORK_INSTALL_GATE
        .try_read()
        .map_err(|_| "ClawScribe is installing an update. Try again after it restarts.".into())
}

struct UpdateInstallPermit {
    _work: tokio::sync::RwLockWriteGuard<'static, ()>,
    _native: tokio::sync::OwnedSemaphorePermit,
}

impl UpdateInstallPermit {
    fn acquire() -> Result<Self, String> {
        let work = WORK_INSTALL_GATE.try_write().map_err(|_| BUSY_MESSAGE)?;
        let native = crate::audio::inference::claim_job().map_err(|_| BUSY_MESSAGE)?;
        Ok(Self {
            _work: work,
            _native: native,
        })
    }
}

// The installer is bounded in memory and tied to the exact checked release.
const MAX_INSTALLER_BYTES: u64 = 1024 * 1024 * 1024;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
// Staged bytes are cleared if unused for this long; a newer download resets the timer.
const STAGED_TTL: Duration = Duration::from_secs(30 * 60);

struct StagedUpdate {
    rid: ResourceId,
    update: Update,
    bytes: Vec<u8>,
}

// The generation covers the download in flight as well as its retained bytes.
struct UpdateStage<T> {
    generation: u64,
    installer: Option<T>,
}

impl<T> UpdateStage<T> {
    const fn new() -> Self {
        Self {
            generation: 0,
            installer: None,
        }
    }

    fn invalidate(&mut self) -> u64 {
        self.installer = None;
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    fn publish(&mut self, generation: u64, installer: T) -> Result<(), String> {
        if generation != self.generation {
            return Err("The selected update changed. Check for updates again.".into());
        }
        self.installer = Some(installer);
        Ok(())
    }

    fn expire(&mut self, generation: u64) {
        if self.generation == generation {
            self.installer = None;
        }
    }
}

static STAGED: Mutex<UpdateStage<StagedUpdate>> = Mutex::new(UpdateStage::new());
static DOWNLOAD_FLIGHT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn staged() -> std::sync::MutexGuard<'static, UpdateStage<StagedUpdate>> {
    STAGED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn over_installer_cap(content_length: Option<u64>, received: u64) -> bool {
    content_length.is_some_and(|length| length > MAX_INSTALLER_BYTES)
        || received > MAX_INSTALLER_BYTES
}

fn bytes_exceed_install_cap(len: usize) -> bool {
    len as u64 > MAX_INSTALLER_BYTES
}

fn installer_url_allowed(url: &str) -> bool {
    url.starts_with(DOWNLOAD_ROOT)
}

fn install_error(error: &tauri_plugin_updater::Error) -> String {
    use tauri_plugin_updater::Error;
    match error {
        Error::Io(io) if io.kind() == std::io::ErrorKind::PermissionDenied || io.raw_os_error() == Some(5) => {
            "Windows blocked the installer. Ask your administrator to review the device security policy.".into()
        }
        Error::Minisign(_) | Error::Base64(_) | Error::SignatureUtf8(_) => {
            "The update signature could not be verified. Try again later.".into()
        }
        Error::Reqwest(_) | Error::Network(_) => {
            "The update could not be downloaded. Check your connection and try again.".into()
        }
        _ => "The update could not be installed. Try again.".into(),
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum DownloadEvent {
    #[serde(rename_all = "camelCase")]
    Started { content_length: Option<u64> },
    #[serde(rename_all = "camelCase")]
    Progress { chunk_length: usize },
}

/// Downloads and signature-verifies the checked update without blocking other
/// work. The verified bytes stay in Rust until [`install_app_update`].
#[tauri::command]
pub async fn download_app_update<R: Runtime>(
    webview: Webview<R>,
    rid: ResourceId,
    on_event: Channel<DownloadEvent>,
) -> Result<(), String> {
    let _flight = DOWNLOAD_FLIGHT
        .try_lock()
        .map_err(|_| "An update download is already running. Wait for it to finish.")?;
    // A temporarily busy install can be retried against the same checked release.
    let generation = {
        let mut stage = staged();
        if stage.installer.as_ref().is_some_and(|item| item.rid == rid) {
            return Ok(());
        }
        stage.invalidate()
    };
    // Fail early instead of downloading a large installer that cannot be used.
    drop(UpdateInstallPermit::acquire()?);
    let update = webview
        .resources_table()
        .get::<Update>(rid)
        .map_err(|_| "The selected update is no longer available. Check for updates again.")?;
    if !installer_url_allowed(update.download_url.as_str()) {
        return Err("The release metadata does not match its installer. Try again after the release is corrected.".into());
    }
    let mut update = (*update).clone();
    update.timeout = Some(DOWNLOAD_TIMEOUT);

    let over = std::sync::Arc::new(tokio::sync::Notify::new());
    let signal = over.clone();
    let (mut received, mut first) = (0u64, true);
    let download = update.download(
        move |chunk_length, content_length| {
            if first {
                first = false;
                let _ = on_event.send(DownloadEvent::Started { content_length });
            }
            received = received.saturating_add(chunk_length as u64);
            if over_installer_cap(content_length, received) {
                signal.notify_one();
                return;
            }
            let _ = on_event.send(DownloadEvent::Progress { chunk_length });
        },
        || {},
    );
    // Dropping the download future stops reading and frees its buffer.
    let bytes = tokio::select! {
        result = download => result.map_err(|error| install_error(&error))?,
        _ = over.notified() => return Err("The update download is larger than expected and was stopped.".into()),
    };
    if bytes_exceed_install_cap(bytes.len()) {
        return Err("The update download is larger than expected and was stopped.".into());
    }
    staged().publish(generation, StagedUpdate { rid, update, bytes })?;
    // Expire the staged bytes after STAGED_TTL so the heap cannot hold 1 GiB
    // indefinitely after a page reload. The generation key ensures a stale timer
    // does not discard bytes from a newer download.
    tokio::spawn(async move {
        tokio::time::sleep(STAGED_TTL).await;
        staged().expire(generation);
    });
    Ok(())
}

/// Installs the exact bytes staged for `rid`. The permit lives on this task, so
/// it covers the whole installer hand-off, survives a page reload, and is
/// released when the installer fails; on success the installer ends the process.
#[tauri::command]
pub async fn install_app_update(
    rid: ResourceId,
    edits: tauri::State<'_, crate::summary::edit_exit::SummaryEditsState>,
) -> Result<(), String> {
    let permit = UpdateInstallPermit::acquire()?;
    edits.ensure_saved_for_update()?;
    let staged = staged()
        .installer
        .take_if(|installer| installer.rid == rid)
        .ok_or("The downloaded update is no longer available. Check for updates again.")?;
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        staged
            .update
            .install(&staged.bytes)
            .map_err(|error| install_error(&error))
    })
    .await
    .map_err(|_| "The installer stopped unexpectedly. Try again.".to_string())?
}

#[tauri::command]
pub fn discard_app_update(rid: ResourceId) {
    // The webview may stop before hand-off if saving meeting edits fails.
    let _ = staged().installer.take_if(|installer| installer.rid == rid);
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Preview,
}

fn saved_channel(value: Option<serde_json::Value>) -> UpdateChannel {
    value
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

#[tauri::command]
pub fn get_update_channel<R: Runtime>(app: AppHandle<R>) -> Result<UpdateChannel, String> {
    let _guard = PREFERENCE_LOCK
        .lock()
        .map_err(|_| "Update preference is unavailable.")?;
    let store = app
        .store_builder("updater.json")
        .disable_auto_save()
        .build()
        .map_err(|_| "Could not read update preferences.")?;
    Ok(saved_channel(store.get("channel")))
}

#[tauri::command]
pub fn set_update_channel<R: Runtime>(
    app: AppHandle<R>,
    channel: UpdateChannel,
) -> Result<(), String> {
    let _guard = PREFERENCE_LOCK
        .lock()
        .map_err(|_| "Update preference is unavailable.")?;
    let store = app
        .store_builder("updater.json")
        .disable_auto_save()
        .build()
        .map_err(|_| "Could not read update preferences.")?;
    let previous = store.get("channel");
    store.set("channel", serde_json::json!(channel));
    if store.save().is_err() {
        if let Some(value) = previous {
            store.set("channel", value);
        } else {
            store.delete("channel");
        }
        return Err("Could not save the update channel. Your previous choice is unchanged.".into());
    }
    staged().invalidate();
    Ok(())
}

#[derive(Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    state: String,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug)]
struct Candidate {
    version: Version,
    tag_version: Version,
    tag: String,
    prerelease: bool,
    manifest_url: String,
    installer_url: String,
}

fn parse_release_tag(tag: &str) -> Option<Version> {
    Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()
}

fn numeric_version(version: &Version) -> Version {
    // Windows ProductVersion cannot carry a prerelease suffix. Equal numeric
    // versions are never successive updates, even across release channels.
    Version::new(version.major, version.minor, version.patch)
}

fn runtime_version(tag: &str) -> Option<Version> {
    parse_release_tag(tag).map(|version| numeric_version(&version))
}

fn newer_runtime(current: &Version, available: &Version) -> bool {
    numeric_version(available) > numeric_version(current)
}

fn newest_candidate(releases: Vec<GithubRelease>, floor: &Version) -> Option<Candidate> {
    releases
        .into_iter()
        .filter_map(|release| {
            if release.draft {
                return None;
            }
            let tag_version = parse_release_tag(&release.tag_name)?;
            let version = numeric_version(&tag_version);
            if version <= *floor {
                return None;
            }
            let root = format!("{DOWNLOAD_ROOT}{}/", release.tag_name);
            let manifest_url = format!("{root}latest.json");
            let installer_name = format!("ClawScribe_{version}_x64-setup.exe");
            let installer_url = format!("{root}{installer_name}");
            let has_asset = |name: &str, url: &str| {
                release.assets.iter().any(|asset| {
                    asset.name == name
                        && asset.browser_download_url == url
                        && asset.size > 0
                        && asset.state == "uploaded"
                })
            };
            if !has_asset("latest.json", &manifest_url)
                || !has_asset(&installer_name, &installer_url)
            {
                return None;
            }
            Some(Candidate {
                version,
                tag_version,
                tag: release.tag_name,
                prerelease: release.prerelease,
                manifest_url,
                installer_url,
            })
        })
        .max_by(|a, b| {
            a.version
                .cmp(&b.version)
                .then_with(|| b.prerelease.cmp(&a.prerelease))
                .then_with(|| a.tag_version.cmp_precedence(&b.tag_version))
                .then_with(|| a.tag.cmp(&b.tag))
        })
}

async fn preview_releases() -> Result<Vec<GithubRelease>, String> {
    let client = reqwest::Client::builder()
        .user_agent("ClawScribe-Updater")
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize update discovery.")?;
    let mut response = client
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|_| "Could not reach preview releases. Check your connection and try again.")?;
    if response.status() == reqwest::StatusCode::FORBIDDEN
        || response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        return Err(
            "GitHub limited preview update checks. Try again later or select stable releases."
                .into(),
        );
    }
    if !response.status().is_success() {
        return Err("Preview releases are unavailable. Try again later.".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Could not read preview releases.")?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_RELEASE_BYTES {
            return Err("The preview release listing is too large.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| "GitHub returned an invalid preview release listing.".into())
}

fn validate_candidate(
    candidate: &Candidate,
    version: &str,
    url: &str,
    signature: &str,
) -> Result<(), String> {
    if version != candidate.version.to_string()
        || url != candidate.installer_url
        || signature.trim().is_empty()
    {
        return Err("The release metadata does not match its installer. Try again after the release is corrected.".into());
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMetadata {
    rid: ResourceId,
    current_version: String,
    version: String,
    date: Option<String>,
    body: Option<String>,
    raw_json: serde_json::Value,
    prerelease: bool,
}

#[derive(Serialize)]
pub struct UpdateCheck {
    channel: UpdateChannel,
    update: Option<UpdateMetadata>,
}

#[tauri::command]
pub async fn check_app_update<R: Runtime>(webview: Webview<R>) -> Result<UpdateCheck, String> {
    staged().invalidate();
    let app = webview.app_handle();
    let channel = get_update_channel(app.clone())?;
    let builder = webview
        .updater_builder()
        .timeout(Duration::from_secs(25))
        .version_comparator(|current, release| newer_runtime(&current, &release.version));
    let mut update = builder
        .build()
        .map_err(|_| "Could not initialize the updater.")?
        .check()
        .await
        .map_err(|_| "Could not check stable updates. Check your connection and try again.")?;
    let mut prerelease = false;

    if channel == UpdateChannel::Preview {
        let floor = update
            .as_ref()
            .and_then(|update| runtime_version(&update.version))
            .unwrap_or_else(|| numeric_version(&app.package_info().version));
        if let Some(candidate) = newest_candidate(preview_releases().await?, &floor) {
            let endpoint = candidate
                .manifest_url
                .parse()
                .map_err(|_| "Invalid release address.")?;
            let preview = webview
                .updater_builder()
                .timeout(Duration::from_secs(25))
                .version_comparator(|current, release| newer_runtime(&current, &release.version))
                .endpoints(vec![endpoint])
                .map_err(|_| "Invalid release address.")?
                .build()
                .map_err(|_| "Could not initialize the preview updater.")?
                .check()
                .await
                .map_err(|_| "Could not verify the preview release metadata. Try again later.")?
                .ok_or("The preview release version does not match its tag.")?;
            validate_candidate(
                &candidate,
                &preview.version,
                preview.download_url.as_str(),
                &preview.signature,
            )?;
            prerelease = candidate.prerelease;
            update = Some(preview);
        }
    }
    if get_update_channel(app.clone())? != channel {
        return Err("The update channel changed. Check for updates again.".into());
    }
    let update = update.map(|update| {
        let metadata = UpdateMetadata {
            current_version: update.current_version.clone(),
            version: update.version.clone(),
            date: update
                .raw_json
                .get("pub_date")
                .and_then(|value| value.as_str())
                .map(str::to_owned),
            body: update.body.clone(),
            raw_json: update.raw_json.clone(),
            prerelease,
            // Keep the plugin's Update resource in Rust; the caller passes this
            // rid back to download_app_update and install_app_update.
            rid: webview.resources_table().add(update),
        };
        metadata
    });
    Ok(UpdateCheck { channel, update })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn install_excludes_summary_and_native_jobs_until_released() {
        use crate::audio::inference::{claim_job, GLOBAL_JOB_TEST_LOCK};
        use crate::summary::SummaryService;

        let _serial = GLOBAL_JOB_TEST_LOCK.lock().await;
        let meeting = "update-admission";
        let summary = SummaryService::register_job(meeting, "openai").unwrap();
        assert!(UpdateInstallPermit::acquire().is_err());
        drop(summary);

        let recording = claim_job().unwrap();
        assert!(UpdateInstallPermit::acquire().is_err());
        assert!(SummaryService::register_job(meeting, "openai").is_ok());
        drop(recording);

        let update = UpdateInstallPermit::acquire().unwrap();
        assert!(SummaryService::register_job(meeting, "openai").is_err());
        assert!(claim_job().is_err());
        drop(update);
        assert!(claim_job().is_ok());
        assert!(SummaryService::register_job(meeting, "openai").is_ok());
    }

    #[test]
    fn post_download_bytes_cap_uses_same_boundary_as_streaming_cap() {
        // Both caps must draw the line at exactly MAX_INSTALLER_BYTES bytes.
        // Test through the production helpers so the boundary lives in one place.
        assert!(!bytes_exceed_install_cap(MAX_INSTALLER_BYTES as usize));
        assert!(bytes_exceed_install_cap(MAX_INSTALLER_BYTES as usize + 1));
        assert!(!over_installer_cap(None, MAX_INSTALLER_BYTES));
        assert!(over_installer_cap(None, MAX_INSTALLER_BYTES + 1));
    }

    #[test]
    fn generation_expiry_does_not_clear_a_newer_download() {
        let mut slot = UpdateStage::new();
        let first = slot.invalidate();
        let current = slot.invalidate();
        assert!(slot.publish(first, vec![1]).is_err());
        slot.publish(current, vec![2]).unwrap();
        slot.expire(first);
        assert_eq!(slot.installer, Some(vec![2]));
        slot.expire(current);
        assert!(slot.installer.is_none());
    }

    #[test]
    fn installer_download_is_bounded_and_limited_to_release_assets() {
        assert!(!over_installer_cap(
            Some(MAX_INSTALLER_BYTES),
            MAX_INSTALLER_BYTES
        ));
        assert!(over_installer_cap(Some(MAX_INSTALLER_BYTES + 1), 0));
        assert!(over_installer_cap(None, MAX_INSTALLER_BYTES + 1));
        assert!(installer_url_allowed(&format!(
            "{DOWNLOAD_ROOT}v1.2.3/ClawScribe_1.2.3_x64-setup.exe"
        )));
        assert!(!installer_url_allowed(
            "https://example.com/ClawScribe_1.2.3_x64-setup.exe"
        ));
        assert!(!installer_url_allowed(&format!(
            "{}x",
            DOWNLOAD_ROOT.trim_end_matches('/')
        )));
    }

    #[tokio::test]
    async fn work_admission_waits_for_install_permit_and_never_outlives_it() {
        use crate::audio::inference::GLOBAL_JOB_TEST_LOCK;

        let _serial = GLOBAL_JOB_TEST_LOCK.lock().await;
        let work = admit_work().unwrap();
        assert!(UpdateInstallPermit::acquire().is_err());
        drop(work);
        // A failed installer drops its permit, so no job is stranded.
        let permit = UpdateInstallPermit::acquire().unwrap();
        assert!(admit_work().is_err());
        drop(permit);
        assert!(admit_work().is_ok());
        assert!(UpdateInstallPermit::acquire().is_ok());
    }

    fn release(tag: &str, prerelease: bool) -> GithubRelease {
        let version = runtime_version(tag).unwrap();
        let installer = format!("ClawScribe_{version}_x64-setup.exe");
        let assets = ["latest.json", &installer].map(|name| {
            json!({
                "name": name, "browser_download_url": format!("{DOWNLOAD_ROOT}{tag}/{name}"),
                "size": 100, "state": "uploaded"
            })
        });
        serde_json::from_value(json!({
            "tag_name": tag, "prerelease": prerelease, "draft": false,
            "assets": assets
        }))
        .unwrap()
    }

    #[test]
    fn preview_requires_explicit_valid_preference() {
        for value in [
            None,
            Some(json!(true)),
            Some(json!("beta")),
            Some(json!("stable")),
        ] {
            assert_eq!(saved_channel(value), UpdateChannel::Stable);
        }
        assert_eq!(
            saved_channel(Some(json!("preview"))),
            UpdateChannel::Preview
        );
        assert!(serde_json::from_value::<UpdateChannel>(json!("unknown")).is_err());
    }

    #[test]
    fn selects_newest_runtime_version_instead_of_feed_or_lexical_order() {
        let candidate = newest_candidate(
            vec![
                release("v0.5.9", true),
                release("v0.5.10", true),
                release("v0.5.8", false),
            ],
            &Version::new(0, 5, 7),
        )
        .unwrap();
        assert_eq!(candidate.version, Version::new(0, 5, 10));
        assert!(candidate.prerelease);
    }

    #[test]
    fn includes_newer_stable_and_prefers_stable_for_equal_runtime_versions() {
        let candidate = newest_candidate(
            vec![release("v0.5.10-beta.1", true), release("v0.5.10", false)],
            &Version::new(0, 5, 9),
        )
        .unwrap();
        assert!(!candidate.prerelease);
        assert_eq!(candidate.tag, "v0.5.10");
    }

    #[test]
    fn compares_preview_numbers_for_equal_runtime_versions() {
        let candidate = newest_candidate(
            vec![
                release("v0.68.2-preview.10", true),
                release("v0.68.2-preview.9", true),
            ],
            &Version::new(0, 68, 1),
        )
        .unwrap();
        assert_eq!(candidate.tag, "v0.68.2-preview.10");
        assert_eq!(candidate.version, Version::new(0, 68, 2));
    }

    #[test]
    fn never_offers_downgrades_or_equal_numeric_prerelease_versions() {
        assert!(!newer_runtime(
            &Version::parse("0.5.10-beta.1").unwrap(),
            &Version::parse("0.5.10").unwrap()
        ));
        assert!(newest_candidate(
            vec![
                release("v0.5.9", false),
                release("v0.5.10-beta.2", true),
                release("v0.5.10", false)
            ],
            &Version::new(0, 5, 10)
        )
        .is_none());
    }

    #[test]
    fn skips_drafts_incomplete_uploads_and_foreign_assets() {
        let mut draft = release("v0.5.15", true);
        draft.draft = true;
        let mut incomplete = release("v0.5.14", true);
        incomplete.assets.pop();
        let mut empty = release("v0.5.13", true);
        empty.assets[0].size = 0;
        let mut uploading = release("v0.5.12", true);
        uploading.assets[1].state = "new".into();
        let mut foreign = release("v0.5.11", true);
        foreign.assets[0].browser_download_url = "https://example.com/latest.json".into();
        let candidate = newest_candidate(
            vec![
                draft,
                incomplete,
                empty,
                uploading,
                foreign,
                release("v0.5.10", true),
            ],
            &Version::new(0, 5, 9),
        )
        .unwrap();
        assert_eq!(candidate.version, Version::new(0, 5, 10));
    }

    #[test]
    fn rejects_mismatched_manifests_and_missing_signatures() {
        let candidate =
            newest_candidate(vec![release("v0.5.10", true)], &Version::new(0, 5, 9)).unwrap();
        assert!(
            validate_candidate(&candidate, "0.5.10", &candidate.installer_url, "signed").is_ok()
        );
        assert!(
            validate_candidate(&candidate, "0.5.11", &candidate.installer_url, "signed").is_err()
        );
        assert!(validate_candidate(
            &candidate,
            "0.5.10",
            "https://example.com/setup.exe",
            "signed"
        )
        .is_err());
        assert!(validate_candidate(&candidate, "0.5.10", &candidate.installer_url, " ").is_err());
    }

    #[test]
    fn rejects_invalid_tags_and_path_components() {
        for tag in [
            "nightly",
            "v0.5",
            "v0.5.10/other",
            "v0.5.10?query=1",
            "v0.5.10#fragment",
        ] {
            assert!(runtime_version(tag).is_none());
        }
    }

    #[tokio::test]
    #[ignore = "Reads the public GitHub release feed and updater manifest"]
    async fn public_preview_feed_and_manifest_are_compatible() {
        let candidate = newest_candidate(preview_releases().await.unwrap(), &Version::new(0, 0, 0))
            .expect("A published Windows release must be discoverable");
        let manifest: tauri_plugin_updater::RemoteRelease = reqwest::Client::builder()
            .timeout(Duration::from_secs(25))
            .build()
            .unwrap()
            .get(&candidate.manifest_url)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        validate_candidate(
            &candidate,
            &manifest.version.to_string(),
            manifest.download_url("windows-x86_64").unwrap().as_str(),
            manifest.signature("windows-x86_64").unwrap(),
        )
        .unwrap();
    }
}
