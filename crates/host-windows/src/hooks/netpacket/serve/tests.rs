//! The serving half, on a Linux test: one fake `CNetPacket`, one real
//! [`HostState`], and the four outcomes the brief names.
//!
//! Nothing here publishes the process-global state — [`prepare`] takes the state
//! by reference — and nothing dereferences Steam's memory: the "packet" is a local
//! array whose two decoded slots hold a real payload.

use core::ffi::c_void;

use prost::Message;
use vapor_forge_steam_protocol::{GetManifestRequestCodeRequest, GetManifestRequestCodeResponse};

use super::*;
use crate::abi::netpacket::Layout;
use crate::hooks::netpacket::decision::CachedCode;
use crate::hooks::tests_support::{config_with, state_with};

/// A byte offset pair the layout accepts, inside `FakePacket`'s 64 bytes.
fn layout() -> Layout {
    Layout::new(8, 24).expect("8-aligned data slot, 4-aligned size slot, disjoint")
}

/// A fake `CNetPacket` holding `payload`.
struct FakePacket {
    storage: Box<[u8; 64]>,
    payload: Vec<u8>,
}

impl FakePacket {
    fn new(payload: &[u8]) -> Self {
        let mut packet = Self {
            storage: Box::new([0u8; 64]),
            payload: payload.to_vec(),
        };
        let layout = layout();
        // SAFETY: the 64-byte storage spans both slots of the layout, and the
        // payload lives in `self.payload` for as long as the packet does.
        unsafe {
            let base = packet.storage.as_mut_ptr().cast::<u8>();
            *(base.add(layout.data).cast::<*const u8>()) = packet.payload.as_ptr();
            *(base.add(layout.size).cast::<u32>()) = packet.payload.len() as u32;
        }
        packet
    }

    fn as_ptr(&mut self) -> *mut c_void {
        self.storage.as_mut_ptr().cast()
    }

    /// The `(pointer, length)` Steam's packet currently carries.
    fn slots(&self) -> (*const u8, u32) {
        let layout = layout();
        // SAFETY: the storage is live and spans both slots.
        unsafe {
            let base = (self.storage.as_ptr() as *mut c_void).cast::<u8>();
            (
                *(base.add(layout.data).cast::<*const u8>()),
                *(base.add(layout.size).cast::<u32>()),
            )
        }
    }

    /// The bytes Steam's packet currently points at.
    fn served(&self) -> Vec<u8> {
        let (data, size) = self.slots();
        // SAFETY: the pointer is either the original payload (still alive) or the
        // swap's own allocation, both valid for `size` bytes.
        unsafe { std::slice::from_raw_parts(data, size as usize).to_vec() }
    }
}

fn request_body(depot: u32, gid: u64) -> Vec<u8> {
    GetManifestRequestCodeRequest {
        app_id: Some(480),
        depot_id: Some(depot),
        manifest_id: Some(gid),
        ..Default::default()
    }
    .encode_to_vec()
}

fn target() -> RequestTarget {
    RequestTarget {
        depot: 731,
        gid: 0x0bad_cafe,
    }
}

/// A state that decoded `layout` and may serve request codes.
fn serving_state() -> HostState {
    let state = state_with(config_with(&[480]), Vec::new());
    state.set_layout(layout());
    state
}

#[test]
fn a_parsed_payload_becomes_the_target_it_names() {
    let body = request_body(731, 0x0bad_cafe);
    let mut packet = FakePacket::new(&body);
    let state = serving_state();
    // SAFETY: the fake packet is live and its layout is the one the state holds.
    let prepared = unsafe { prepare(&state, packet.as_ptr()) };
    assert!(prepared.swap.is_none(), "nothing is cached, so nothing is served");
    let observation = prepared.observation.expect("the message is recorded");
    assert_eq!(observation.target, Some(target()));
    assert_eq!(observation.blocker, ServeBlocker::NoCachedCode);
    assert_eq!(observation.emsg, None);
    assert_eq!(observation.class, None);
}

#[test]
fn a_cached_code_is_served_and_the_payload_is_swapped_for_the_call() {
    let body = request_body(731, 0x0bad_cafe);
    let mut packet = FakePacket::new(&body);
    let state = serving_state();
    state.request_codes().insert(
        target(),
        CachedCode {
            code: 0xfeed_face,
            provider: Some("test"),
        },
    );
    let original = packet.slots();

    // SAFETY: the fake packet outlives the swap, which is dropped right here.
    let served = unsafe {
        let prepared = prepare(&state, packet.as_ptr());
        let swap = prepared.swap.expect("a cached code must be served");
        assert_eq!(swap.served_size() as usize, packet.served().len());
        let served = packet.served();
        assert_eq!(
            prepared.observation.map(|observation| observation.blocker),
            Some(ServeBlocker::Served),
            "a served message says so in its record, not that nothing was cached"
        );
        drop(swap);
        served
    };

    let decoded = GetManifestRequestCodeResponse::decode(served.as_slice()).expect("a response body");
    assert_eq!(decoded.manifest_request_code, Some(0xfeed_face));
    assert_eq!(packet.slots(), original, "Steam's two slots are back");
    assert_eq!(packet.served(), body, "Steam's own bytes are back");
}

#[test]
fn a_cache_miss_falls_through_untouched_and_names_no_cached_code() {
    let body = request_body(731, 1);
    let mut packet = FakePacket::new(&body);
    let state = serving_state();
    // A code for a *different* pair must not be served for this one.
    state.request_codes().insert(
        RequestTarget { depot: 731, gid: 2 },
        CachedCode {
            code: 5,
            provider: None,
        },
    );
    let original = packet.slots();
    // SAFETY: nothing is written on this path; the packet is live anyway.
    let prepared = unsafe { prepare(&state, packet.as_ptr()) };
    assert!(prepared.swap.is_none());
    assert_eq!(
        prepared.observation.map(|observation| observation.blocker),
        Some(ServeBlocker::NoCachedCode)
    );
    assert_eq!(packet.slots(), original);
    assert_eq!(packet.served(), body);
}

#[test]
fn a_malformed_payload_is_a_no_target_record_and_no_write() {
    let body = vec![0xff, 0xff, 0xff];
    let mut packet = FakePacket::new(&body);
    let state = serving_state();
    let original = packet.slots();
    // SAFETY: nothing is written on this path.
    let prepared = unsafe { prepare(&state, packet.as_ptr()) };
    assert!(prepared.swap.is_none());
    let observation = prepared.observation.expect("recorded");
    assert_eq!(observation.blocker, ServeBlocker::NoTarget);
    assert_eq!(observation.target, None);
    assert_eq!(packet.slots(), original);
}

#[test]
fn a_disabled_request_code_capability_records_the_reason_and_never_writes() {
    let body = request_body(731, 0x0bad_cafe);
    let mut packet = FakePacket::new(&body);
    let state = serving_state();
    state.request_codes().insert(
        target(),
        CachedCode {
            code: 9,
            provider: None,
        },
    );
    let mut gate = state.gate();
    gate.disable(Capability::RequestCodes, "the install pass said no");
    state.set_gate(gate);
    let original = packet.slots();
    // SAFETY: nothing is written on this path.
    let prepared = unsafe { prepare(&state, packet.as_ptr()) };
    assert!(prepared.swap.is_none());
    assert_eq!(
        prepared.observation.map(|observation| observation.blocker),
        Some(ServeBlocker::CapabilityDisabled)
    );
    assert_eq!(packet.slots(), original);
}

#[test]
fn an_install_gap_on_cm_interception_reads_nothing_at_all() {
    // With `cm-interception` off the body must not even look at the packet, which
    // is why the pointer is deliberately dangling here.
    let state = state_with(config_with(&[480]), Vec::new());
    state.set_layout(layout());
    let mut gate = state.gate();
    gate.disable(Capability::CmInterception, "no outbound symbols");
    state.set_gate(gate);
    // SAFETY: the capability check returns before any dereference.
    let prepared = unsafe { prepare(&state, 0x1000 as *mut c_void) };
    assert!(prepared.swap.is_none());
    assert!(prepared.observation.is_none(), "a disabled gate records nothing");
}

#[test]
fn without_a_layout_the_record_says_no_layout_and_nothing_is_read() {
    let state = state_with(config_with(&[480]), Vec::new());
    // SAFETY: no layout means `read_payload` returns before the dereference.
    let prepared = unsafe { prepare(&state, 0x1000 as *mut c_void) };
    assert!(prepared.swap.is_none());
    assert_eq!(
        prepared.observation.map(|observation| observation.blocker),
        Some(ServeBlocker::NoLayout)
    );
}
