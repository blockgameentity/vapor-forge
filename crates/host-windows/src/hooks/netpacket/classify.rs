//! What an inbound CM message id is, decided without touching the packet.
//!
//! Pure: `u32` in, classification out. That is deliberate — the message id is the
//! one field the body would have to read from the packet, and the win64 corpus
//! anchors neither the layout nor the EMsg accessor (see
//! [`crate::abi::netpacket`]). Keeping the table pure means it is tested on Linux
//! with the values the corpus measured, and the body is a one-line wrapper the day
//! an accessor anchor lands.
//!
//! # Where the numbers come from
//!
//! `res/patterns/win64/VTABLE-REPORT.md` §3 decodes the dispatch at
//! `CCMInterface::RecvPkt` (public `0x5d3030`): a seven-value pre-check on
//! `k_EMsgServiceMethod` (`0x93`), `k_EMsgDestJobFailed` (`0x92`), `0x2bf`,
//! `0x2ef`, `0x157c`, `0x157d`, `0x15f8`, then a two-level MSVC switch over
//! `0x2f3..=0x370` (bias `0x2f3`, `cmp $0x7d`), whose 126 values collapse to nine
//! case bodies; the rest reach the shared exit. The `.data` `{name, 0, emsg}`
//! record table names them. Those nine names are reproduced below as
//! [`CoreMessage`] variants, and the values agree with
//! `vapor_forge_steam_protocol`'s constants (`EMSG_SERVICE_METHOD_RESPONSE = 147`,
//! `EMSG_DEST_JOB_FAILED = 146`), which the Linux router classifies on.
//!
//! Nothing here guesses a name for a value the report did not name: a core-range
//! value outside the nine is [`MessageClass::CoreOther`].

/// `k_EMsgDestJobFailed` — Steam reports that a job will never complete.
pub const EMSG_DEST_JOB_FAILED: u32 = 0x92;

/// `k_EMsgServiceMethod` — the response of a service-method call.
///
/// This is the message a manifest request-code round trip returns in, and the one
/// the Linux host rewrites
/// (`vapor_forge_steam_protocol::EMSG_SERVICE_METHOD_RESPONSE`).
pub const EMSG_SERVICE_METHOD_RESPONSE: u32 = 0x93;

/// First value of the table-dispatched core range (bias of the switch).
pub const CORE_RANGE_FIRST: u32 = 0x2f3;

/// Last value of the table-dispatched core range (`0x2f3 + 0x7d`).
pub const CORE_RANGE_LAST: u32 = 0x370;

/// The nine core messages the dispatch has a real case body for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoreMessage {
    ClientLogOnResponse,
    ClientNotLoggedOnDeprecated,
    ClientPurchaseResponse,
    ClientServerList,
    ClientGameConnectDeny,
    ClientNop,
    ClientGetLegacyGameKey,
    ClientSteamUsageEvent,
    ClientClanState,
}

impl CoreMessage {
    /// The wire value, as the `.data` record table records it.
    pub fn emsg(self) -> u32 {
        match self {
            Self::ClientLogOnResponse => 0x2f3,
            Self::ClientNotLoggedOnDeprecated => 0x2f5,
            Self::ClientPurchaseResponse => 0x2fc,
            Self::ClientServerList => 0x305,
            Self::ClientGameConnectDeny => 0x306,
            Self::ClientNop => 0x30b,
            Self::ClientGetLegacyGameKey => 0x311,
            Self::ClientSteamUsageEvent => 0x32d,
            Self::ClientClanState => 0x33e,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ClientLogOnResponse => "k_EMsgClientLogOnResponse",
            Self::ClientNotLoggedOnDeprecated => "k_EMsgClientNotLoggedOnDeprecated",
            Self::ClientPurchaseResponse => "k_EMsgClientPurchaseResponse",
            Self::ClientServerList => "k_EMsgClientServerList",
            Self::ClientGameConnectDeny => "k_EMsgClientGameConnectDeny",
            Self::ClientNop => "k_EMsgClientNOP",
            Self::ClientGetLegacyGameKey => "k_EMsgClientGetLegacyGameKey",
            Self::ClientSteamUsageEvent => "k_EMsgClientSteamUsageEvent",
            Self::ClientClanState => "k_EMsgClientClanState",
        }
    }

    fn from_emsg(emsg: u32) -> Option<Self> {
        const ALL: [CoreMessage; 9] = [
            CoreMessage::ClientLogOnResponse,
            CoreMessage::ClientNotLoggedOnDeprecated,
            CoreMessage::ClientPurchaseResponse,
            CoreMessage::ClientServerList,
            CoreMessage::ClientGameConnectDeny,
            CoreMessage::ClientNop,
            CoreMessage::ClientGetLegacyGameKey,
            CoreMessage::ClientSteamUsageEvent,
            CoreMessage::ClientClanState,
        ];
        ALL.into_iter().find(|message| message.emsg() == emsg)
    }
}

/// Which path the dispatch would take for one message id.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageClass {
    /// A service-method response: the request-code round trip travels here.
    ServiceMethodResponse,
    /// A job failure. Steam's own path handles it; nothing to inject.
    DestJobFailed,
    /// One of the nine core messages with a case body.
    Core(CoreMessage),
    /// Inside the table-dispatched range, but one of the 117 values that reach
    /// the shared exit (no case body).
    CoreOther { emsg: u32 },
    /// Outside the range this dispatch handles at all: it belongs to another
    /// dispatcher (application messages go through a `CAPIJob`, per the report).
    OutsideCoreRange { emsg: u32 },
}

impl MessageClass {
    /// Whether this message can carry a manifest request code.
    ///
    /// Only [`MessageClass::ServiceMethodResponse`]: the request code is the
    /// payload of a `ContentServerDirectory.GetManifestRequestCode#1` response,
    /// which arrives as EMsg 147. A job failure or a core message never carries
    /// one, so the body has no reason to look at their bodies.
    pub fn carries_request_code(self) -> bool {
        matches!(self, Self::ServiceMethodResponse)
    }

    pub fn label(self) -> String {
        match self {
            Self::ServiceMethodResponse => "k_EMsgServiceMethod".to_owned(),
            Self::DestJobFailed => "k_EMsgDestJobFailed".to_owned(),
            Self::Core(message) => message.label().to_owned(),
            Self::CoreOther { emsg } => format!("core message {emsg:#x} (shared exit)"),
            Self::OutsideCoreRange { emsg } => format!("message {emsg:#x} (not dispatched here)"),
        }
    }
}

/// Classify one inbound message id.
pub fn classify(emsg: u32) -> MessageClass {
    match emsg {
        EMSG_SERVICE_METHOD_RESPONSE => MessageClass::ServiceMethodResponse,
        EMSG_DEST_JOB_FAILED => MessageClass::DestJobFailed,
        emsg if (CORE_RANGE_FIRST..=CORE_RANGE_LAST).contains(&emsg) => {
            match CoreMessage::from_emsg(emsg) {
                Some(message) => MessageClass::Core(message),
                None => MessageClass::CoreOther { emsg },
            }
        }
        emsg => MessageClass::OutsideCoreRange { emsg },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_method_response_is_the_only_request_code_carrier() {
        assert_eq!(
            classify(EMSG_SERVICE_METHOD_RESPONSE),
            MessageClass::ServiceMethodResponse
        );
        assert!(classify(EMSG_SERVICE_METHOD_RESPONSE).carries_request_code());
        for emsg in [0x92, 0x2f3, 0x30b, 0x2fd, 0x370, 0x157c, 0x97] {
            assert!(
                !classify(emsg).carries_request_code(),
                "{emsg:#x} is not a request-code carrier"
            );
        }
    }

    #[test]
    fn the_switch_bias_and_its_two_ends_classify_as_measured() {
        assert_eq!(CORE_RANGE_LAST - CORE_RANGE_FIRST, 0x7d);
        assert_eq!(
            classify(CORE_RANGE_FIRST),
            MessageClass::Core(CoreMessage::ClientLogOnResponse)
        );
        // 0x2fd and 0x370 are named in the report as values that reach the shared
        // exit, not case bodies.
        assert_eq!(
            classify(0x2fd),
            MessageClass::CoreOther { emsg: 0x2fd }
        );
        assert_eq!(
            classify(CORE_RANGE_LAST),
            MessageClass::CoreOther {
                emsg: CORE_RANGE_LAST
            }
        );
        // Just outside the bias window.
        assert_eq!(
            classify(CORE_RANGE_LAST + 1),
            MessageClass::OutsideCoreRange {
                emsg: CORE_RANGE_LAST + 1
            }
        );
        assert_eq!(
            classify(CORE_RANGE_FIRST - 1),
            MessageClass::OutsideCoreRange {
                emsg: CORE_RANGE_FIRST - 1
            }
        );
    }

    #[test]
    fn every_named_core_message_round_trips_through_classify() {
        for message in [
            CoreMessage::ClientLogOnResponse,
            CoreMessage::ClientNotLoggedOnDeprecated,
            CoreMessage::ClientPurchaseResponse,
            CoreMessage::ClientServerList,
            CoreMessage::ClientGameConnectDeny,
            CoreMessage::ClientNop,
            CoreMessage::ClientGetLegacyGameKey,
            CoreMessage::ClientSteamUsageEvent,
            CoreMessage::ClientClanState,
        ] {
            assert_eq!(classify(message.emsg()), MessageClass::Core(message));
            assert!(message.label().starts_with("k_EMsg"));
        }
    }
}
