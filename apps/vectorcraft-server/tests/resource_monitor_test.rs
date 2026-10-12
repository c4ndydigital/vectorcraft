use std::time::{Duration, Instant};
use vectorcraft_server::{AlertEvaluator, AlertEvent, AuthResult, LegalConfig, ResourceMetrics, UmamiConfig, check_bearer_auth, html_escape};

#[test]
fn test_threshold_state_machine_and_recovery() {
    let mut evaluator = AlertEvaluator::new(80.0, 80.0, 30, true);
    let t0 = Instant::now();

    // 1. Initial normal state -> no alert
    let mut metrics = ResourceMetrics {
        memory_percent: 75.0,
        rooms_percent: 40.0,
        rooms_count: 4,
        max_rooms: 10,
        active_connections: 8,
        largest_room_bytes: 1024,
        rejected_by_limit_count: 0,
        uptime_seconds: 100,
        ..Default::default()
    };
    assert_eq!(evaluator.evaluate(&metrics, t0), None);

    // 2. Spike in memory -> Critical alert fired
    metrics.memory_percent = 88.5;
    let event = evaluator.evaluate(&metrics, t0).expect("should fire alert");
    match &event {
        AlertEvent::Critical { mem_percent, rooms_percent, .. } => {
            assert_eq!(*mem_percent, 88.5);
            assert_eq!(*rooms_percent, 40.0);
        }
        _ => panic!("expected Critical alert"),
    }
    assert!(event.format_german().contains("ACHTUNG: VectorCraft Ressourcengrenze"));

    // 3. 15 minutes later, memory still 88.5% -> suppressed by 30-min cooldown
    let t1 = t0 + Duration::from_secs(15 * 60);
    assert_eq!(evaluator.evaluate(&metrics, t1), None);

    // 4. 35 minutes later, memory still 88.5% -> cooldown elapsed, fire alert again
    let t2 = t0 + Duration::from_secs(35 * 60);
    let event2 = evaluator.evaluate(&metrics, t2).expect("cooldown elapsed, should refire");
    assert!(matches!(event2, AlertEvent::Critical { .. }));

    // 5. Memory drops to 60.0% -> Recovery alert fired
    metrics.memory_percent = 60.0;
    let t3 = t0 + Duration::from_secs(40 * 60);
    let event3 = evaluator.evaluate(&metrics, t3).expect("should fire recovery");
    match &event3 {
        AlertEvent::Recovery { mem_percent, .. } => {
            assert_eq!(*mem_percent, 60.0);
        }
        _ => panic!("expected Recovery alert"),
    }
    assert!(event3.format_german().contains("ENTWARNUNG"));

    // 6. Normal remains normal -> no alert
    let t4 = t0 + Duration::from_secs(45 * 60);
    assert_eq!(evaluator.evaluate(&metrics, t4), None);
}

#[test]
fn test_bearer_token_authorization() {
    let token = "super-secret-token-123";

    // Disabled when unset
    assert_eq!(check_bearer_auth(None, None), AuthResult::Disabled);
    assert_eq!(check_bearer_auth(Some("Bearer foo"), None), AuthResult::Disabled);
    assert_eq!(check_bearer_auth(Some("Bearer foo"), Some("")), AuthResult::Disabled);

    // Configured token checks
    assert_eq!(check_bearer_auth(None, Some(token)), AuthResult::Unauthorized);
    assert_eq!(check_bearer_auth(Some(""), Some(token)), AuthResult::Unauthorized);
    assert_eq!(check_bearer_auth(Some("Basic 12345"), Some(token)), AuthResult::Unauthorized);
    assert_eq!(check_bearer_auth(Some("Bearer wrong-token"), Some(token)), AuthResult::Unauthorized);
    assert_eq!(check_bearer_auth(Some("Bearer  wrong  "), Some(token)), AuthResult::Unauthorized);

    // Valid Bearer token
    assert_eq!(check_bearer_auth(Some("Bearer super-secret-token-123"), Some(token)), AuthResult::Authorized);
    assert_eq!(check_bearer_auth(Some("Bearer  super-secret-token-123  "), Some(token)), AuthResult::Authorized);
}

#[test]
fn test_html_escape_special_characters() {
    assert_eq!(html_escape("<script>\"'hello & world'\"</script>"), "&lt;script&gt;&quot;&#39;hello &amp; world&#39;&quot;&lt;/script&gt;");
}

#[test]
fn test_legal_and_umami_placeholders() {
    let cfg = LegalConfig {
        company_or_name: "Example GmbH".to_string(),
        address: "Musterstraße 123\n12345 Musterstadt".to_string(),
        email: "contact@example.com".to_string(),
        phone: Some("+49 123 456789".to_string()),
        register_court: Some("Amtsgericht Musterstadt".to_string()),
        register_number: Some("HRB 99999".to_string()),
        vat_id: Some("DE999999999".to_string()),
    };
    assert!(cfg.is_configured());

    let umami =
        UmamiConfig { script_url: "https://example.com/umami.js".to_string(), website_id: "00000000-0000-0000-0000-000000000000".to_string() };
    assert_eq!(umami.script_url, "https://example.com/umami.js");
}
