//! Channel selection stays local; downloads and signature verification remain
//! owned by Tauri's updater. Never accept an endpoint supplied by the webview.
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{sync::Mutex, time::Duration};
use tauri::{AppHandle, Manager, ResourceId, Runtime, Webview};
use tauri_plugin_store::StoreExt;
use tauri_plugin_updater::UpdaterExt;

const RELEASES_API: &str = "https://api.github.com/repos/ch3573r/ClawScribe/releases?per_page=100";
const DOWNLOAD_ROOT: &str = "https://github.com/ch3573r/ClawScribe/releases/download/";
const MAX_RELEASE_BYTES: usize = 4 * 1024 * 1024;
static PREFERENCE_LOCK: Mutex<()> = Mutex::new(());

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
    tag: String,
    prerelease: bool,
    manifest_url: String,
    installer_url: String,
}

fn runtime_version(tag: &str) -> Option<Version> {
    let version = Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()?;
    // Windows ProductVersion cannot carry a prerelease suffix. Equal numeric
    // versions are never successive updates, even across release channels.
    Some(Version::new(version.major, version.minor, version.patch))
}

fn newer_runtime(current: &Version, available: &Version) -> bool {
    Version::new(available.major, available.minor, available.patch)
        > Version::new(current.major, current.minor, current.patch)
}

fn newest_candidate(releases: Vec<GithubRelease>, floor: &Version) -> Option<Candidate> {
    releases
        .into_iter()
        .filter_map(|release| {
            if release.draft {
                return None;
            }
            let version = runtime_version(&release.tag_name)?;
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
            .unwrap_or_else(|| {
                let version = &app.package_info().version;
                Version::new(version.major, version.minor, version.patch)
            });
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
            // Use the plugin's actual Update resource so the official JS API
            // downloads this exact release and verifies the pinned public key.
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
