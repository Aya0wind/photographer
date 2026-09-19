//! MTP 设备健康监控测试（纯决策核心 monitor_step，ping 闭包 mock）：
//! 连续 2 次失败摘除、成功清零、graveyard 探回与退避、多设备独立。

#[path = "../src/devices/health.rs"]
#[allow(dead_code)]
mod health;

use std::collections::HashMap;

use health::{monitor_step, HealthAction};

#[test]
fn busy_camera_is_neither_offline_nor_revived() {
    let registered = vec![("cam".to_string(), "camera".to_string())];
    let mut failures = HashMap::from([("cam".to_string(), 1)]);
    let mut graveyard = vec![("offline".to_string(), "camera".to_string())];
    for _ in 0..4 {
        assert!(monitor_step(
            &registered,
            &mut graveyard,
            &mut failures,
            &mut |_| None::<bool>,
            true
        )
        .is_empty());
    }
    assert_eq!(failures["cam"], 1);
    assert_eq!(graveyard.len(), 1);
}

fn offline(action: &HealthAction) -> Option<(&String, &String)> {
    match action {
        HealthAction::MarkOffline { id, name } => Some((id, name)),
        _ => None,
    }
}

#[test]
fn two_consecutive_failures_marks_offline() {
    let registered = vec![("cam-1".to_string(), "ILCE".to_string())];
    let mut graveyard = Vec::new();
    let mut failures = HashMap::new();

    // 第 1 次失败：仅计数，不摘除
    let actions = monitor_step(
        &registered,
        &mut graveyard,
        &mut failures,
        &mut |_| false,
        false,
    );
    assert!(actions.is_empty(), "单次失败不得摘除: {actions:?}");
    assert_eq!(failures.get("cam-1"), Some(&1));

    // 第 2 次失败：摘除 + 计数消费
    let actions = monitor_step(
        &registered,
        &mut graveyard,
        &mut failures,
        &mut |_| false,
        false,
    );
    assert_eq!(actions.len(), 1);
    let (id, name) = offline(&actions[0]).expect("应为 MarkOffline");
    assert_eq!((id.as_str(), name.as_str()), ("cam-1", "ILCE"));
    assert!(!failures.contains_key("cam-1"), "摘除后计数消费");
}

#[test]
fn success_resets_failure_streak() {
    let registered = vec![("cam-1".to_string(), "N".to_string())];
    let mut graveyard = Vec::new();
    let mut failures = HashMap::new();

    monitor_step(
        &registered,
        &mut graveyard,
        &mut failures,
        &mut |_| false,
        false,
    );
    let actions = monitor_step(
        &registered,
        &mut graveyard,
        &mut failures,
        &mut |_| true,
        false,
    );
    assert!(actions.is_empty());
    assert!(!failures.contains_key("cam-1"), "成功清零");

    // 再失败一轮也只计 1（未达阈值）
    let actions = monitor_step(
        &registered,
        &mut graveyard,
        &mut failures,
        &mut |_| false,
        false,
    );
    assert!(actions.is_empty(), "清零后重新累计: {actions:?}");
}

#[test]
fn graveyard_revives_on_success_and_backs_off() {
    let mut graveyard = vec![("cam-1".to_string(), "ILCE".to_string())];
    let mut failures = HashMap::new();
    let mut ping_calls = 0usize;

    // 退避轮空：不探 graveyard（0 次 ping）
    let actions = monitor_step(
        &[],
        &mut graveyard,
        &mut failures,
        &mut |_| {
            ping_calls += 1;
            true
        },
        false,
    );
    assert!(actions.is_empty());
    assert_eq!(ping_calls, 0, "退避轮不得探 graveyard");

    // 探回轮：可达 → Revive + 出 graveyard
    let actions = monitor_step(&[], &mut graveyard, &mut failures, &mut |_| true, true);
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        HealthAction::Revive { id, name } => {
            assert_eq!((id.as_str(), name.as_str()), ("cam-1", "ILCE"));
        }
        other => panic!("应为 Revive: {other:?}"),
    }
    assert!(graveyard.is_empty(), "Revive 后出名单");

    // 探回轮不可达：留名单，无动作
    let mut graveyard = vec![("cam-2".to_string(), "X".to_string())];
    let actions = monitor_step(&[], &mut graveyard, &mut failures, &mut |_| false, true);
    assert!(actions.is_empty());
    assert_eq!(graveyard.len(), 1, "不可达留在 graveyard");
}

#[test]
fn dual_devices_are_independent() {
    let registered = vec![
        ("cam-ok".to_string(), "A".to_string()),
        ("cam-bad".to_string(), "B".to_string()),
    ];
    let mut graveyard = Vec::new();
    let mut failures = HashMap::new();
    let mut results = HashMap::new();
    results.insert("cam-ok", true);
    results.insert("cam-bad", false);

    monitor_step(
        &registered,
        &mut graveyard,
        &mut failures,
        &mut |id| results[id],
        false,
    );
    let actions = monitor_step(
        &registered,
        &mut graveyard,
        &mut failures,
        &mut |id| results[id],
        false,
    );

    assert_eq!(actions.len(), 1, "仅坏设备被摘除: {actions:?}");
    assert_eq!(offline(&actions[0]).unwrap().0, "cam-bad");
    assert!(!failures.contains_key("cam-ok"), "好设备无失败计数");
}
