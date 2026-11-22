use super::*;

fn add(metrics: &mut ConversationMetrics, id: u64, side: Side, start: f64, end: f64, text: &str) {
    metrics.ingest(Segment {
        id,
        side,
        start,
        end,
        text,
        is_partial: false,
    });
}

#[test]
fn talk_ratio_counts_a_sides_overlaps_once_and_total_unions_both_sides() {
    let mut metrics = ConversationMetrics::default();
    add(&mut metrics, 2, Side::Me, 2., 6., "my next words");
    add(&mut metrics, 3, Side::Others, 4., 8., "their words");
    add(&mut metrics, 1, Side::Me, 0., 4., "my first words");
    let snapshot = metrics.summarize().snapshot(8., true);
    assert_eq!(snapshot.totals.speech_seconds, 8.);
    let sides = snapshot.sides.unwrap();
    assert_eq!(sides.me_seconds, 6.);
    assert_eq!(sides.others_seconds, 4.);
    assert_eq!(sides.me_share, 0.6);
    assert_eq!(sides.pace_wpm, Some(60));
}

#[test]
fn duplicates_revisions_and_partials_follow_segment_identity() {
    let mut metrics = ConversationMetrics::default();
    add(&mut metrics, 7, Side::Unknown, 0., 3., "Three old words");
    add(&mut metrics, 7, Side::Unknown, 0., 3., "Three old words");
    assert_eq!(metrics.revision(), 1);
    assert!(metrics.summarize().snapshot(3., true).sides.is_none());
    add(&mut metrics, 7, Side::Others, 1., 3., "Revised?");
    metrics.ingest(Segment {
        id: 7,
        side: Side::Me,
        start: 0.,
        end: 4.,
        text: "Partial",
        is_partial: true,
    });
    let snapshot = metrics.summarize().snapshot(4., true);
    assert_eq!(metrics.revision(), 2);
    assert_eq!(snapshot.totals.words, 1);
    assert_eq!(snapshot.totals.speech_seconds, 2.);
    assert_eq!(snapshot.sides.unwrap().others_questions, 1);
}

#[test]
fn questions_count_terminal_mark_clusters_and_supported_punctuation() {
    assert_eq!(question_count("Ready? Really?! «Sure？» ¿Sí? نعم؟ Να;"), 6);
    assert_eq!(
        question_count("Yes; maybe: ¿ question?word 何？はい？ نعم؟لا؟"),
        4
    );
}

#[test]
fn monologue_bridges_small_gaps_but_stops_for_others_or_silence() {
    let mut metrics = ConversationMetrics::default();
    add(&mut metrics, 4, Side::Me, 9., 12., "last");
    add(&mut metrics, 2, Side::Me, 4., 6., "again");
    add(&mut metrics, 1, Side::Me, 0., 3., "first");
    let summary = metrics.summarize();
    let sides = summary.snapshot(12., true).sides.unwrap();
    assert_eq!(sides.current_monologue_seconds, 3.);
    assert_eq!(sides.longest_monologue_seconds, 6.);
    assert_eq!(
        summary
            .snapshot(15., true)
            .sides
            .unwrap()
            .current_monologue_seconds,
        0.
    );

    add(&mut metrics, 3, Side::Others, 10., 11., "Interruption");
    let sides = metrics.summarize().snapshot(12., true).sides.unwrap();
    assert_eq!(sides.current_monologue_seconds, 1.);
    assert_eq!(sides.longest_monologue_seconds, 6.);
}

#[test]
fn paused_wall_time_never_enters_the_audio_clock() {
    let mut metrics = ConversationMetrics::default();
    add(&mut metrics, 1, Side::Me, 0., 4., "Before");
    let summary = metrics.summarize();
    // A live caller passes the same audio position throughout a pause.
    let before_pause = summary.snapshot(4., true).sides.unwrap();
    let during_pause = summary.snapshot(4., true).sides.unwrap();
    assert_eq!(during_pause.current_monologue_seconds, 4.);
    assert_eq!(
        during_pause.longest_monologue_seconds,
        before_pause.longest_monologue_seconds
    );
    assert_eq!(during_pause.recent_pace_wpm, before_pause.recent_pace_wpm);
    add(&mut metrics, 2, Side::Me, 4.5, 6., "After");
    let sides = metrics.summarize().snapshot(6., true).sides.unwrap();
    assert_eq!(sides.current_monologue_seconds, 6.);
    assert_eq!(sides.longest_monologue_seconds, 6.);
}

#[test]
fn recent_pace_clips_segments_and_expires_as_the_audio_clock_advances() {
    let mut metrics = ConversationMetrics::default();
    add(&mut metrics, 1, Side::Me, 0., 4., "one two three four");
    add(&mut metrics, 2, Side::Me, 121., 123., "two three four five");
    let summary = metrics.summarize();
    let sides = summary.snapshot(124., true).sides.unwrap();
    assert_eq!(sides.pace_wpm, Some(80));
    assert_eq!(
        summary.snapshot(123., true).sides.unwrap().recent_pace_wpm,
        Some(100)
    );
    assert_eq!(sides.recent_pace_wpm, Some(120));
    assert_eq!(
        summary.snapshot(243., true).sides.unwrap().recent_pace_wpm,
        None
    );
}

#[test]
fn pace_is_unavailable_for_scripts_without_consistent_word_spaces() {
    for text in ["中文句子", "日本語です", "ひらがな", "ภาษาไทย"] {
        let mut metrics = ConversationMetrics::default();
        add(&mut metrics, 1, Side::Me, 0., 3., text);
        add(&mut metrics, 2, Side::Me, 121., 123., "Then English");
        let summary = metrics.summarize();
        let sides = summary.snapshot(123., true).sides.unwrap();
        assert_eq!(sides.pace_wpm, None, "{text}");
        assert_eq!(sides.recent_pace_wpm, Some(60), "{text}");
        assert_eq!(summary.snapshot(123., false).sides, None);
    }
}

#[test]
fn unknown_sides_and_disabled_attribution_keep_overall_totals() {
    let mut metrics = ConversationMetrics::default();
    add(&mut metrics, 1, Side::Me, 0., 3., "One?");
    let disabled = metrics.summarize().snapshot(3., false);
    assert_eq!(disabled.availability, Availability::LabelsOff);
    assert!(disabled.sides.is_none());
    add(&mut metrics, 2, Side::Unknown, 3., 6., "Two?");
    let snapshot = metrics.summarize().snapshot(6., true);
    assert_eq!(snapshot.availability, Availability::UnclearSource);
    assert!(snapshot.sides.is_none());
    assert_eq!(snapshot.totals.questions, 2);
    assert_eq!(snapshot.totals.words, 2);
    assert_eq!(snapshot.totals.speech_seconds, 6.);

    let next = ConversationMetrics::default()
        .summarize()
        .snapshot(0., true);
    assert_eq!(next.availability, Availability::WaitingForSpeech);
    assert_eq!(next.totals, Totals::default());
    assert!(next.sides.is_none());
}
