//! `native-response-delivery` on Windows: retired into the synchronous
//! dispatch, so the capability is gated by the body that serves.
//!
//! These are install-batch tests, not hook tests: they assert what the report
//! says about the capability, which is the contract the retirement is worth.

use super::*;

#[test]
fn native_response_delivery_is_not_missing_while_the_serve_body_installs() {
    // Windows delivers a response inside `CCMInterface::RecvPkt`, not from a
    // posted work item, so the capability is gated by that dispatch. With the
    // dispatch installed and the packet layout undecoded, the report must still
    // keep `native-response-delivery`: only `request-codes` needs the layout.
    let corpus = TempCorpus::new("native-delivery");
    corpus.write(&corpus_text(&[(RECV, "AA BB CC DD")]));
    let mut installer = FakeInstaller::default();

    let report = run(
        &identity(),
        &synthetic_pe64(),
        &corpus.root,
        0,
        &mut installer,
    );

    assert_eq!(report.installed.len(), 1);
    assert!(report.gate.is_enabled(Capability::NativeResponseDelivery));
    assert_eq!(report.gate.reason(Capability::NativeResponseDelivery), None);
    assert!(
        !report.report().contains("native-response-delivery"),
        "a covered capability must not appear in the disabled list"
    );
    // The layout is missing in this synthetic module, so request-codes is the
    // one capability the same symbol cannot cover here.
    assert!(!report.gate.is_enabled(Capability::RequestCodes));
}

#[test]
fn a_dispatch_that_cannot_install_disables_native_response_delivery() {
    // The capability is mapped onto the body that serves, so the reason a user
    // sees is the real one: the replacement could not be installed.
    let corpus = TempCorpus::new("native-delivery-failure");
    corpus.write(&corpus_text(&[(RECV, "AA BB CC DD")]));
    let mut installer = FakeInstaller::failing_on(RECV);

    let report = run(
        &identity(),
        &synthetic_pe64(),
        &corpus.root,
        0,
        &mut installer,
    );

    assert!(!report.gate.is_enabled(Capability::NativeResponseDelivery));
    let reason = report
        .gate
        .reason(Capability::NativeResponseDelivery)
        .expect("disabled reason");
    assert!(reason.contains(RECV), "{reason}");
    assert!(reason.contains("failed to install"), "{reason}");
    assert!(report.report().contains("native-response-delivery"));
}
