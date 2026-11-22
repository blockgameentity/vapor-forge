use super::*;
use std::collections::HashMap;

fn controlled_config(app_id: u32) -> RuntimeConfig {
    RuntimeConfig {
        apps: vapor_forge_config::AppsSection::with_inject(vec![vapor_forge_config::InjectApp {
            id: AppId(app_id),
            dlc: Vec::new(),
            ticket: Default::default(),
            purchase_time: 0,
        }]),
        ..Default::default()
    }
}

#[test]
fn service_response_captures_schema_before_stats_are_cleared() {
    let app_id = 3_456_781;
    let job_id = 9_001_001;
    let config = controlled_config(app_id);
    let request_header = CMsgProtoBufHeader {
        jobid_source: Some(job_id),
        ..Default::default()
    };
    let request = PlayerGetUserStatsRequest {
        steamid: Some(76561198000000001),
        appid: Some(app_id),
        sha_schema: Some(vec![0xaa]),
        ..Default::default()
    };

    let rewritten = on_send_service_stats(
        &request_header,
        &request.encode_to_vec(),
        &config,
        &HashMap::new(),
    )
    .expect("controlled request should be redirected");
    let rewritten = PlayerGetUserStatsRequest::decode(rewritten.as_slice()).unwrap();
    assert_eq!(rewritten.steamid, Some(DEFAULT_REF_STEAMID));
    assert_eq!(rewritten.sha_schema, None);

    let response_header = CMsgProtoBufHeader {
        jobid_target: Some(job_id),
        eresult: Some(2),
        routing_app_id: Some(480),
        debug_source: Some("cm".into()),
        forward_to_system_id: vec![7, 8],
        ip_addr: Some(MsgProtoBufHeaderIpAddress::V6(vec![0; 16])),
        ..Default::default()
    };
    let response = PlayerGetUserStatsResponse {
        sha_schema: Some(vec![0x12, 0xab, 0x00]),
        schema: Some(vec![0x00, 0x01, 0x02, 0x08]),
        stats: vec![PlayerStatsEntry {
            stat_id: Some(7),
            stat_value: Some(42),
            unlock_times: Vec::new(),
        }],
        ..Default::default()
    };

    let (header, body, captured) =
        on_recv_service_stats(&response_header, &response.encode_to_vec()).unwrap();
    let captured = captured.expect("schema should be captured");
    assert_eq!(captured.app_id, app_id);
    assert_eq!(captured.schema_version.as_deref(), Some("12ab00"));
    assert_eq!(captured.content, vec![0x00, 0x01, 0x02, 0x08]);

    let header = CMsgProtoBufHeader::decode(header.as_slice()).unwrap();
    let body = PlayerGetUserStatsResponse::decode(body.as_slice()).unwrap();
    assert_eq!(header.eresult, Some(ERESULT_OK));
    assert_eq!(header.routing_app_id, Some(480));
    assert_eq!(header.debug_source.as_deref(), Some("cm"));
    assert_eq!(header.forward_to_system_id, vec![7, 8]);
    assert!(matches!(
        header.ip_addr,
        Some(MsgProtoBufHeaderIpAddress::V6(ref value)) if value == &[0; 16]
    ));
    assert!(body.stats.is_empty());
    assert_eq!(body.schema, response.schema);
}

#[test]
fn legacy_request_is_redirected_and_response_values_are_cleared() {
    let app_id = 3_456_782;
    let config = controlled_config(app_id);
    let request = ClientGetUserStatsRequest {
        game_id: Some(u64::from(app_id)),
        schema_local_version: Some(-1),
        steam_id_for_user: Some(76561198000000001),
        ..Default::default()
    };

    let rewritten = on_send_legacy_stats(&request.encode_to_vec(), &config, &HashMap::new())
        .expect("controlled request should be handled");
    let rewritten = ClientGetUserStatsRequest::decode(rewritten.as_slice()).unwrap();
    assert_eq!(rewritten.steam_id_for_user, Some(DEFAULT_REF_STEAMID));

    let response = ClientGetUserStatsResponse {
        game_id: Some(u64::from(app_id)),
        eresult: Some(2),
        crc_stats: Some(0x12ab34cd),
        schema: Some(vec![1, 2, 3]),
        stats: vec![LegacyStatsEntry {
            stat_id: Some(7),
            stat_value: Some(42),
        }],
        achievement_blocks: vec![AchievementBlock {
            achievement_id: Some(9),
            unlock_time: vec![1_700_000_000],
        }],
    };
    let (body, captured) = on_recv_legacy_stats(&response.encode_to_vec(), &config).unwrap();
    let body = ClientGetUserStatsResponse::decode(body.as_slice()).unwrap();
    assert_eq!(body.eresult, Some(ERESULT_OK));
    assert!(body.stats.is_empty());
    assert!(body.achievement_blocks.is_empty());
    assert_eq!(captured.unwrap().content, vec![1, 2, 3]);
}

#[test]
fn legacy_crc_probe_is_forced_to_full_schema() {
    let app_id = 3_456_783;
    let request = ClientGetUserStatsRequest {
        game_id: Some(u64::from(app_id)),
        crc_stats: Some(0x12345678),
        schema_local_version: Some(7),
        steam_id_for_user: Some(76561198000000001),
    };
    let rewritten = on_send_legacy_stats(
        &request.encode_to_vec(),
        &controlled_config(app_id),
        &HashMap::new(),
    )
    .unwrap();
    let rewritten = ClientGetUserStatsRequest::decode(rewritten.as_slice()).unwrap();
    assert_eq!(rewritten.crc_stats, None);
    assert_eq!(rewritten.schema_local_version, Some(-1));
    assert_eq!(rewritten.steam_id_for_user, Some(DEFAULT_REF_STEAMID));
}

#[test]
fn service_plan_uses_explicit_ownership_and_lua_donor() {
    let app_id = AppId(3_456_784);
    let donor = 76561199000000001;
    let header = CMsgProtoBufHeader {
        jobid_source: Some(91),
        ..Default::default()
    };
    let request = PlayerGetUserStatsRequest {
        appid: Some(app_id.0),
        steamid: Some(76561198000000001),
        sha_schema: Some(vec![1, 2, 3]),
        crc_schema: Some(0x1234_5678),
        ..Default::default()
    };
    let donors = HashMap::from([(app_id, donor)]);

    let StatsSendPlan::Rewrite {
        body,
        donor_steam_id,
        job_id,
        ..
    } = plan_send_service_stats(
        &header,
        &request.encode_to_vec(),
        &controlled_config(app_id.0),
        &donors,
        |_| crate::apps::OwnershipState::Unowned,
    )
    else {
        panic!("expected a rewrite plan");
    };
    let rewritten = PlayerGetUserStatsRequest::decode(body.as_slice()).unwrap();
    assert_eq!(donor_steam_id, donor);
    assert_eq!(job_id, Some(91));
    assert_eq!(rewritten.steamid, Some(donor));
    assert_eq!(rewritten.sha_schema, None);
    assert_eq!(rewritten.crc_schema, None);
}

#[test]
fn stats_plans_pass_genuinely_owned_apps() {
    let app_id = AppId(3_456_785);
    let header = CMsgProtoBufHeader {
        jobid_source: Some(92),
        ..Default::default()
    };
    let request = PlayerGetUserStatsRequest {
        appid: Some(app_id.0),
        ..Default::default()
    };
    assert_eq!(
        plan_send_service_stats(
            &header,
            &request.encode_to_vec(),
            &controlled_config(app_id.0),
            &HashMap::new(),
            |_| crate::apps::OwnershipState::Owned,
        ),
        StatsSendPlan::Pass
    );
}
