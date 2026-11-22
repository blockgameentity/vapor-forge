//! The reply rewrite, end to end on a Linux test: a fake job object, a real
//! [`HostState`], and the swap/restore the body performs around the original call.

use core::ffi::c_void;

use prost::Message;
use vapor_forge_features::achievements::set_local_rebuild;
use vapor_forge_steam_protocol::{AchievementBlock, ClientGetUserStatsResponse};
use vapor_forge_steam_protocol::PlayerGetUserStatsResponse;

use super::*;
use crate::hooks::achievements::geometry::JobReply;
use crate::hooks::tests_support::{config_with, state_with};

/// The same schema fixture as [`super::reply`]'s tests: stat 11 with bits 0 and 3,
/// stat 12 with bit 1.
fn schema() -> Vec<u8> {
    fn object(out: &mut Vec<u8>, key: &str, body: impl FnOnce(&mut Vec<u8>)) {
        out.push(0);
        out.extend_from_slice(key.as_bytes());
        out.push(0);
        body(out);
        out.push(8);
    }
    fn int(out: &mut Vec<u8>, key: &str, value: i32) {
        out.push(2);
        out.extend_from_slice(key.as_bytes());
        out.push(0);
        out.extend_from_slice(&value.to_le_bytes());
    }
    fn string(out: &mut Vec<u8>, key: &str, value: &str) {
        out.push(1);
        out.extend_from_slice(key.as_bytes());
        out.push(0);
        out.extend_from_slice(value.as_bytes());
        out.push(0);
    }
    let mut out = Vec::new();
    object(&mut out, "620", |root| {
        object(root, "stats", |stats| {
            object(stats, "11", |stat| {
                int(stat, "type", 4);
                object(stat, "bits", |bits| {
                    object(bits, "0", |achievement| string(achievement, "name", "ACH_FIRST"));
                    object(bits, "3", |achievement| string(achievement, "name", "ACH_THIRD"));
                });
            });
            object(stats, "12", |stat| {
                int(stat, "type", 4);
                object(stat, "bits", |bits| {
                    object(bits, "1", |achievement| string(achievement, "name", "ACH_OTHER"));
                });
            });
        });
    });
    out
}

fn game_id(app: u32) -> u64 {
    u64::from(app) | (1 << 24)
}

/// A legacy reply that carries no schema and unlocks one of stat 11's two bits.
fn legacy_body() -> Vec<u8> {
    ClientGetUserStatsResponse {
        game_id: Some(game_id(480)),
        eresult: Some(1),
        schema: None,
        achievement_blocks: vec![AchievementBlock {
            achievement_id: Some(11),
            unlock_time: vec![1_600_000_000],
        }],
        ..Default::default()
    }
    .encode_to_vec()
}

struct FakeJob {
    job: Box<[u8; 0x40]>,
    reply: Box<[u8; 0x40]>,
    body: Vec<u8>,
}

impl FakeJob {
    fn new(body: Vec<u8>) -> Self {
        let mut fake = Self {
            job: Box::new([0u8; 0x40]),
            reply: Box::new([0u8; 0x40]),
            body,
        };
        let geometry = geometry();
        // SAFETY: both storages span the offsets this fixture uses.
        unsafe {
            let reply_object = fake.reply.as_mut_ptr();
            *(fake.job.as_mut_ptr().add(geometry.reply_field).cast::<*mut u8>()) = reply_object;
            *(reply_object.add(geometry.body).cast::<*const u8>()) = fake.body.as_ptr();
            *(reply_object.add(geometry.size_offset()).cast::<usize>()) = fake.body.len();
        }
        fake
    }

    fn this(&mut self) -> *mut c_void {
        self.job.as_mut_ptr().cast()
    }

    fn slots(&self) -> (*const u8, usize) {
        let geometry = geometry();
        // SAFETY: both storages are live and span the offsets used here.
        unsafe {
            let reply_object = (self.reply.as_ptr() as *mut u8).cast::<u8>();
            (
                *(reply_object.add(geometry.body).cast::<*const u8>()),
                *(reply_object.add(geometry.size_offset()).cast::<usize>()),
            )
        }
    }

    fn served(&self) -> Vec<u8> {
        let (data, size) = self.slots();
        // SAFETY: the pointer is either the fixture's body or the swap's allocation.
        unsafe { std::slice::from_raw_parts(data, size).to_vec() }
    }
}

fn geometry() -> JobReply {
    JobReply::new(0x10, 0x08).expect("aligned and in bounds")
}

/// A state that decoded the geometry and turned the rebuild on.
fn rewriting_state() -> HostState {
    set_local_rebuild(true);
    let state = state_with(config_with(&[480]), Vec::new());
    state.set_reply_geometry(geometry());
    // The definitions come from a schema a *previous* reply carried, which is the
    // only source this host has.
    state.achievement_schemas().remember(480, &schema());
    state.with_achievement_settings(crate::features::AchievementSettings {
        local_rebuild: true,
    })
}

fn serial() -> std::sync::MutexGuard<'static, ()> {
    crate::test_lock::serial()
}

#[test]
fn a_rebuilt_reply_is_served_for_the_call_and_steams_bytes_come_back() {
    let _guard = serial();
    let state = rewriting_state();
    let mut fake = FakeJob::new(legacy_body());
    let original = fake.slots();
    let steam_body = fake.served();

    // SAFETY: the fake job and reply object outlive the swap, which is dropped here.
    let prepared = unsafe { prepare(&state, fake.this(), 1) };
    let swap = prepared.swap.expect("a schema-less reply with known definitions is rebuilt");
    let served = fake.served();
    assert_eq!(prepared.observation.reply, ReplyOutcome::Served { bytes: served.len() });
    assert!(prepared.observation.rebuilds());
    drop(swap);

    let decoded = ClientGetUserStatsResponse::decode(served.as_slice()).expect("a legacy reply");
    assert_eq!(decoded.game_id, Some(game_id(480)), "the game id stays Steam's");
    assert_eq!(decoded.eresult, Some(1), "the EResult stays Steam's");
    assert_eq!(
        decoded.achievement_blocks.len(),
        2,
        "the rebuilt payload speaks for both achievements stats, not just Steam's one"
    );
    assert_eq!(decoded.achievement_blocks[0].unlock_time[0], 1_600_000_000);

    assert_eq!(fake.slots(), original, "Steam's pointer and length are back");
    assert_eq!(fake.served(), steam_body, "Steam's own bytes are back");
    set_local_rebuild(false);
}

#[test]
fn a_disabled_flag_or_capability_gates_before_any_read() {
    let _guard = serial();
    set_local_rebuild(false);
    let state = rewriting_state().with_achievement_settings(crate::features::AchievementSettings {
        local_rebuild: false,
    });
    // SAFETY: the gate returns before `this` is dereferenced, which is why a dangling
    // pointer is passed on purpose.
    let prepared = unsafe { prepare(&state, 0x1000 as *mut c_void, 1) };
    assert!(prepared.swap.is_none());
    assert_eq!(prepared.observation.reply, ReplyOutcome::Gated);
    assert!(!prepared.observation.rebuilds());
    assert!(describe(&prepared.observation.reply).contains("forwarded unchanged"));
}

#[test]
fn a_disabled_capability_gates_even_with_the_flag_on() {
    let _guard = serial();
    set_local_rebuild(true);
    let state = rewriting_state();
    let mut gate = state.gate();
    gate.disable(Capability::Achievements, "the merge anchor did not resolve");
    state.set_gate(gate);
    // SAFETY: gated before any dereference.
    let prepared = unsafe { prepare(&state, 0x1000 as *mut c_void, 1) };
    assert!(prepared.swap.is_none());
    assert_eq!(prepared.observation.reply, ReplyOutcome::Gated);
    assert_eq!(
        prepared.observation.plan,
        crate::features::achievements::RebuildPlan::CapabilityDisabled(
            "the merge anchor did not resolve".to_owned()
        )
    );
    set_local_rebuild(false);
}

#[test]
fn without_a_geometry_the_merge_records_the_pending_decoder() {
    let _guard = serial();
    set_local_rebuild(true);
    let state = state_with(config_with(&[480]), Vec::new()).with_achievement_settings(
        crate::features::AchievementSettings {
            local_rebuild: true,
        },
    );
    // SAFETY: no geometry means the body returns before the dereference.
    let prepared = unsafe { prepare(&state, 0x1000 as *mut c_void, 1) };
    assert!(prepared.swap.is_none());
    assert!(matches!(prepared.observation.reply, ReplyOutcome::NoGeometry(reason) if reason == geometry::PENDING));
    set_local_rebuild(false);
}

#[test]
fn a_schema_the_reply_carries_is_remembered_even_when_nothing_is_rewritten() {
    let _guard = serial();
    let state = rewriting_state();
    let body = ClientGetUserStatsResponse {
        game_id: Some(game_id(480)),
        schema: Some(schema()),
        ..Default::default()
    }
    .encode_to_vec();
    let mut fake = FakeJob::new(body);
    // A different app, so this reply is the first schema this host sees for it.
    let other = ClientGetUserStatsResponse {
        game_id: Some(game_id(481)),
        schema: Some(schema()),
        ..Default::default()
    }
    .encode_to_vec();
    let mut other_fake = FakeJob::new(other);
    // SAFETY: both fakes outlive their calls.
    unsafe {
        let prepared = prepare(&state, other_fake.this(), 1);
        assert!(prepared.swap.is_none(), "a reply with a schema keeps the schema path");
        let prepared = prepare(&state, fake.this(), 1);
        assert!(prepared.swap.is_none());
    }
    assert!(state.achievement_schemas().schema(481).is_some());
    set_local_rebuild(false);
}

#[test]
fn a_service_reply_names_no_app_and_is_left_alone() {
    let _guard = serial();
    let state = rewriting_state();
    let body = PlayerGetUserStatsResponse {
        schema: Some(schema()),
        ..Default::default()
    }
    .encode_to_vec();
    let mut fake = FakeJob::new(body);
    // SAFETY: the fake outlives the call; nothing is written on this path.
    let prepared = unsafe { prepare(&state, fake.this(), 1) };
    assert!(prepared.swap.is_none());
    let ReplyOutcome::Unreadable(reason) = &prepared.observation.reply else {
        panic!("a service reply must be reported, not rewritten: {:?}", prepared.observation.reply);
    };
    assert!(reason.contains("carries no app id"), "{reason}");
    set_local_rebuild(false);
}

#[test]
fn a_malformed_body_is_reported_and_never_written() {
    let _guard = serial();
    let state = rewriting_state();
    let mut fake = FakeJob::new(vec![0xff, 0xff, 0xff]);
    let original = fake.slots();
    // SAFETY: nothing is written on this path.
    let prepared = unsafe { prepare(&state, fake.this(), 1) };
    assert!(prepared.swap.is_none());
    assert!(matches!(prepared.observation.reply, ReplyOutcome::Unreadable(_)));
    assert_eq!(fake.slots(), original);
    set_local_rebuild(false);
}
