//! The bytes of one inbound CM message, read and re-encoded.
//!
//! Everything here is pure: a `&[u8]` in, a [`RequestTarget`] or a
//! [`PayloadBlocker`] out. The body in [`super`] owns the two unsafe reads that
//! get those bytes out of Steam's `CNetPacket`, so this module runs unchanged on
//! a Linux test with a literal byte array.
//!
//! # Which message this is, and why the payload decides
//!
//! The dispatch's `EMsg` comes from an accessor the corpus does not anchor (see
//! [`crate::abi::netpacket`]), so the host cannot ask "which message is this".
//! It can ask the opposite question — "does this body parse as the request I can
//! answer, and does it name a `(depot, gid)` I hold a code for" — and a message
//! that does not parse is left alone. That is the conservative direction: a
//! payload the host cannot read is one it must not rewrite.
//!
//! The request shape is `GetManifestRequestCodeRequest` (EMsg 153,
//! `app_id/depot_id/manifest_id/app_branch/branch_password_hash`); the answer a
//! cached code replaces the body with is `GetManifestRequestCodeResponse`
//! (`manifest_request_code`). Both are the workspace's generated
//! `vapor-forge-steam-protocol` shapes, so the encoding is Steam's own.

use prost::Message;
use vapor_forge_steam_protocol::{GetManifestRequestCodeRequest, GetManifestRequestCodeResponse};

use super::decision::RequestTarget;
use super::fetch::Request;

/// A sanity bound on the body a request may carry.
///
/// The largest real request is a branch name plus a password hash, so anything
/// past this is not a request-code message and must not be decoded as one. It is
/// a bound, not a layout fact: nothing here is a structure offset.
pub const MAX_REQUEST_BODY: usize = 1024;

/// Why a payload could not be turned into a target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PayloadBlocker {
    /// No bytes at all: there is nothing to parse.
    Empty,
    /// Larger than [`MAX_REQUEST_BODY`]: not this message.
    TooLarge { size: usize },
    /// prost refused the bytes.
    Malformed(String),
    /// The bytes parse but name no `(depot, gid)`: nothing to look a code up for.
    NoTarget,
}

impl PayloadBlocker {
    /// One line for the record or the log, naming the cause rather than a code.
    pub fn reason(&self) -> String {
        match self {
            Self::Empty => "the inbound payload is empty".to_owned(),
            Self::TooLarge { size } => {
                format!("the inbound payload is {size} bytes, too large for a request-code message")
            }
            Self::Malformed(error) => {
                format!("the inbound payload is not a GetManifestRequestCodeRequest: {error}")
            }
            Self::NoTarget => {
                "the inbound payload names no depot or no manifest, so no code applies".to_owned()
            }
        }
    }
}

/// The `(depot, gid)` one request body is about, with the app it names.
///
/// `depot_id` and `manifest_id` are both required; `app_id` is carried on the
/// wire but a code is keyed by the depot and manifest pair, which is what Steam
/// asks with and what a provider answers with. The app is kept because a provider
/// url may name it (`{appId}`), so the fetch worker needs all three numbers.
pub fn request(body: &[u8]) -> Result<Request, PayloadBlocker> {
    let parsed = parse(body)?;
    match (parsed.depot_id, parsed.manifest_id) {
        (Some(depot), Some(gid)) if depot != 0 && gid != 0 => Ok(Request {
            app: parsed.app_id.unwrap_or(0),
            target: RequestTarget { depot, gid },
        }),
        _ => Err(PayloadBlocker::NoTarget),
    }
}

/// The `(depot, gid)` one request body is about.
pub fn target(body: &[u8]) -> Result<RequestTarget, PayloadBlocker> {
    request(body).map(|request| request.target)
}

/// Parse one request body.
pub fn parse(body: &[u8]) -> Result<GetManifestRequestCodeRequest, PayloadBlocker> {
    if body.is_empty() {
        return Err(PayloadBlocker::Empty);
    }
    if body.len() > MAX_REQUEST_BODY {
        return Err(PayloadBlocker::TooLarge { size: body.len() });
    }
    GetManifestRequestCodeRequest::decode(body).map_err(|error| PayloadBlocker::Malformed(error.to_string()))
}

/// The body a served code replaces the request with.
///
/// Encoded from the workspace's generated response shape, so the bytes are the
/// message Steam's own parser would accept — nothing is hand-assembled.
pub fn response_body(code: u64) -> Vec<u8> {
    GetManifestRequestCodeResponse {
        manifest_request_code: Some(code),
    }
    .encode_to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(depot: u32, gid: u64) -> Vec<u8> {
        GetManifestRequestCodeRequest {
            app_id: Some(480),
            depot_id: Some(depot),
            manifest_id: Some(gid),
            app_branch: Some("public".to_owned()),
            branch_password_hash: None,
        }
        .encode_to_vec()
    }

    #[test]
    fn a_request_body_becomes_the_target_it_names() {
        let body = encoded(731, 0x0bad_cafe_dead_beef);
        assert_eq!(
            target(&body),
            Ok(RequestTarget {
                depot: 731,
                gid: 0x0bad_cafe_dead_beef
            })
        );
    }

    #[test]
    fn a_request_body_also_names_the_app_its_provider_url_may_use() {
        // `{appId}` is a real placeholder in a provider url, so the app id has to
        // survive the parse and reach the fetch worker.
        let body = encoded(731, 7);
        assert_eq!(
            request(&body),
            Ok(Request {
                app: 480,
                target: RequestTarget { depot: 731, gid: 7 }
            })
        );
        // A request that omits the app keeps the target and reports app 0 rather
        // than refusing a lookup it could still answer.
        let no_app = GetManifestRequestCodeRequest {
            depot_id: Some(731),
            manifest_id: Some(7),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(request(&no_app).map(|parsed| parsed.app), Ok(0));
    }

    #[test]
    fn a_request_without_a_depot_or_without_a_manifest_has_no_target() {
        let no_depot = GetManifestRequestCodeRequest {
            app_id: Some(480),
            manifest_id: Some(7),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(target(&no_depot), Err(PayloadBlocker::NoTarget));

        let no_manifest = GetManifestRequestCodeRequest {
            app_id: Some(480),
            depot_id: Some(731),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(target(&no_manifest), Err(PayloadBlocker::NoTarget));

        // A zero is "absent" for a code lookup, not a depot id.
        assert_eq!(target(&encoded(0, 7)), Err(PayloadBlocker::NoTarget));
        assert_eq!(target(&encoded(731, 0)), Err(PayloadBlocker::NoTarget));
    }

    #[test]
    fn a_malformed_or_empty_payload_is_a_named_blocker_not_a_panic() {
        assert_eq!(target(&[]), Err(PayloadBlocker::Empty));
        assert!(matches!(
            target(&[0xff, 0xff, 0xff]),
            Err(PayloadBlocker::Malformed(_))
        ));
        let blocker = target(&[0xff, 0xff, 0xff]).unwrap_err();
        assert!(blocker.reason().contains("not a GetManifestRequestCodeRequest"));
    }

    #[test]
    fn a_body_past_the_bound_is_refused_before_prost_sees_it() {
        let mut body = encoded(731, 7);
        body.resize(MAX_REQUEST_BODY + 1, 0);
        assert_eq!(
            target(&body),
            Err(PayloadBlocker::TooLarge {
                size: MAX_REQUEST_BODY + 1
            })
        );
    }

    #[test]
    fn the_replacement_body_is_the_response_carrying_the_code() {
        let bytes = response_body(0x0123_4567_89ab_cdef);
        let decoded = GetManifestRequestCodeResponse::decode(bytes.as_slice()).expect("round trip");
        assert_eq!(decoded.manifest_request_code, Some(0x0123_4567_89ab_cdef));
        // Whatever it decodes as, it is not a request with a depot and a
        // manifest: the replacement body must never be read back as a target.
        assert!(target(&bytes).is_err());
    }

    #[test]
    fn a_payload_of_zeros_is_not_a_request_code_message() {
        // A zero byte is not a protobuf tag, so the body is refused rather than read
        // as an empty request with no fields.
        assert!(matches!(target(&[0u8; 8]), Err(PayloadBlocker::Malformed(_))));
    }
}
