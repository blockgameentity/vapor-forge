use super::*;

fn base() -> Inputs {
    Inputs {
        cm_enabled: true,
        request_codes_enabled: true,
        layout: None,
        emsg: None,
        target: None,
        cached: None,
    }
}

fn target() -> RequestTarget {
    RequestTarget {
        depot: 731,
        gid: 0x0bad_cafe,
    }
}

#[test]
fn a_disabled_gate_is_a_plain_fall_through() {
    let inputs = Inputs {
        cm_enabled: false,
        // Even a fully readable, fully cached message is left alone.
        layout: Layout::new(8, 16),
        emsg: Some(classify::EMSG_SERVICE_METHOD_RESPONSE),
        target: Some(target()),
        cached: Some(CachedCode {
            code: 42,
            provider: Some("test"),
        }),
        ..base()
    };
    assert_eq!(decide(&inputs), Disposition::Pass);
}

#[test]
fn without_a_layout_even_a_cached_code_is_only_observed() {
    // The capability gap as an assertion: everything else is available.
    let inputs = Inputs {
        emsg: Some(classify::EMSG_SERVICE_METHOD_RESPONSE),
        target: Some(target()),
        cached: Some(CachedCode {
            code: 42,
            provider: None,
        }),
        ..base()
    };
    let Disposition::Observe(observation) = decide(&inputs) else {
        panic!("a cached code with no layout must not be served");
    };
    assert_eq!(observation.blocker, ServeBlocker::NoLayout);
    assert_eq!(
        observation.class,
        Some(MessageClass::ServiceMethodResponse)
    );
    assert_eq!(observation.target, Some(target()));
}

#[test]
fn a_disabled_request_code_capability_outranks_the_layout_gap() {
    let inputs = Inputs {
        request_codes_enabled: false,
        ..base()
    };
    let Disposition::Observe(observation) = decide(&inputs) else {
        panic!("a disabled request-code capability must not serve");
    };
    assert_eq!(observation.blocker, ServeBlocker::CapabilityDisabled);
    assert!(observation.blocker.reason().contains("capability is disabled"));
}

#[test]
fn a_decoded_layout_with_a_target_and_a_cached_code_serves() {
    let inputs = Inputs {
        layout: Layout::new(8, 16),
        emsg: Some(classify::EMSG_SERVICE_METHOD_RESPONSE),
        target: Some(target()),
        cached: Some(CachedCode {
            code: 0x1234,
            provider: Some("provider-a"),
        }),
        ..base()
    };
    assert_eq!(
        decide(&inputs),
        Disposition::Serve {
            target: target(),
            code: 0x1234
        }
    );
}

#[test]
fn a_decoded_layout_without_a_target_or_a_code_still_only_observes() {
    let no_target = Inputs {
        layout: Layout::new(8, 16),
        ..base()
    };
    assert_eq!(
        decide(&no_target),
        Disposition::Observe(Observation {
            emsg: None,
            class: None,
            target: None,
            blocker: ServeBlocker::NoTarget,
        })
    );

    let no_code = Inputs {
        layout: Layout::new(8, 16),
        target: Some(target()),
        ..base()
    };
    let Disposition::Observe(observation) = decide(&no_code) else {
        panic!("nothing cached means nothing to serve");
    };
    assert_eq!(observation.blocker, ServeBlocker::NoCachedCode);
}

#[test]
fn an_unreadable_message_is_recorded_with_the_gaps_named() {
    // What the Windows body produces today: no layout, no accessor, so no
    // EMsg and no target — and a record that says exactly that.
    let Disposition::Observe(observation) = decide(&base()) else {
        panic!("an unreadable message is observed, not served");
    };
    assert_eq!(observation.emsg, None);
    assert_eq!(observation.class, None);
    assert_eq!(observation.target, None);
    assert_eq!(observation.blocker, ServeBlocker::NoLayout);
    assert!(observation.blocker.reason().contains("CNetPacket::Init"));
}

#[test]
fn an_unreadable_message_still_classifies_whatever_it_could_read() {
    // The body passes `emsg: None` because no accessor is anchored, but the
    // table does not assume that: a readable id classifies.
    let inputs = Inputs {
        emsg: Some(classify::EMSG_DEST_JOB_FAILED),
        ..base()
    };
    let Disposition::Observe(observation) = decide(&inputs) else {
        panic!("no target means nothing to serve");
    };
    assert_eq!(observation.class, Some(MessageClass::DestJobFailed));
    assert!(!observation.class.is_some_and(MessageClass::carries_request_code));
}
