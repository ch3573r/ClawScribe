//! A single Upcoming invitation, held only in memory from the click to admission.
use super::{sync_prompt, tracker, Mode, Tracker, JOIN_STATE_CHANGED};
use crate::audio::recording_commands::{capture_active, is_starting, recording_epoch};
use crate::exports::ms_auth_state::MicrosoftAuthState;
use crate::library::calendar::{CalendarLink, CalendarSource};
use crate::teams_detection::calls::{meet_room, scan, teams_room, CallScan, CallWindow, Provider};
use chrono::Utc;
use serde::Serialize;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Runtime};

const WAIT_MS: i64 = 5 * 60_000;
const EXPIRED: &str = "The invitation to record is no longer active.";

pub(super) struct Invitation {
    pub(super) id: String,
    pub(super) link: CalendarLink,
    pub(super) provider: Provider,
    room: [u8; 32],
    deadline: Instant,
    expires_at: i64,
    epoch: u64,
    near_start: bool,
    // None until the pre-launch scan finishes and the launcher succeeds.
    active_before: Option<Vec<String>>,
    open_before: Vec<String>,
    // Dropping the invitation also wakes and releases its timeout worker.
    timeout: Option<tokio::sync::oneshot::Sender<()>>,
    /// Only observations newer than the accepted pre-launch baseline belong to this invitation.
    pub(super) baseline_serial: u64,
}

impl Invitation {
    fn new(
        link: CalendarLink,
        provider: Provider,
        room: [u8; 32],
        end: &str,
    ) -> Result<Self, String> {
        let now = Utc::now().timestamp_millis();
        let start = link
            .start_time()
            .ok_or("The event start time is unavailable.")?;
        let end = chrono::DateTime::parse_from_rfc3339(end)
            .map_err(|_| "The event end time is unavailable.")?
            .timestamp_millis();
        if end <= now || start >= end || link.is_reviewed() {
            return Err("Choose a current meeting invitation from Upcoming.".into());
        }
        let wait = (end - now).min(WAIT_MS);
        Ok(Self {
            id: uuid::Uuid::new_v4().to_string(),
            link,
            provider,
            room,
            expires_at: now + wait,
            deadline: Instant::now() + Duration::from_millis(wait as u64),
            epoch: recording_epoch(),
            near_start: start <= now + WAIT_MS,
            active_before: None,
            open_before: Vec::new(),
            timeout: None,
            baseline_serial: 0,
        })
    }

    pub(super) fn current(&self) -> bool {
        Instant::now() < self.deadline
            && Utc::now().timestamp_millis() < self.expires_at
            && self.epoch == recording_epoch()
    }

    pub(super) fn mode(&self, call: &CallWindow) -> Mode {
        let Some(before) = &self.active_before else {
            return Mode::Off;
        };
        if !self.current() || call.provider != self.provider || before.contains(&call.window) {
            return Mode::Off;
        }
        match call.room_identity {
            Some(room) if room != self.room => Mode::Off,
            None if self.provider != Provider::Teams => Mode::Off,
            // An already-open window may have been showing a call that Windows
            // UI Automation could not inspect before the invitation opened.
            Some(_) if self.near_start && !self.open_before.contains(&call.window) => Mode::Auto,
            _ => Mode::Prompt,
        }
    }

    pub(super) fn armed(&self) -> bool {
        self.active_before.is_some() && self.current()
    }

    fn launch(
        &mut self,
        scan: Option<&CallScan>,
        open: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        if !self.current() || self.active_before.is_some() {
            return Err(EXPIRED.into());
        }
        open()?;
        let scan = scan.filter(|s| s.supported && s.complete).ok_or(
            "The meeting link opened. ClawScribe could not check existing calls. Start recording manually.",
        )?;
        self.active_before = Some(scan.active.iter().map(|call| call.window.clone()).collect());
        self.open_before = scan.open_windows.clone();
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinView {
    id: String,
    subject: String,
    opening: bool,
    prompt_key: Option<String>,
}

impl Tracker {
    fn reserve_join(&mut self, join: Invitation) -> Result<(), String> {
        if self.join.as_ref().is_some_and(Invitation::current) {
            return Err("Cancel the pending recording before joining another meeting.".into());
        }
        if !join.current() {
            return Err(EXPIRED.into());
        }
        self.prompt = None;
        self.join = Some(join);
        self.revision += 1;
        Ok(())
    }

    fn launch_join(
        &mut self,
        id: &str,
        baseline: Option<&CallScan>,
        open: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let join = self
            .join
            .as_mut()
            .filter(|join| join.id == id)
            .ok_or(EXPIRED)?;
        let result = join.launch(baseline, open);
        if result.is_ok() {
            join.baseline_serial = self.seen.last_serial;
        }
        self.revision += 1; // Reject any tick begun before the launch completed.
        result
    }

    pub(super) fn clear_join(&mut self, id: &str) -> bool {
        if self.join.as_ref().is_none_or(|join| join.id != id) {
            return false;
        }
        self.join = None;
        self.prompt = None;
        self.revision += 1;
        true
    }
}

#[tauri::command]
pub fn get_join_recording() -> Option<JoinView> {
    let tracker = tracker();
    let join = tracker.join.as_ref().filter(|join| join.current())?;
    Some(JoinView {
        id: join.id.clone(),
        subject: join.link.subject().unwrap_or("Untitled event").into(),
        opening: join.active_before.is_none(),
        prompt_key: tracker.prompt.as_ref().map(|p| p.key.clone()),
    })
}

#[tauri::command]
pub fn cancel_join_recording<R: Runtime>(app: AppHandle<R>, id: String) -> bool {
    cancel(&app, &id)
}

pub(super) fn cancel<R: Runtime>(app: &AppHandle<R>, id: &str) -> bool {
    let before = {
        let mut tracker = tracker();
        let before = tracker.prompt_view();
        if !tracker.clear_join(id) {
            return false;
        }
        before
    };
    sync_prompt(app, before, None);
    JOIN_STATE_CHANGED.notify_one();
    let _ = app.emit("join-recording-changed", ());
    true
}

pub(crate) fn cancel_pending<R: Runtime>(app: &AppHandle<R>) {
    let id = tracker().join.as_ref().map(|j| j.id.clone());
    if let Some(id) = id {
        cancel(app, &id);
    }
}

pub(crate) fn cancel_microsoft_pending<R: Runtime>(app: &AppHandle<R>) {
    let id = tracker()
        .join
        .as_ref()
        .filter(|join| join.link.source() == CalendarSource::Microsoft)
        .map(|join| join.id.clone());
    if let Some(id) = id {
        cancel(app, &id);
    }
}

fn invitation_matches_ical_source(invitation: &Invitation, source: uuid::Uuid) -> bool {
    if invitation.link.source() != CalendarSource::Ical {
        return false;
    }
    // The serialized link exposes the source for both single and recurring events.
    serde_json::to_value(&invitation.link)
        .ok()
        .and_then(|link| uuid::Uuid::parse_str(link.get("sourceId")?.as_str()?).ok())
        == Some(source)
}

pub(crate) fn cancel_ical_pending<R: Runtime>(app: &AppHandle<R>, source: uuid::Uuid) {
    let id = tracker()
        .join
        .as_ref()
        .filter(|join| invitation_matches_ical_source(join, source))
        .map(|join| join.id.clone());
    if let Some(id) = id {
        cancel(app, &id);
    }
}

fn prepare_invitation(
    url: &str,
    link: CalendarLink,
    end: &str,
) -> Result<(url::Url, Invitation), String> {
    let raw = url;
    if raw.len() > 4096
        || raw
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
    {
        return Err("Invalid meeting link.".into());
    }
    let mut url = url::Url::parse(raw).map_err(|_| "Invalid meeting link.")?;
    let authority_and_path = raw.split_once("://").map(|(_, rest)| rest);
    let exact_meet_link = authority_and_path
        .and_then(|rest| rest.split(['?', '#']).next())
        .and_then(|rest| rest.split_once('/'))
        .is_some_and(|(host, path)| {
            host.eq_ignore_ascii_case("meet.google.com")
                && url.path().strip_prefix('/') == Some(path)
        });
    let (provider, room) = if let Some(room) = teams_room(&url) {
        (Provider::Teams, room)
    } else if let Some(room) = exact_meet_link.then(|| meet_room(&url)).flatten() {
        (Provider::GoogleMeet, room)
    } else {
        return Err("Unsupported meeting link.".into());
    };
    url.set_fragment(None);
    let invitation = Invitation::new(link.normalized()?, provider, room, end)?;
    Ok((url, invitation))
}

#[tauri::command]
pub async fn join_and_record<R: Runtime>(
    app: AppHandle<R>,
    url: String,
    link: CalendarLink,
    end: String,
) -> Result<(), String> {
    let (url, mut join) = prepare_invitation(&url, link, &end)?;
    let id = join.id.clone();
    let deadline = join.deadline;
    let provider = join.provider;
    let (timeout, removed) = tokio::sync::oneshot::channel();
    join.timeout = Some(timeout);
    if !cfg!(windows) {
        return Err("Join and record requires Windows.".into());
    }
    let before = {
        let auth = app.state::<MicrosoftAuthState>();
        let _session = if join.link.source() == CalendarSource::Microsoft {
            let session = auth.inner.read().await;
            if session.current_token.is_none() {
                return Err("Microsoft calendar is disconnected.".into());
            }
            Some(session)
        } else {
            None
        };
        let mut tracker = tracker();
        if capture_active() || is_starting() || crate::start_request::is_pending() {
            return Err("Join and record requires an idle recorder.".into());
        }
        let before = tracker.prompt_view();
        tracker.reserve_join(join)?;
        before
    };
    sync_prompt(&app, before, None);
    let _ = app.emit("join-recording-changed", ());
    let timer_app = app.clone();
    let timer_id = id.clone();
    tauri::async_runtime::spawn(async move {
        if tokio::time::timeout_at(deadline.into(), removed)
            .await
            .is_err()
            && cancel(&timer_app, &timer_id)
        {
            let _ = timer_app.emit(
                "recording-start-feedback",
                "The request to record the meeting expired.",
            );
        }
    });
    if !crate::tray::check_can_record(&app).await {
        cancel(&app, &id);
        return Err("Complete recording setup before using Join and record.".into());
    }
    let baseline = scan(vec![provider], vec![]).await.ok();
    let launched = {
        let mut tracker = tracker();
        // Cancellation and sign-out serialize with the actual OS launch.
        if capture_active() || is_starting() || crate::start_request::is_pending() {
            Err(EXPIRED.into())
        } else {
            tracker.launch_join(&id, baseline.as_ref(), || {
                crate::external_url::open_url_in_default_browser(url.as_str())
            })
        }
    };
    if launched.is_err() {
        cancel(&app, &id);
    } else {
        JOIN_STATE_CHANGED.notify_one();
        let _ = app.emit("join-recording-changed", ());
    }
    launched
}

#[cfg(test)]
mod tests {
    use super::super::{CallSettings, Context, Decision};
    use super::*;

    #[test]
    fn removing_an_ical_feed_matches_only_its_own_pending_invitation() {
        let feed = uuid::Uuid::new_v4();
        let other = uuid::Uuid::new_v4();
        let now = Utc::now();
        let end = (now + chrono::Duration::hours(1)).to_rfc3339();
        for (source, expected) in [("ical", true), ("microsoft", false)] {
            let link: CalendarLink = serde_json::from_value(serde_json::json!({
                "source": source, "sourceId": feed, "eventId": "single-event",
                "start": now.to_rfc3339(),
            }))
            .unwrap();
            let (_, invitation) =
                prepare_invitation("https://teams.microsoft.com/meet/123456", link, &end).unwrap();
            assert_eq!(invitation_matches_ical_source(&invitation, feed), expected);
            assert!(!invitation_matches_ical_source(&invitation, other));
        }
    }

    const IDLE: Context = Context {
        idle: true,
        recording: false,
    };

    #[test]
    fn both_calendar_sources_can_prepare_only_allowlisted_teams_links() {
        let now = Utc::now();
        let end = (now + chrono::Duration::hours(1)).to_rfc3339();
        for (source, source_id) in [("microsoft", None), ("ical", Some("calendar-feed"))] {
            let link: CalendarLink = serde_json::from_value(serde_json::json!({
                "source": source, "sourceId": source_id, "eventId": "selected",
                "start": now.to_rfc3339(), "attendees": [{ "name": "Invitee" }]
            }))
            .unwrap();
            for url in [
                "https://teams.microsoft.com/meet/123456",
                "https://teams.cloud.microsoft/l/meetup-join/19%3ameeting_example%40thread.v2/0",
                "https://teams.live.com/meet/123456",
            ] {
                let (accepted, invitation) = prepare_invitation(url, link.clone(), &end).unwrap();
                assert_eq!(accepted.as_str(), url);
                assert_eq!(invitation.link.source(), link.source());
                assert!(!invitation.link.is_reviewed());
                assert_eq!(invitation.room, teams_room(&accepted).unwrap());
            }
            for url in [
                "not a URL",
                "https://meet.google.com/abc-defg-hij/extra",
                "https://sub.meet.google.com/abc-defg-hij",
                "https://@meet.google.com/abc-defg-hij",
                "https://meet.google.com:443/abc-defg-hij",
                "https://meet.google.com:444/abc-defg-hij",
                "https://meet.google.com/unrelated/../abc-defg-hij",
                "https://zoom.us/j/123456789",
                "https://teams.microsoft.com.example.com/meet/123456",
                "http://teams.microsoft.com/meet/123456",
                "https://user@teams.microsoft.com/meet/123456",
                "https://teams.microsoft.com:444/meet/123456",
                "https://teams.microsoft.com/",
                "https://teams.microsoft.com/l/meetup-join/thread%2fother/0",
            ] {
                assert!(
                    prepare_invitation(url, link.clone(), &end).is_err(),
                    "{url}"
                );
            }
        }
    }

    #[test]
    fn ical_still_requires_a_valid_source_and_current_unreviewed_invitation() {
        let now = Utc::now();
        let end = (now + chrono::Duration::hours(1)).to_rfc3339();
        let url = "https://teams.microsoft.com/meet/123456";
        for (source_id, reviewed, event_end) in [
            (None, false, end.as_str()),
            (Some("calendar-feed"), true, end.as_str()),
            (Some("calendar-feed"), false, "2020-01-01T00:00:00Z"),
        ] {
            let link = serde_json::from_value(serde_json::json!({
                "source": "ical", "sourceId": source_id, "eventId": "selected",
                "start": now.to_rfc3339(), "reviewed": reviewed,
            }))
            .unwrap();
            assert!(prepare_invitation(url, link, event_end).is_err());
        }
    }

    fn invitation() -> Invitation {
        let now = Utc::now();
        let link = serde_json::from_value(serde_json::json!({
            "eventId": "selected", "subject": "Planning", "start": now.to_rfc3339(),
            "attendees": [{ "name": "Invitee" }]
        }))
        .unwrap();
        Invitation::new(
            link,
            Provider::Teams,
            [1; 32],
            &(now + chrono::Duration::hours(1)).to_rfc3339(),
        )
        .unwrap()
    }

    fn call(window: &str, room: Option<[u8; 32]>) -> CallWindow {
        CallWindow {
            provider: Provider::Teams,
            key: window.into(),
            window: window.into(),
            room_identity: room,
            desktop_teams: room.is_none(),
        }
    }

    fn snapshot(active: Vec<CallWindow>) -> CallScan {
        CallScan {
            supported: true,
            complete: true,
            active,
            ..Default::default()
        }
    }

    #[test]
    fn an_existing_window_that_first_becomes_readable_after_launch_requires_confirmation() {
        let mut join = invitation();
        let baseline = CallScan {
            open_windows: vec!["existing".into()],
            ..snapshot(vec![])
        };
        join.launch(Some(&baseline), || Ok(())).unwrap();
        assert!(matches!(
            join.mode(&call("existing", Some([1; 32]))),
            Mode::Prompt
        ));
        assert!(matches!(join.mode(&call("new", Some([1; 32]))), Mode::Auto));
    }

    #[test]
    fn meet_requires_the_invited_room_and_accelerates_only_after_launch() {
        let mut tracker = Tracker::new(CallSettings::default());
        let source = invitation();
        let end = (Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        let (_, meet) = prepare_invitation(
            "https://meet.google.com/abc-defg-hij?authuser=0",
            source.link,
            &end,
        )
        .unwrap();
        let id = meet.id.clone();
        let mut exact = call("new browser", Some(meet.room));
        exact.provider = Provider::GoogleMeet;
        let wrong = CallWindow {
            room_identity: Some([7; 32]),
            ..exact.clone()
        };
        let unknown = CallWindow {
            room_identity: None,
            ..exact.clone()
        };
        let other_provider = CallWindow {
            provider: Provider::Zoom,
            ..exact.clone()
        };
        assert_eq!(tracker.poll_interval(), super::super::POLL_INTERVAL);
        tracker.reserve_join(meet).unwrap();
        assert_eq!(tracker.providers(), [Provider::GoogleMeet]);
        assert_eq!(tracker.poll_interval(), super::super::POLL_INTERVAL);
        tracker
            .launch_join(&id, Some(&snapshot(vec![])), || Ok(()))
            .unwrap();
        assert_eq!(tracker.poll_interval(), super::super::JOIN_POLL_INTERVAL);
        for call in [wrong, unknown, other_provider] {
            assert_eq!(tracker.mode_for(&call), Mode::Off);
        }
        assert_eq!(tracker.mode_for(&exact), Mode::Auto);
        tracker.clear_join(&id);
        assert_eq!(tracker.poll_interval(), super::super::POLL_INTERVAL);
    }

    #[test]
    fn only_a_fresh_matching_call_can_auto_start_with_monitoring_off() {
        let mut tracker = Tracker::new(CallSettings::default());
        let old = call("already active", Some([1; 32]));
        let exact = call("new browser", Some([1; 32]));
        let wrong = call("another room", Some([2; 32]));
        let before = tracker.revision();
        let join = invitation();
        let id = join.id.clone();
        tracker.reserve_join(join).unwrap();
        let opening = tracker.revision();
        let one = snapshot(vec![exact.clone()]);
        assert_eq!(
            tracker.observe(opening, Some(&one), IDLE),
            Decision::Nothing
        );
        tracker
            .launch_join(&id, Some(&snapshot(vec![old.clone()])), || Ok(()))
            .unwrap();
        let armed = tracker.revision();
        for revision in [before, opening] {
            assert_eq!(
                tracker.observe(revision, Some(&one), IDLE),
                Decision::Nothing
            );
        }
        for scan in [
            snapshot(vec![old]),
            snapshot(vec![wrong.clone()]),
            snapshot(vec![exact.clone(), wrong]),
            CallScan {
                complete: false,
                ..one.clone()
            },
        ] {
            assert_eq!(tracker.observe(armed, Some(&scan), IDLE), Decision::Nothing);
        }
        let Decision::Start { serial, .. } = tracker.observe(armed, Some(&one), IDLE) else {
            panic!("missing exact match")
        };
        assert!(tracker.may_begin(&exact, serial, armed, true, false));
        assert_eq!(tracker.observe(armed, Some(&one), IDLE), Decision::Nothing);
        assert!(tracker.clear_join(&id));
        assert!(!tracker.may_begin(&exact, serial, armed, true, false));
        assert!(!tracker.watching());
        assert_eq!(tracker.settings(), &CallSettings::default());
    }

    #[test]
    fn ambiguous_identity_needs_confirmation_bound_to_this_observation_and_invitation() {
        let mut tracker = Tracker::new(CallSettings {
            mode: Mode::Auto,
            ..Default::default()
        });
        let joined = invitation();
        let id = joined.id.clone();
        let unknown = call("desktop", None);
        let scan = snapshot(vec![unknown.clone()]);
        tracker.reserve_join(joined).unwrap();
        tracker
            .launch_join(&id, Some(&snapshot(vec![])), || Ok(()))
            .unwrap();
        let revision = tracker.revision();
        assert_eq!(
            tracker.observe(revision, Some(&scan), IDLE),
            Decision::Prompt
        );
        let old = tracker.prompt_view().unwrap();
        assert_eq!(old.event_title.as_deref(), Some("Planning"));
        tracker.observe(revision, None, IDLE);
        assert!(tracker.confirm(&old.key).is_none());
        assert_eq!(
            tracker.observe(revision, Some(&scan), IDLE),
            Decision::Prompt
        );
        assert!(tracker.confirm(&old.key).is_none());
        let key = tracker.prompt_view().unwrap().key;
        let (call, serial, revision) = tracker.confirm(&key).unwrap();
        assert!(!tracker.may_begin(&call, serial, revision, true, false));
        assert!(tracker.may_begin(&call, serial, revision, true, true));
        tracker.join.as_mut().unwrap().deadline = Instant::now();
        assert!(!tracker.may_begin(&call, serial, revision, true, true));
        assert!(tracker.clear_join(&id));
        tracker.reserve_join(invitation()).unwrap();
        assert!(!tracker.may_begin(&call, serial, revision, true, true));
    }

    #[test]
    fn repeated_click_stale_timer_cancellation_and_launcher_failures_cannot_arm_another_event() {
        let mut tracker = Tracker::new(CallSettings::default());
        let first = invitation();
        let id = first.id.clone();
        tracker.reserve_join(first).unwrap();
        assert!(tracker.reserve_join(invitation()).is_err());
        assert!(tracker.clear_join(&id));
        let replacement = invitation();
        let next = replacement.id.clone();
        tracker.reserve_join(replacement).unwrap();
        assert!(!tracker.clear_join(&id));
        let baseline = snapshot(vec![]);
        assert!(tracker
            .launch_join(&id, Some(&baseline), || panic!("stale launch"))
            .is_err());
        assert!(tracker
            .launch_join(
                &next,
                Some(&baseline),
                || Err("launcher unavailable".into())
            )
            .is_err());
        assert!(tracker.join.as_ref().unwrap().active_before.is_none());
        assert!(tracker.launch_join(&next, None, || Ok(())).is_err());
        assert!(tracker.join.as_ref().unwrap().active_before.is_none());
        tracker.join.as_mut().unwrap().deadline = Instant::now();
        assert!(tracker
            .launch_join(&next, Some(&baseline), || panic!("expired launch"))
            .is_err());
    }

    fn arm(tracker: &mut Tracker, baseline: &CallScan) -> String {
        let join = invitation();
        let id = join.id.clone();
        tracker.reserve_join(join).unwrap();
        tracker.launch_join(&id, Some(baseline), || Ok(())).unwrap();
        id
    }

    #[test]
    fn dismissing_a_calendar_prompt_skips_that_call_and_keeps_the_invitation() {
        let mut tracker = Tracker::new(CallSettings::default());
        arm(&mut tracker, &snapshot(vec![]));
        let scan = snapshot(vec![call("desktop Teams", None)]);
        assert_eq!(
            tracker.observe(tracker.revision(), Some(&scan), IDLE),
            Decision::Prompt
        );
        let key = tracker.prompt_view().unwrap().key;
        assert!(tracker.dismiss(&key));
        assert!(tracker.prompt_view().is_none());
        assert_eq!(
            tracker.observe(tracker.revision(), Some(&scan), IDLE),
            Decision::Nothing
        );
        let matching = snapshot(vec![call("new matching window", Some([1; 32]))]);
        assert!(matches!(
            tracker.observe(tracker.revision(), Some(&matching), IDLE),
            Decision::Start { .. }
        ));
    }

    #[test]
    fn a_later_invitation_can_start_once_in_the_same_recurring_room() {
        let mut tracker = Tracker::new(CallSettings::default());
        let baseline = snapshot(vec![]);
        let room = call("reused browser window", Some([1; 32]));
        let active = snapshot(vec![room.clone()]);
        let first = arm(&mut tracker, &baseline);
        let first_revision = tracker.revision();
        let Decision::Start {
            serial: first_serial,
            ..
        } = tracker.observe(first_revision, Some(&active), IDLE)
        else {
            panic!("expected the first invitation to start");
        };
        assert!(tracker.clear_join(&first)); // Native admission consumes the first invitation.
        assert!(!tracker.watching()); // No intervening scan observes its end.

        arm(&mut tracker, &baseline);
        let revision = tracker.revision();
        let Decision::Start { serial, .. } = tracker.observe(revision, Some(&active), IDLE) else {
            panic!("the old room observation suppressed the new invitation");
        };
        tracker.start_deferred(&room, first_serial); // Late work from the previous invitation.
        assert!(!tracker.may_begin(&room, first_serial, first_revision, true, false));
        assert!(tracker.may_begin(&room, serial, revision, true, false));
        assert_eq!(
            tracker.observe(revision, Some(&active), IDLE),
            Decision::Nothing
        );
    }

    #[test]
    fn a_canceled_desktop_prompt_does_not_suppress_confirmation_for_the_next_join() {
        let mut tracker = Tracker::new(CallSettings::default());
        let baseline = snapshot(vec![]);
        let desktop = snapshot(vec![call("reused desktop window", None)]);
        let first = arm(&mut tracker, &baseline);
        assert_eq!(
            tracker.observe(tracker.revision(), Some(&desktop), IDLE),
            Decision::Prompt
        );
        let old_key = tracker.prompt_view().unwrap().key;
        assert!(tracker.clear_join(&first));

        arm(&mut tracker, &baseline);
        let revision = tracker.revision();
        assert_eq!(
            tracker.observe(revision, Some(&desktop), IDLE),
            Decision::Prompt
        );
        assert_eq!(
            tracker.observe(revision, Some(&desktop), IDLE),
            Decision::Nothing
        );
        let key = tracker.prompt_view().unwrap().key;
        assert!(tracker.confirm(&old_key).is_none());
        let (call, serial, _) = tracker.confirm(&key).unwrap();
        assert!(!tracker.may_begin(&call, serial, revision, true, false));
        assert!(tracker.may_begin(&call, serial, revision, true, true));
    }

    #[test]
    fn canceling_a_join_preserves_continuous_deduplication_for_an_existing_call() {
        let mut tracker = Tracker::new(CallSettings {
            mode: Mode::Auto,
            ..Default::default()
        });
        let active = snapshot(vec![call("continuous call", Some([1; 32]))]);
        assert!(matches!(
            tracker.observe(tracker.revision(), Some(&active), IDLE),
            Decision::Start { .. }
        ));
        let id = arm(&mut tracker, &active);
        assert_eq!(
            tracker.observe(tracker.revision(), Some(&active), IDLE),
            Decision::Nothing
        );
        assert!(tracker.clear_join(&id));
        assert_eq!(
            tracker.observe(tracker.revision(), Some(&active), IDLE),
            Decision::Nothing
        );
    }
}
