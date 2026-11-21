//! Only user-authored template bodies, custom words, and preferences for
//! archived calendar series travel. Entire settings stores never leave the app.
use super::*;
use crate::audio::{vocabulary::normalize_custom_words, vocabulary_settings};
use crate::summary::templates::{custom_template_dirs, validate_id, Template, REMOVED_TEMPLATE};

const MAX_TEMPLATES: usize = 500;
const MAX_TEMPLATE_BYTES: u64 = 128 * 1024;

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Portable {
    #[serde(default)]
    templates: BTreeMap<String, Template>,
    #[serde(default)]
    words: Vec<String>,
    #[serde(default)]
    pub(super) series: Vec<SeriesPreference>,
}

#[derive(Serialize, Deserialize, sqlx::FromRow)]
#[serde(deny_unknown_fields)]
pub(super) struct SeriesPreference {
    source_id: String,
    series_id: String,
    template_id: String,
}

impl SeriesPreference {
    fn key(&self) -> (&str, &str) {
        (&self.source_id, &self.series_id)
    }
}

fn series_keys<'a>(
    meetings: impl Iterator<Item = &'a Map<String, Value>>,
) -> HashSet<(String, String)> {
    meetings
        .filter_map(|row| {
            let link =
                super::super::calendar::decode(row.get("calendar_link")?.as_str()?).ok()??;
            let (source, series) = link.series_key()?;
            Some((source.to_owned(), series.to_owned()))
        })
        .collect()
}

pub(super) async fn capture_series(
    connection: &mut sqlx::SqliteConnection,
    meetings: &[Map<String, Value>],
) -> Result<Vec<SeriesPreference>, String> {
    use futures_util::TryStreamExt;
    let keys = series_keys(meetings.iter());
    let mut rows = sqlx::query_as::<_, SeriesPreference>(
        "SELECT source_id, series_id, template_id FROM calendar_series_templates ORDER BY source_id, series_id",
    )
    .fetch(connection);
    let mut preferences = Vec::new();
    while let Some(row) = rows.try_next().await.map_err(failure)? {
        if keys.contains(&(row.source_id.clone(), row.series_id.clone())) {
            preferences.push(row);
        }
    }
    Ok(preferences)
}

impl Portable {
    pub(super) fn is_empty(&self) -> bool {
        self.templates.is_empty() && self.words.is_empty() && self.series.is_empty()
    }

    pub(super) fn validate(&self, meetings: &[Map<String, Value>]) -> Result<(), String> {
        if self.templates.len() > MAX_TEMPLATES
            || self.words.len() > 500
            || normalize_custom_words(self.words.clone())? != self.words
        {
            return Err("Invalid portable preferences.".into());
        }
        let mut identifiers = HashSet::new();
        for (id, template) in &self.templates {
            validate_id(id)?;
            if !identifiers.insert(id.to_lowercase()) {
                return Err("Duplicate template identifier in backup.".into());
            }
            template_bytes(template)?;
        }
        let keys = series_keys(meetings.iter());
        let mut seen = HashSet::new();
        for row in &self.series {
            validate_id(&row.template_id)?;
            if !seen.insert(row.key())
                || !keys.contains(&(row.source_id.clone(), row.series_id.clone()))
            {
                return Err("Invalid or duplicate archived calendar series.".into());
            }
        }
        Ok(())
    }

    pub(super) async fn restore_series(
        &self,
        connection: &mut sqlx::SqliteConnection,
        meetings: &[Map<String, Value>],
        selected: &HashSet<String>,
    ) -> Result<(), String> {
        let keys = series_keys(meetings.iter().filter(|row| {
            row.get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| selected.contains(id))
        }));
        for row in &self.series {
            if keys.contains(&(row.source_id.clone(), row.series_id.clone())) {
                sqlx::query(
                    "INSERT INTO calendar_series_templates (source_id, series_id, template_id)
                     VALUES (?, ?, ?) ON CONFLICT (source_id, series_id) DO NOTHING",
                )
                .bind(&row.source_id)
                .bind(&row.series_id)
                .bind(&row.template_id)
                .execute(&mut *connection)
                .await
                .map_err(failure)?;
            }
        }
        Ok(())
    }

    pub(super) fn capture_files(&mut self, app_data: &Path) -> Result<(), String> {
        self.capture_from(app_data, &custom_template_dirs())
    }

    fn capture_from(&mut self, app_data: &Path, directories: &[PathBuf]) -> Result<(), String> {
        let vocabulary = vocabulary_settings::portable_path(app_data);
        if let Some(metadata) = file_metadata(&vocabulary)? {
            if metadata.len() > vocabulary_settings::MAX_STORE_BYTES {
                return Err("Custom vocabulary is too large for backup.".into());
            }
            self.words = vocabulary_settings::read_saved_words(&vocabulary).0;
        }
        let mut seen = HashSet::new();
        for directory in directories {
            for entry in template_entries(directory)? {
                let (id, path) = entry?;
                if !seen.insert(id.to_lowercase()) {
                    continue;
                }
                let file = File::open(path).map_err(failure)?;
                let mut bytes = Vec::new();
                file.take(MAX_TEMPLATE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(failure)?;
                if bytes.len() as u64 > MAX_TEMPLATE_BYTES {
                    return Err("A custom template exceeds the 128 KiB template limit.".into());
                }
                // A deletion marker shadows the legacy copy under the same ID.
                if bytes == REMOVED_TEMPLATE.as_bytes() {
                    continue;
                }
                if self.templates.len() >= MAX_TEMPLATES {
                    return Err("There are too many custom templates for backup.".into());
                }
                let template = serde_json::from_slice::<Template>(&bytes).map_err(failure)?;
                template_bytes(&template)?;
                self.templates.insert(id, template);
            }
        }
        Ok(())
    }

    pub(super) fn publish(self, app_data: Option<&Path>) -> Result<Published, String> {
        if self.templates.is_empty() && self.words.is_empty() {
            return Ok(Published::default());
        }
        self.publish_into(
            app_data.ok_or("Portable preferences require an app data directory.")?,
            &custom_template_dirs(),
        )
    }

    fn publish_into(self, app_data: &Path, directories: &[PathBuf]) -> Result<Published, String> {
        let mut published = Published::default();
        if !self.templates.is_empty() {
            let target = directories
                .first()
                .ok_or("Template storage is unavailable.")?;
            let mut existing = HashSet::new();
            for directory in directories {
                for entry in template_entries(directory)? {
                    existing.insert(entry?.0.to_lowercase());
                }
            }
            for (id, template) in self.templates {
                validate_id(&id)?;
                if !existing.contains(&id.to_lowercase()) {
                    published.create(
                        &target.join(format!("{id}.json")),
                        &template_bytes(&template)?,
                    )?;
                }
            }
        }
        // An existing store, including an explicitly empty list, wins.
        if !self.words.is_empty() {
            let bytes =
                serde_json::to_vec(&serde_json::json!({ "words": self.words })).map_err(failure)?;
            published.create(&vocabulary_settings::portable_path(app_data), &bytes)?;
        }
        Ok(published)
    }
}

fn template_bytes(template: &Template) -> Result<Vec<u8>, String> {
    template.validate().map_err(failure)?;
    let bytes = serde_json::to_vec_pretty(template).map_err(failure)?;
    if bytes.len() as u64 > MAX_TEMPLATE_BYTES {
        return Err("A custom template exceeds the 128 KiB template limit.".into());
    }
    Ok(bytes)
}

fn file_metadata(path: &Path) -> Result<Option<std::fs::Metadata>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !crate::local_path::is_link(&metadata) => {
            Ok(Some(metadata))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        _ => Err("A portable settings file is unreadable or linked.".into()),
    }
}

fn template_entries(
    directory: &Path,
) -> Result<impl Iterator<Item = Result<(String, PathBuf), String>>, String> {
    let entries = match std::fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.is_dir() && !crate::local_path::is_link(&metadata) => {
            crate::local_path::require_unlinked_directory(directory)?;
            Some(std::fs::read_dir(directory).map_err(failure)?)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        _ => return Err("Template storage is unreadable or linked.".into()),
    };
    Ok(entries.into_iter().flatten().filter_map(|entry| {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => return Some(Err(failure(error))),
        };
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            return None;
        }
        let id = path.file_stem()?.to_str()?.to_owned();
        Some(validate_id(&id).and_then(|_| {
            file_metadata(&path)?.ok_or("A template changed during the archive operation.")?;
            Ok((id, path))
        }))
    }))
}

#[derive(Default)]
pub(super) struct Published(Vec<PathBuf>);

impl Published {
    fn create(&mut self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        let parent = path
            .parent()
            .ok_or("Portable settings storage is unavailable.")?;
        // Validate the closest existing ancestor before creating missing folders.
        let ancestor = parent
            .ancestors()
            .find(|p| std::fs::symlink_metadata(p).is_ok())
            .ok_or("Portable settings storage is unavailable.")?;
        crate::local_path::require_unlinked_directory(ancestor)?;
        std::fs::create_dir_all(parent).map_err(failure)?;
        crate::local_path::require_unlinked_directory(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(failure)?;
        file.write_all(bytes).map_err(failure)?;
        file.as_file().sync_all().map_err(failure)?;
        match file.persist_noclobber(path) {
            Ok(_) => self.0.push(path.to_owned()),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(failure(error)),
        }
        Ok(())
    }

    pub(super) fn keep(mut self) {
        self.0.clear();
    }
}

impl Drop for Published {
    fn drop(&mut self) {
        for path in self.0.drain(..) {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::backup::tests::empty_conversation_destination;

    #[test]
    fn custom_files_round_trip_and_keep_existing_preferences() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let source = source.path().canonicalize().unwrap();
        let target = target.path().canonicalize().unwrap();
        let source_templates = source.join("templates");
        let target_templates = target.join("templates");
        std::fs::create_dir(&source_templates).unwrap();
        std::fs::create_dir(&target_templates).unwrap();
        let standup = crate::summary::templates::get_template("daily_standup").unwrap();
        let body = template_bytes(&standup).unwrap();
        std::fs::write(source_templates.join("custom-sync.json"), &body).unwrap();
        std::fs::write(source_templates.join("custom-existing.json"), &body).unwrap();
        std::fs::write(
            source.join("custom_vocabulary.json"),
            br#"{"words":["Exampleterm"]}"#,
        )
        .unwrap();
        std::fs::write(source.join("backup-schedule.json"), b"host-local settings").unwrap();
        std::fs::write(
            target_templates.join("custom-existing.json"),
            b"destination template",
        )
        .unwrap();

        let mut preferences = Portable::default();
        preferences
            .capture_from(&source, &[source_templates])
            .unwrap();
        let bytes = serde_json::to_vec(&preferences).unwrap();
        let decoded: Portable = serde_json::from_slice(&bytes).unwrap();
        decoded.validate(&[]).unwrap();
        decoded
            .publish_into(&target, std::slice::from_ref(&target_templates))
            .unwrap()
            .keep();
        assert_eq!(
            std::fs::read(target_templates.join("custom-sync.json")).unwrap(),
            body
        );
        assert_eq!(
            std::fs::read(target_templates.join("custom-existing.json")).unwrap(),
            b"destination template"
        );
        assert_eq!(
            vocabulary_settings::read_saved_words(&target.join("custom_vocabulary.json")).0,
            vec!["Exampleterm"]
        );
        assert!(!target.join("backup-schedule.json").exists());

        std::fs::write(target.join("custom_vocabulary.json"), br#"{"words":[]}"#).unwrap();
        preferences
            .publish_into(&target, &[target_templates])
            .unwrap()
            .keep();
        assert!(
            vocabulary_settings::read_saved_words(&target.join("custom_vocabulary.json"))
                .0
                .is_empty()
        );
    }

    #[test]
    fn removed_personal_templates_stay_out_of_archives() {
        let root = tempfile::tempdir().unwrap();
        let personal = root.path().join("personal");
        let legacy = root.path().join("legacy");
        for directory in [&personal, &legacy] {
            std::fs::create_dir(directory).unwrap();
        }
        std::fs::write(personal.join("custom.json"), REMOVED_TEMPLATE).unwrap();
        std::fs::write(legacy.join("custom.json"), b"shadowed legacy copy").unwrap();
        let mut portable = Portable::default();
        portable
            .capture_from(root.path(), &[personal, legacy])
            .unwrap();
        assert!(portable.templates.is_empty());
    }

    #[tokio::test]
    async fn archived_series_apply_only_to_new_linked_meetings_and_keep_destination_choices() {
        let source = empty_conversation_destination().await;
        let target = empty_conversation_destination().await;
        let first = format!("graph:v1:{}", "a".repeat(64));
        let second = format!("graph:v1:{}", "b".repeat(64));
        let unrelated = format!("graph:v1:{}", "c".repeat(64));
        for series in [&first, &second, &unrelated] {
            sqlx::query(
                "INSERT INTO calendar_series_templates VALUES ('microsoft', ?, 'daily_standup')",
            )
            .bind(series)
            .execute(&source)
            .await
            .unwrap();
        }
        let meetings = [("first", &first), ("second", &second)].map(|(id, series)| {
            let calendar = serde_json::json!({"eventId": id, "seriesId": series}).to_string();
            serde_json::json!({"id": id, "calendar_link": calendar})
                .as_object()
                .unwrap()
                .clone()
        });
        let portable = Portable {
            series: capture_series(&mut *source.acquire().await.unwrap(), &meetings)
                .await
                .unwrap(),
            ..Portable::default()
        };
        assert_eq!(portable.series.len(), 2);
        portable.validate(&meetings).unwrap();
        sqlx::query(
            "INSERT INTO calendar_series_templates VALUES ('microsoft', ?, 'standard_meeting')",
        )
        .bind(&first)
        .execute(&target)
        .await
        .unwrap();
        let mut transaction = target.begin().await.unwrap();
        portable
            .restore_series(
                &mut transaction,
                &meetings,
                &HashSet::from(["first".into()]),
            )
            .await
            .unwrap();
        transaction.commit().await.unwrap();
        let only: String = sqlx::query_scalar(
            "SELECT template_id FROM calendar_series_templates WHERE series_id=?",
        )
        .bind(&first)
        .fetch_one(&target)
        .await
        .unwrap();
        assert_eq!(only, "standard_meeting");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM calendar_series_templates")
                .fetch_one(&target)
                .await
                .unwrap(),
            1
        );
        let mut transaction = target.begin().await.unwrap();
        portable
            .restore_series(
                &mut transaction,
                &meetings,
                &HashSet::from(["second".into()]),
            )
            .await
            .unwrap();
        transaction.commit().await.unwrap();
        let new: String = sqlx::query_scalar(
            "SELECT template_id FROM calendar_series_templates WHERE series_id=?",
        )
        .bind(&second)
        .fetch_one(&target)
        .await
        .unwrap();
        assert_eq!(new, "daily_standup");
    }
}
