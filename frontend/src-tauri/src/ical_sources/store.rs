use super::feed::{
    download, subscription_url, validate_calendar, without_unused_text, MAX_BYTES, TOO_LARGE,
};
use crate::{credentials, library::atomic_write};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, ErrorKind, Read},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};
use uuid::Uuid;

pub(super) const DUPLICATE: &str = "This calendar source has already been added.";
pub(super) const MISSING: &str = "This calendar source no longer exists.";
pub(super) const STORAGE: &str =
    "Could not read or save calendar sources. Check access to app storage.";
const PROTECTED: &str =
    "Could not access the protected calendar URL. Check your OS credential store.";
const NAME: &str = "Enter a calendar name of 1 to 80 characters.";
const LIMIT: &str = "You can add up to 32 calendar sources.";
const MAX_HEADER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SourceKind {
    File,
    Subscription,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CalendarSource {
    pub(super) id: Uuid,
    pub(super) name: String,
    pub(super) kind: SourceKind,
}

// Each source is published atomically as a JSON header followed by optional
// imported UTF-8 bytes. Listing sources reads only their headers.
#[derive(Serialize, Deserialize)]
struct Header {
    #[serde(flatten)]
    source: CalendarSource,
    fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    protected_url: Option<String>,
}

pub(super) fn validate_name(name: &str) -> Result<String, String> {
    let clean = name.trim();
    if clean.is_empty()
        || clean.chars().count() > 80
        || name.chars().any(char::is_control)
        || clean.contains("://")
    {
        return Err(NAME.into());
    }
    Ok(clean.into())
}

fn path_for(dir: &Path, id: Uuid) -> PathBuf {
    dir.join(format!("{id}.ical-source"))
}

fn open_source(dir: &Path, id: Uuid) -> Result<(Header, BufReader<File>), String> {
    let file = File::open(path_for(dir, id)).map_err(|e| {
        if e.kind() == ErrorKind::NotFound {
            MISSING
        } else {
            STORAGE
        }
    })?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    reader
        .by_ref()
        .take(MAX_HEADER_BYTES as u64 + 1)
        .read_until(b'\n', &mut line)
        .map_err(|_| STORAGE)?;
    if line.len() > MAX_HEADER_BYTES || line.last() != Some(&b'\n') {
        return Err(STORAGE.into());
    }
    let header: Header = serde_json::from_slice(&line).map_err(|_| STORAGE)?;
    if header.source.id != id || validate_name(&header.source.name).is_err() {
        return Err(STORAGE.into());
    }
    Ok((header, reader))
}

fn headers(dir: &Path) -> Result<Vec<Header>, String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(STORAGE.into()),
    };
    let mut result = Vec::new();
    for entry in entries {
        let path = entry.map_err(|_| STORAGE)?.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("ical-source") {
            let id = path
                .file_stem()
                .and_then(|stem| Uuid::parse_str(stem.to_str()?).ok())
                .ok_or(STORAGE)?;
            result.push(open_source(dir, id)?.0);
        }
    }
    Ok(result)
}

fn check_capacity(dir: &Path, kind: SourceKind, fingerprint: &str) -> Result<(), String> {
    let current = headers(dir)?;
    if current
        .iter()
        .any(|h| h.source.kind == kind && h.fingerprint == fingerprint)
    {
        return Err(DUPLICATE.into());
    }
    if current.len() >= 32 {
        return Err(LIMIT.into());
    }
    Ok(())
}

fn persist(dir: &Path, header: &Header, content: &str) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|_| STORAGE)?;
    let mut bytes = serde_json::to_vec(header).map_err(|_| STORAGE)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_HEADER_BYTES {
        return Err(STORAGE.into());
    }
    bytes.extend_from_slice(content.as_bytes());
    atomic_write(&path_for(dir, header.source.id), &bytes).map_err(|_| STORAGE.into())
}

fn credential_scope(id: Uuid) -> String {
    format!("calendar/ical/{id}")
}

#[derive(Default)]
struct FeedCache {
    recent: Option<(Instant, Result<String, String>)>,
}

impl FeedCache {
    fn get_or_fetch(
        &mut self,
        force: bool,
        fetch: impl FnOnce() -> Result<String, String>,
    ) -> Result<String, String> {
        if let Some((at, cached)) = &self.recent {
            let ttl = if cached.is_ok() { 60 } else { 10 };
            if !force && at.elapsed() < Duration::from_secs(ttl) {
                return cached.clone();
            }
        }
        let fetched = fetch();
        self.recent = Some((Instant::now(), fetched.clone()));
        fetched
    }
}

type CacheMap = HashMap<Uuid, Arc<Mutex<FeedCache>>>;

// All commands run on blocking workers. Serialize local mutations, but allow
// unrelated feeds to download concurrently and settings to remain responsive.
#[derive(Default)]
pub(crate) struct CalendarStore {
    cache: Mutex<CacheMap>,
}

impl CalendarStore {
    fn lock(&self) -> Result<MutexGuard<'_, CacheMap>, String> {
        self.cache.lock().map_err(|_| STORAGE.into())
    }

    pub(super) fn list(&self, dir: &Path) -> Result<Vec<CalendarSource>, String> {
        let _guard = self.lock()?;
        let mut sources: Vec<_> = headers(dir)?.into_iter().map(|h| h.source).collect();
        sources.sort_by_cached_key(|s| (s.name.to_lowercase(), s.id));
        Ok(sources)
    }

    pub(super) fn add_file(
        &self,
        dir: &Path,
        name: &str,
        content: String,
    ) -> Result<CalendarSource, String> {
        let name = validate_name(name)?;
        validate_calendar(&content)?;
        let fingerprint = format!("{:x}", Sha256::digest(content.as_bytes()));
        let source = CalendarSource {
            id: Uuid::new_v4(),
            name,
            kind: SourceKind::File,
        };
        let _guard = self.lock()?;
        check_capacity(dir, source.kind, &fingerprint)?;
        persist(
            dir,
            &Header {
                source: source.clone(),
                fingerprint,
                protected_url: None,
            },
            &without_unused_text(&content),
        )?;
        Ok(source)
    }

    pub(super) fn add_subscription(
        &self,
        dir: &Path,
        name: &str,
        address: &str,
    ) -> Result<CalendarSource, String> {
        let url = subscription_url(address)?;
        let name = validate_name(name)?;
        let kind = SourceKind::Subscription;
        let fingerprint = format!("{:x}", Sha256::digest(url.as_str().as_bytes()));
        {
            let _guard = self.lock()?;
            check_capacity(dir, kind, &fingerprint)?;
        }
        let calendar = download(url.clone())?;
        let source = CalendarSource {
            id: Uuid::new_v4(),
            name,
            kind,
        };
        let mut cache = self.lock()?;
        check_capacity(dir, kind, &fingerprint)?;
        let scope = credential_scope(source.id);
        let protected = credentials::seal(&scope, url.as_str()).map_err(|_| PROTECTED)?;
        let header = Header {
            source: source.clone(),
            fingerprint,
            protected_url: Some(protected.clone()),
        };
        if let Err(error) = persist(dir, &header, "") {
            credentials::forget(&scope, &protected).map_err(|_| PROTECTED)?;
            return Err(error);
        }
        cache.insert(
            source.id,
            Arc::new(Mutex::new(FeedCache {
                recent: Some((Instant::now(), Ok(calendar))),
            })),
        );
        Ok(source)
    }

    pub(super) fn read(&self, dir: &Path, id: Uuid, force: bool) -> Result<String, String> {
        let (protected, slot) = {
            let mut cache = self.lock()?;
            let (header, reader) = open_source(dir, id)?;
            if header.source.kind == SourceKind::File {
                let mut bytes = Vec::new();
                reader
                    .take(MAX_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| STORAGE)?;
                if bytes.len() > MAX_BYTES {
                    return Err(TOO_LARGE.into());
                }
                let content =
                    String::from_utf8(bytes).map_err(|_| super::feed::INVALID_CALENDAR)?;
                validate_calendar(&content)?;
                return Ok(content);
            }
            (
                header.protected_url.ok_or(PROTECTED)?,
                cache.entry(id).or_default().clone(),
            )
        };
        let mut feed = slot.lock().map_err(|_| STORAGE)?;
        if !self.lock()?.contains_key(&id) {
            return Err(MISSING.into());
        }
        let content = feed.get_or_fetch(force, || {
            let address =
                credentials::open(&credential_scope(id), &protected).map_err(|_| PROTECTED)?;
            download(subscription_url(&address)?)
        });
        if !self.lock()?.contains_key(&id) {
            return Err(MISSING.into());
        }
        content
    }

    pub(super) fn remove(&self, dir: &Path, id: Uuid) -> Result<(), String> {
        let mut cache = self.lock()?;
        let (header, _) = open_source(dir, id)?;
        // Forget first. An interrupted removal can safely retry the keyring
        // deletion (including after restart) before unlinking the record.
        if let Some(protected) = header.protected_url {
            credentials::forget(&credential_scope(id), &protected).map_err(|_| PROTECTED)?;
        }
        fs::remove_file(path_for(dir, id)).map_err(|_| STORAGE)?;
        cache.remove(&id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validated_subscription_uses_memory_cache_and_force_reopens_credential() {
        let dir = tempfile::tempdir().unwrap();
        let id = Uuid::new_v4();
        let header = Header {
            source: CalendarSource {
                id,
                name: "Project".into(),
                kind: SourceKind::Subscription,
            },
            fingerprint: "synthetic fingerprint".into(),
            protected_url: Some("clawscribe:secret:v1:dpapi:synthetic".into()),
        };
        persist(dir.path(), &header, "").unwrap();
        let store = CalendarStore::default();
        let sample = "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n";
        store.lock().unwrap().insert(
            id,
            Arc::new(Mutex::new(FeedCache {
                recent: Some((Instant::now(), Ok(sample.into()))),
            })),
        );
        assert_eq!(store.read(dir.path(), id, false).unwrap(), sample);
        // Invalid synthetic encrypted bytes cannot be opened on any platform.
        assert_eq!(store.read(dir.path(), id, true).unwrap_err(), PROTECTED);
        assert!(!fs::read_to_string(path_for(dir.path(), id))
            .unwrap()
            .contains(sample));
        assert_eq!(
            serde_json::to_value(store.list(dir.path()).unwrap()).unwrap(),
            serde_json::json!([
                {"id": id, "name": "Project", "kind": "subscription"}
            ])
        );
        store.remove(dir.path(), id).unwrap();
        assert!(store.lock().unwrap().is_empty());
        assert!(CalendarStore::default()
            .list(dir.path())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn expired_results_and_manual_refresh_download_again() {
        let mut cache = FeedCache::default();
        assert_eq!(
            cache.get_or_fetch(false, || Ok("first".into())).unwrap(),
            "first"
        );
        assert_eq!(
            cache
                .get_or_fetch(false, || panic!("unexpected download"))
                .unwrap(),
            "first"
        );
        assert_eq!(
            cache.get_or_fetch(true, || Ok("second".into())).unwrap(),
            "second"
        );
        cache.recent.as_mut().unwrap().0 = Instant::now() - Duration::from_secs(61);
        assert_eq!(
            cache
                .get_or_fetch(false, || Err("offline".into()))
                .unwrap_err(),
            "offline"
        );
        assert_eq!(
            cache
                .get_or_fetch(false, || panic!("unexpected retry"))
                .unwrap_err(),
            "offline"
        );
        assert_eq!(
            cache
                .get_or_fetch(true, || Ok("back online".into()))
                .unwrap(),
            "back online"
        );
    }
}
