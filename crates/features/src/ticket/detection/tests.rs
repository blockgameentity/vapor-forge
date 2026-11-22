//! Tests for [`super`].

use super::*;
use vapor_forge_config::DenuvoMode;

fn metadata<'a>(
    app: u32,
    version: Option<u64>,
    sections: &'a [String],
    dlls: &'a [String],
    flags: u32,
) -> AppMetadata<'a> {
    AppMetadata {
        app: AppId(app),
        version,
        section_names: sections,
        dll_names: dlls,
        flags,
    }
}

fn detection_section() -> TicketSection {
    TicketSection {
        denuvo_detection: true,
        ..TicketSection::default()
    }
}

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn denuvo_section_is_detected() {
    let sections = names(&[".text", ".Denuvo", ".data"]);
    let dlls = names(&["kernel32.dll"]);
    let detection = detect(&metadata(480, Some(1), &sections, &dlls, 0));

    assert!(detection.is_denuvo_like());
    assert_eq!(
        detection.evidence(),
        &[Evidence::Section {
            name: ".Denuvo".to_owned()
        }]
    );
    assert_ne!(detection.fingerprint(), 0);
}

#[test]
fn denuvo_dll_is_detected() {
    let sections = names(&[".text"]);
    let dlls = names(&["denuvo64.dll", "game.dll"]);
    let detection = detect(&metadata(480, Some(1), &sections, &dlls, 0));

    assert!(detection.is_denuvo_like());
    assert_eq!(
        detection.evidence(),
        &[Evidence::Dll {
            name: "denuvo64.dll".to_owned()
        }]
    );
}

#[test]
fn denuvo_flag_is_detected() {
    let sections = names(&[".text"]);
    let dlls = names(&["game.dll"]);
    let detection = detect(&metadata(480, Some(1), &sections, &dlls, FLAG_DENUVO));

    assert!(detection.is_denuvo_like());
    assert_eq!(detection.evidence(), &[Evidence::Flag { bit: FLAG_DENUVO }]);
}

#[test]
fn clean_app_is_not_denuvo_like() {
    let sections = names(&[".text", ".rdata"]);
    let dlls = names(&["game.dll", "steam_api64.dll"]);
    let detection = detect(&metadata(480, Some(1), &sections, &dlls, 0));

    assert!(!detection.is_denuvo_like());
    assert!(detection.evidence().is_empty());
    assert_eq!(detection.fingerprint(), 0);
}

#[test]
fn override_true_beats_a_clean_structure() {
    let sections = names(&[".text"]);
    let dlls = names(&["game.dll"]);
    let metadata = metadata(480, Some(1), &sections, &dlls, 0);
    let mut section = detection_section();
    section.denuvo_override = Some(true);

    let decision = decide(&metadata, &section);
    assert!(decision.is_denuvo_like());
    assert_eq!(decision.reason(), "manual override: Denuvo");
    assert_eq!(decision, DenuvoDecision::Overridden { denuvo_like: true });
}

#[test]
fn override_false_beats_a_denuvo_structure() {
    let sections = names(&[".Denuvo"]);
    let dlls = names(&["denuvo64.dll"]);
    let metadata = metadata(480, Some(1), &sections, &dlls, FLAG_DENUVO);
    let mut section = detection_section();
    section.denuvo_override = Some(false);

    let decision = decide(&metadata, &section);
    assert!(!decision.is_denuvo_like());
    assert_eq!(decision.reason(), "manual override: not Denuvo");
}

#[test]
fn flags_off_means_no_app_is_denuvo_like() {
    let sections = names(&[".Denuvo"]);
    let dlls = names(&["denuvo64.dll"]);
    let metadata = metadata(480, Some(1), &sections, &dlls, FLAG_DENUVO);

    let decision = decide(&metadata, &TicketSection::default());
    assert_eq!(decision, DenuvoDecision::Undetermined);
    assert!(!decision.is_denuvo_like());
    assert_eq!(decision.reason(), "detection disabled");
}

#[test]
fn detection_only_runs_when_enabled() {
    let sections = names(&[".Denuvo"]);
    let dlls = names(&[]);
    let metadata = metadata(480, Some(1), &sections, &dlls, 0);

    assert_eq!(
        decide(&metadata, &TicketSection::default()),
        DenuvoDecision::Undetermined
    );
    let decision = decide(&metadata, &detection_section());
    assert!(decision.is_denuvo_like());
    assert_eq!(decision.reason(), "structural detection");
}

#[test]
fn version_change_is_reported() {
    let sections = names(&[".Denuvo"]);
    let dlls = names(&[]);
    let section = detection_section();
    let mut tracker = DenuvoTracker::default();

    let first = metadata(480, Some(1), &sections, &dlls, 0);
    assert_eq!(
        tracker.observe(&first, &section).1,
        VerdictChange::FirstSeen
    );
    assert_eq!(
        tracker.observe(&first, &section).1,
        VerdictChange::Unchanged
    );

    let updated = metadata(480, Some(2), &sections, &dlls, 0);
    assert_eq!(
        tracker.observe(&updated, &section).1,
        VerdictChange::Changed
    );

    // A different app is new, not changed.
    let other = metadata(481, Some(2), &sections, &dlls, 0);
    assert_eq!(
        tracker.observe(&other, &section).1,
        VerdictChange::FirstSeen
    );
}

#[test]
fn structure_change_is_reported() {
    let section = detection_section();
    let mut tracker = DenuvoTracker::default();
    let plain = names(&[".text"]);
    let denuvo_sections = names(&[".text", ".Denuvo"]);
    let dlls = names(&[]);

    let before = metadata(480, Some(7), &plain, &dlls, 0);
    let (decision, change) = tracker.observe(&before, &section);
    assert!(!decision.is_denuvo_like());
    assert_eq!(change, VerdictChange::FirstSeen);

    // Same version, but the protection layer appeared.
    let after = metadata(480, Some(7), &denuvo_sections, &dlls, 0);
    let (decision, change) = tracker.observe(&after, &section);
    assert!(decision.is_denuvo_like());
    assert_eq!(change, VerdictChange::Changed);
    assert_eq!(
        tracker.last_fingerprint(AppId(480)),
        Some(detect(&after).fingerprint())
    );
}

#[test]
fn override_follows_only_the_app_version() {
    let section = TicketSection {
        denuvo_mode: DenuvoMode::Lease,
        denuvo_override: Some(true),
        ..TicketSection::default()
    };
    let mut tracker = DenuvoTracker::default();
    let sections = names(&[".text"]);
    let dlls = names(&[]);

    let first = metadata(480, Some(1), &sections, &dlls, 0);
    assert_eq!(
        tracker.observe(&first, &section).1,
        VerdictChange::FirstSeen
    );
    assert!(tracker.observe(&first, &section).0.is_denuvo_like());
    assert_eq!(
        tracker.observe(&first, &section).1,
        VerdictChange::Unchanged
    );

    let updated = metadata(480, Some(2), &sections, &dlls, 0);
    assert_eq!(
        tracker.observe(&updated, &section).1,
        VerdictChange::Changed
    );
}

#[test]
fn forget_drops_the_record() {
    let sections = names(&[".Denuvo"]);
    let dlls = names(&[]);
    let section = detection_section();
    let mut tracker = DenuvoTracker::default();
    let metadata = metadata(480, Some(1), &sections, &dlls, 0);

    tracker.observe(&metadata, &section);
    assert!(tracker.forget(AppId(480)));
    assert!(!tracker.forget(AppId(480)));
    assert_eq!(
        tracker.observe(&metadata, &section).1,
        VerdictChange::FirstSeen
    );
}
