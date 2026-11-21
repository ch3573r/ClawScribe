//! Follow-through records with a provable owner in the archived library.
use super::*;
use crate::knowledge::live_notes::is_portable_live_answer;
use futures_util::TryStreamExt;
use sqlx::SqliteConnection;

#[cfg(test)]
pub(super) const TABLES: &[&str] = &[
    "approved_series_notes",
    "recurring_meeting_preferences",
    "saved_live_answers",
];
const INVALID: &str = "Invalid meeting follow-through data in archive.";
const MAX_ANSWERS_PER_MEETING: usize = 32;
const MAX_ANSWER_JSON_BYTES: usize = 512 * 1024;

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Portable {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    notes: Vec<ApprovedNote>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    series: Vec<SeriesPreference>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    answers: Vec<SavedAnswer>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    omitted_answers: BTreeMap<String, u32>,
}

#[derive(Serialize, Deserialize, sqlx::FromRow)]
#[serde(deny_unknown_fields)]
struct ApprovedNote {
    meeting_id: String,
    source_id: String,
    series_id: String,
    event_id: String,
    approved_notes: String,
}

#[derive(Serialize, Deserialize, sqlx::FromRow)]
#[serde(deny_unknown_fields)]
struct SeriesPreference {
    source_id: String,
    series_id: String,
    summary_language: Option<String>,
    include_approved_notes: bool,
    destination: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedAnswer {
    id: String,
    meeting_id: String,
    recording_id: String,
    saved_at: String,
    answer: Value,
}

fn linked_series<'a>(
    meetings: impl Iterator<Item = &'a Map<String, Value>>,
) -> HashMap<&'a str, super::super::calendar::CalendarLink> {
    meetings
        .filter_map(|row| {
            let id = row.get("id")?.as_str()?;
            let link =
                super::super::calendar::decode(row.get("calendar_link")?.as_str()?).ok()??;
            link.series_key()?;
            Some((id, link))
        })
        .collect()
}

impl ApprovedNote {
    fn matches(&self, link: &super::super::calendar::CalendarLink) -> bool {
        link.reviewed_series_key() == Some((self.source_id.as_str(), self.series_id.as_str()))
            && link.event_id() == self.event_id
            && !self.approved_notes.is_empty()
            && self.approved_notes.trim() == self.approved_notes
            && self.approved_notes.chars().count() <= 2_000
            && !self.approved_notes.contains('\0')
    }
}

impl SeriesPreference {
    fn key(&self) -> (&str, &str) {
        (&self.source_id, &self.series_id)
    }

    fn valid(&self) -> bool {
        self.summary_language.as_deref().is_none_or(|code| {
            code.len() <= 64
                && code
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_ ".contains(&b))
                && crate::summary::processor::language_name_from_code(code).is_some()
        }) && matches!(
            self.destination.as_deref(),
            None | Some("planner" | "todo" | "email")
        )
    }
}

impl SavedAnswer {
    fn valid(&self) -> bool {
        self.recording_id == self.meeting_id
            && chrono::NaiveDateTime::parse_from_str(&self.saved_at, "%Y-%m-%d %H:%M:%S").is_ok()
            && is_portable_live_answer(&self.id, &self.answer)
    }
}

fn charge(budget: &mut u64, row: &impl Serialize) -> Result<(), String> {
    let size = serde_json::to_vec(row).map_err(failure)?.len();
    *budget = budget
        .checked_sub(size as u64)
        .ok_or("Meeting metadata exceeds the 128 MiB archive limit.")?;
    Ok(())
}

impl Portable {
    pub(super) fn is_empty(&self) -> bool {
        self.notes.is_empty()
            && self.series.is_empty()
            && self.answers.is_empty()
            && self.omitted_answers.is_empty()
    }

    pub(super) async fn capture(
        connection: &mut SqliteConnection,
        meetings: &[Map<String, Value>],
        budget: &mut u64,
        scheduled: Option<&ScheduledBackup>,
    ) -> Result<Self, String> {
        backup_checkpoint(scheduled)?;
        let mut portable = Self::default();
        let links = linked_series(meetings.iter());
        let series: HashSet<_> = links
            .values()
            .filter_map(|link| link.series_key())
            .collect();
        {
            let mut rows = sqlx::query_as::<_, ApprovedNote>(
                "SELECT meeting_id, source_id, series_id, event_id, approved_notes
                 FROM approved_series_notes WHERE length(CAST(approved_notes AS BLOB)) <= 8000
                 ORDER BY meeting_id",
            )
            .fetch(&mut *connection);
            while let Some(note) = rows.try_next().await.map_err(failure)? {
                backup_checkpoint(scheduled)?;
                if links
                    .get(note.meeting_id.as_str())
                    .is_some_and(|link| note.matches(link))
                {
                    charge(budget, &note)?;
                    portable.notes.push(note);
                }
            }
        }
        {
            let mut rows = sqlx::query_as::<_, SeriesPreference>(
                "SELECT source_id, series_id, summary_language, include_approved_notes, destination
                 FROM recurring_meeting_preferences ORDER BY source_id, series_id",
            )
            .fetch(&mut *connection);
            while let Some(preference) = rows.try_next().await.map_err(failure)? {
                backup_checkpoint(scheduled)?;
                if series.contains(&preference.key()) && preference.valid() {
                    charge(budget, &preference)?;
                    portable.series.push(preference);
                }
            }
        }
        // Project only eligible bodies. An absent dependency list for an older
        // answer proves nothing about the sources that went into its prompt.
        let mut rows = sqlx::query_as::<_, (String, String, String, String, Option<String>)>(
            "SELECT a.id, a.meeting_id, a.recording_id, a.saved_at,
                    CASE WHEN a.recording_id = a.meeting_id
                         AND a.source_dependencies_complete = 1
                         AND NOT EXISTS (SELECT 1 FROM saved_live_answer_dependencies d WHERE d.answer_id = a.id)
                         AND length(CAST(a.answer AS BLOB)) <= ? THEN a.answer END
             FROM saved_live_answers a JOIN meetings m ON m.id = a.meeting_id
             ORDER BY a.meeting_id, a.saved_at, a.id",
        )
        .bind(MAX_ANSWER_JSON_BYTES as i64)
        .fetch(connection);
        let mut counts = HashMap::<String, usize>::new();
        while let Some((id, meeting_id, recording_id, saved_at, raw)) =
            rows.try_next().await.map_err(failure)?
        {
            backup_checkpoint(scheduled)?;
            let count = counts.entry(meeting_id.clone()).or_default();
            if *count < MAX_ANSWERS_PER_MEETING {
                if let Some(answer) = raw.and_then(|raw| serde_json::from_str(&raw).ok()) {
                    let row = SavedAnswer {
                        id,
                        meeting_id: meeting_id.clone(),
                        recording_id,
                        saved_at,
                        answer,
                    };
                    if row.valid() {
                        charge(budget, &row)?;
                        portable.answers.push(row);
                        *count += 1;
                        continue;
                    }
                }
            }
            *portable.omitted_answers.entry(meeting_id).or_default() += 1;
        }
        if !portable.omitted_answers.is_empty() {
            charge(budget, &portable.omitted_answers)?;
        }
        Ok(portable)
    }

    pub(super) fn validate(
        &self,
        manifest: &Manifest,
        scheduled: Option<&ScheduledBackup>,
    ) -> Result<(), String> {
        backup_checkpoint(scheduled)?;
        if self.is_empty() {
            return Ok(());
        }
        if manifest.version < 3 {
            return Err(INVALID.into());
        }
        let meetings = &manifest.tables["meetings"];
        let ids: HashSet<_> = meetings
            .iter()
            .filter_map(|row| row.get("id")?.as_str())
            .collect();
        let links = linked_series(meetings.iter());
        let series: HashSet<_> = links
            .values()
            .filter_map(|link| link.series_key())
            .collect();
        let mut seen = HashSet::new();
        for note in &self.notes {
            backup_checkpoint(scheduled)?;
            if !seen.insert(note.meeting_id.as_str())
                || !links
                    .get(note.meeting_id.as_str())
                    .is_some_and(|link| note.matches(link))
            {
                return Err(INVALID.into());
            }
        }
        let mut seen = HashSet::new();
        for preference in &self.series {
            backup_checkpoint(scheduled)?;
            if !seen.insert(preference.key())
                || !series.contains(&preference.key())
                || !preference.valid()
            {
                return Err(INVALID.into());
            }
        }
        let mut seen = HashSet::new();
        let mut counts = HashMap::<&str, usize>::new();
        for answer in &self.answers {
            backup_checkpoint(scheduled)?;
            let count = counts.entry(&answer.meeting_id).or_default();
            *count += 1;
            if *count > MAX_ANSWERS_PER_MEETING
                || !seen.insert(answer.id.as_str())
                || !ids.contains(answer.meeting_id.as_str())
                || !answer.valid()
            {
                return Err(INVALID.into());
            }
        }
        if self
            .omitted_answers
            .iter()
            .any(|(id, count)| *count == 0 || !ids.contains(id.as_str()))
        {
            return Err(INVALID.into());
        }
        Ok(())
    }

    pub(super) fn omitted_answers(&self, selected: Option<&HashSet<String>>) -> u64 {
        self.omitted_answers
            .iter()
            .filter(|(id, _)| selected.is_none_or(|selected| selected.contains(*id)))
            .map(|(_, count)| u64::from(*count))
            .sum()
    }

    pub(super) async fn restore(
        &self,
        connection: &mut SqliteConnection,
        meetings: &[Map<String, Value>],
        selected: &HashSet<String>,
    ) -> Result<u64, String> {
        let mut omitted = self.omitted_answers(Some(selected));
        for row in self
            .answers
            .iter()
            .filter(|row| selected.contains(&row.meeting_id))
        {
            let result = sqlx::query(
                "INSERT INTO saved_live_answers (id, meeting_id, recording_id, saved_at, answer, source_dependencies_complete)
                 VALUES (?, ?, ?, ?, ?, 1) ON CONFLICT(id) DO NOTHING",
            )
            .bind(&row.id).bind(&row.meeting_id).bind(&row.recording_id).bind(&row.saved_at)
            .bind(serde_json::to_string(&row.answer).map_err(failure)?)
            .execute(&mut *connection).await.map_err(failure)?;
            if result.rows_affected() == 0 {
                omitted += 1;
            }
        }
        for row in self
            .notes
            .iter()
            .filter(|row| selected.contains(&row.meeting_id))
        {
            sqlx::query(
                "INSERT INTO approved_series_notes (meeting_id, source_id, series_id, event_id, approved_notes)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&row.meeting_id).bind(&row.source_id).bind(&row.series_id).bind(&row.event_id)
            .bind(&row.approved_notes).execute(&mut *connection).await.map_err(failure)?;
        }
        let links = linked_series(meetings.iter().filter(|row| {
            row.get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| selected.contains(id))
        }));
        let series: HashSet<_> = links
            .values()
            .filter_map(|link| link.series_key())
            .collect();
        for row in self.series.iter().filter(|row| series.contains(&row.key())) {
            sqlx::query(
                "INSERT INTO recurring_meeting_preferences (source_id, series_id, summary_language, include_approved_notes, destination)
                 VALUES (?, ?, ?, ?, ?) ON CONFLICT(source_id, series_id) DO NOTHING",
            )
            .bind(&row.source_id).bind(&row.series_id).bind(&row.summary_language)
            .bind(row.include_approved_notes).bind(&row.destination)
            .execute(&mut *connection).await.map_err(failure)?;
        }
        Ok(omitted)
    }
}
