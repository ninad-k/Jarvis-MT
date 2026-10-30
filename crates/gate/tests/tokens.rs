//! Token store behaviour: expiry boundaries, read-back timing, batch two-stage flow,
//! cancellation, replay reporting, kill switch and purge (SPEC rules 2-4, 6).

use std::collections::HashSet;
use std::sync::Arc;

use jarvis_bridge_client::protocol::{Command, OrderCancelArgs, PositionCloseArgs};
use jarvis_gate::{
    ActionPreview, ConfirmOutcome, EstPnl, IssuedToken, KillSwitch, TokenError, TokenStage,
    TokenStore, TokenTiming,
};
use uuid::Uuid;

const TTL: i64 = 15_000;
const CAP: i64 = 30_000;
const T0: i64 = 1_000_000;

fn store() -> (TokenStore, KillSwitch) {
    let ks = KillSwitch::default();
    (TokenStore::new(TTL, CAP, ks.clone()), ks)
}

fn single_preview() -> ActionPreview {
    ActionPreview {
        id: Uuid::new_v4(),
        description: "Close buy 0.10 XAUUSD".into(),
        symbols: vec!["XAUUSD".into()],
        tickets: vec![101],
        total_volume: 0.1,
        est_pnl: EstPnl::default(),
        is_batch: false,
        count: 1,
        warnings: vec![],
        commands: vec![Command::PositionClose(PositionCloseArgs {
            ticket: 101,
            deviation_points: None,
        })],
    }
}

fn batch_close_preview(n: u64) -> ActionPreview {
    ActionPreview {
        id: Uuid::new_v4(),
        description: format!("Close {n} positions"),
        symbols: vec!["XAUUSD".into()],
        tickets: (100..100 + n).collect(),
        total_volume: 0.1 * n as f64,
        est_pnl: EstPnl::default(),
        is_batch: true,
        count: n as usize,
        warnings: vec![],
        commands: (100..100 + n)
            .map(|t| {
                Command::PositionClose(PositionCloseArgs {
                    ticket: t,
                    deviation_points: None,
                })
            })
            .collect(),
    }
}

fn batch_cancel_preview(n: u64) -> ActionPreview {
    let mut p = batch_close_preview(n);
    p.commands = (100..100 + n)
        .map(|t| Command::OrderCancel(OrderCancelArgs { ticket: t }))
        .collect();
    p
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn expect_second(outcome: ConfirmOutcome) -> (IssuedToken, usize, String) {
    match outcome {
        ConfirmOutcome::NeedsSecond {
            token,
            count,
            message,
        } => (token, count, message),
        other => panic!("expected NeedsSecond, got {other:?}"),
    }
}

// ---- issue -----------------------------------------------------------------------

#[test]
fn issue_immediate_sets_expiry_and_shape() {
    let (s, _) = store();
    let p = single_preview();
    let t = s.issue(p.clone(), TokenTiming::Immediate, T0);
    assert!(is_hex64(&t.token), "token {:?}", t.token);
    assert_eq!(t.preview_id, p.id);
    assert_eq!(t.stage, TokenStage::First);
    assert_eq!(t.expires_at_ms, T0 + TTL);
}

#[test]
fn issue_after_readback_starts_at_cap() {
    let (s, _) = store();
    let t = s.issue(single_preview(), TokenTiming::AfterReadback, T0);
    assert_eq!(t.expires_at_ms, T0 + CAP);
}

#[test]
fn thousand_tokens_are_unique_64_hex() {
    let (s, _) = store();
    let mut seen = HashSet::new();
    for i in 0..1000 {
        let t = s.issue(single_preview(), TokenTiming::Immediate, T0 + i);
        assert!(is_hex64(&t.token), "token {:?}", t.token);
        assert!(seen.insert(t.token), "duplicate token at {i}");
    }
    assert_eq!(seen.len(), 1000);
}

// ---- confirm: single actions -----------------------------------------------------

#[test]
fn confirm_single_executes_exact_preview() {
    let (s, _) = store();
    let p = single_preview();
    let t = s.issue(p.clone(), TokenTiming::Immediate, T0);
    assert_eq!(s.confirm(&t.token, T0 + 1), Ok(ConfirmOutcome::Execute(p)));
}

#[test]
fn confirm_is_single_use() {
    let (s, _) = store();
    let t = s.issue(single_preview(), TokenTiming::Immediate, T0);
    assert!(s.confirm(&t.token, T0 + 1).is_ok());
    assert_eq!(s.confirm(&t.token, T0 + 2), Err(TokenError::AlreadyUsed));
    assert_eq!(s.confirm(&t.token, T0 + 3), Err(TokenError::AlreadyUsed));
}

#[test]
fn confirm_unknown_token() {
    let (s, _) = store();
    assert_eq!(s.confirm("deadbeef", T0), Err(TokenError::Unknown));
    assert_eq!(s.confirm("", T0), Err(TokenError::Unknown));
}

#[test]
fn expiry_boundary_is_exclusive_immediate() {
    // (confirm time offset from issue, expected ok)
    let cases = [
        (0, true),
        (1, true),
        (TTL - 1, true),
        (TTL, false),
        (TTL + 1, false),
        (TTL * 10, false),
    ];
    for (offset, ok) in cases {
        let (s, _) = store();
        let t = s.issue(single_preview(), TokenTiming::Immediate, T0);
        let r = s.confirm(&t.token, T0 + offset);
        if ok {
            assert!(r.is_ok(), "offset {offset}: {r:?}");
        } else {
            assert_eq!(r, Err(TokenError::Expired), "offset {offset}");
        }
    }
}

#[test]
fn expired_token_replay_reports_expired_not_unknown() {
    let (s, _) = store();
    let t = s.issue(single_preview(), TokenTiming::Immediate, T0);
    assert_eq!(s.confirm(&t.token, T0 + TTL), Err(TokenError::Expired));
    assert_eq!(s.confirm(&t.token, T0 + TTL + 5), Err(TokenError::Expired));
    // even if the clock were to go backwards the token stays dead
    assert_eq!(s.confirm(&t.token, T0), Err(TokenError::Expired));
}

// ---- read-back timing (ADR-0007) --------------------------------------------------

#[test]
fn readback_window_starts_after_tts() {
    let (s, _) = store();
    let t = s.issue(single_preview(), TokenTiming::AfterReadback, T0);
    let updated = s.readback_finished(&t.token, T0 + 8_000).unwrap();
    assert_eq!(updated.token, t.token);
    assert_eq!(updated.expires_at_ms, T0 + 8_000 + TTL);
    assert_eq!(updated.stage, TokenStage::First);
    assert_eq!(updated.preview_id, t.preview_id);
    // valid up to but excluding the new expiry
    assert!(s.confirm(&t.token, T0 + 8_000 + TTL - 1).is_ok());
}

#[test]
fn readback_window_expires_at_new_expiry() {
    let (s, _) = store();
    let t = s.issue(single_preview(), TokenTiming::AfterReadback, T0);
    s.readback_finished(&t.token, T0 + 8_000).unwrap();
    assert_eq!(
        s.confirm(&t.token, T0 + 8_000 + TTL),
        Err(TokenError::Expired)
    );
}

#[test]
fn readback_window_is_capped() {
    let (s, _) = store();
    let t = s.issue(single_preview(), TokenTiming::AfterReadback, T0);
    // read-back ends 25 s after issue: 25 + 15 = 40 s > 30 s cap
    let updated = s.readback_finished(&t.token, T0 + 25_000).unwrap();
    assert_eq!(updated.expires_at_ms, T0 + CAP);
    assert!(s.confirm(&t.token, T0 + CAP - 1).is_ok());
}

#[test]
fn readback_cap_boundary() {
    let (s, _) = store();
    let t = s.issue(single_preview(), TokenTiming::AfterReadback, T0);
    s.readback_finished(&t.token, T0 + 25_000).unwrap();
    assert_eq!(s.confirm(&t.token, T0 + CAP), Err(TokenError::Expired));
}

#[test]
fn readback_never_finishing_expires_at_cap() {
    let (s, _) = store();
    let t = s.issue(single_preview(), TokenTiming::AfterReadback, T0);
    assert!(s.confirm(&t.token, T0 + CAP - 1).is_ok());
    let t2 = s.issue(single_preview(), TokenTiming::AfterReadback, T0);
    assert_eq!(s.confirm(&t2.token, T0 + CAP), Err(TokenError::Expired));
}

#[test]
fn readback_at_or_after_cap_is_expired() {
    let (s, _) = store();
    let t = s.issue(single_preview(), TokenTiming::AfterReadback, T0);
    assert_eq!(
        s.readback_finished(&t.token, T0 + CAP),
        Err(TokenError::Expired)
    );
    assert_eq!(
        s.readback_finished(&t.token, T0 + CAP + 10),
        Err(TokenError::Expired)
    );
}

#[test]
fn readback_is_idempotent_and_cannot_extend() {
    let (s, _) = store();
    let t = s.issue(single_preview(), TokenTiming::AfterReadback, T0);
    let first = s.readback_finished(&t.token, T0 + 2_000).unwrap();
    let second = s.readback_finished(&t.token, T0 + 12_000).unwrap();
    assert_eq!(first.expires_at_ms, T0 + 2_000 + TTL);
    assert_eq!(second.expires_at_ms, first.expires_at_ms);
}

#[test]
fn readback_on_immediate_token_changes_nothing() {
    let (s, _) = store();
    let t = s.issue(single_preview(), TokenTiming::Immediate, T0);
    let r = s.readback_finished(&t.token, T0 + 5_000).unwrap();
    assert_eq!(r.expires_at_ms, T0 + TTL);
}

#[test]
fn readback_errors() {
    let (s, _) = store();
    assert_eq!(s.readback_finished("nope", T0), Err(TokenError::Unknown));
    let t = s.issue(single_preview(), TokenTiming::AfterReadback, T0);
    s.confirm(&t.token, T0 + 1).unwrap();
    assert_eq!(
        s.readback_finished(&t.token, T0 + 2),
        Err(TokenError::AlreadyUsed)
    );
    let t2 = s.issue(single_preview(), TokenTiming::AfterReadback, T0);
    s.cancel(&t2.token, T0 + 1).unwrap();
    assert_eq!(
        s.readback_finished(&t2.token, T0 + 2),
        Err(TokenError::AlreadyUsed)
    );
}

#[test]
fn confirm_before_readback_finishes_is_allowed_within_cap() {
    let (s, _) = store();
    let t = s.issue(single_preview(), TokenTiming::AfterReadback, T0);
    assert!(s.confirm(&t.token, T0 + 3_000).is_ok());
}

// ---- batch double confirmation (rule 4, ADR-0008) ---------------------------------

#[test]
fn batch_first_confirmation_needs_second() {
    let (s, _) = store();
    let p = batch_close_preview(3);
    let t1 = s.issue(p.clone(), TokenTiming::Immediate, T0);
    let (t2, count, message) = expect_second(s.confirm(&t1.token, T0 + 1_000).unwrap());
    assert_eq!(count, 3);
    assert_eq!(message, "Confirm again to close 3 positions");
    assert_ne!(t2.token, t1.token);
    assert!(is_hex64(&t2.token));
    assert_eq!(t2.stage, TokenStage::Second);
    assert_eq!(t2.preview_id, p.id);
    assert_eq!(t2.expires_at_ms, T0 + 1_000 + TTL);
}

#[test]
fn batch_second_confirmation_executes_same_preview() {
    let (s, _) = store();
    let p = batch_close_preview(3);
    let t1 = s.issue(p.clone(), TokenTiming::Immediate, T0);
    let (t2, _, _) = expect_second(s.confirm(&t1.token, T0 + 1_000).unwrap());
    assert_eq!(
        s.confirm(&t2.token, T0 + 2_000),
        Ok(ConfirmOutcome::Execute(p))
    );
}

#[test]
fn batch_first_token_cannot_be_reused_after_stage_one() {
    let (s, _) = store();
    let t1 = s.issue(batch_close_preview(2), TokenTiming::Immediate, T0);
    s.confirm(&t1.token, T0 + 1).unwrap();
    assert_eq!(s.confirm(&t1.token, T0 + 2), Err(TokenError::AlreadyUsed));
}

#[test]
fn batch_second_token_is_single_use() {
    let (s, _) = store();
    let t1 = s.issue(batch_close_preview(2), TokenTiming::Immediate, T0);
    let (t2, _, _) = expect_second(s.confirm(&t1.token, T0 + 1).unwrap());
    assert!(matches!(
        s.confirm(&t2.token, T0 + 2),
        Ok(ConfirmOutcome::Execute(_))
    ));
    assert_eq!(s.confirm(&t2.token, T0 + 3), Err(TokenError::AlreadyUsed));
}

#[test]
fn batch_second_stage_has_its_own_ttl_window() {
    let (s, _) = store();
    let t1 = s.issue(batch_close_preview(2), TokenTiming::Immediate, T0);
    let (t2, _, _) = expect_second(s.confirm(&t1.token, T0 + 14_000).unwrap());
    // valid beyond the first token's own window
    assert!(s.confirm(&t2.token, T0 + 14_000 + TTL - 1).is_ok());
}

#[test]
fn batch_second_stage_expires_independently() {
    let (s, _) = store();
    let t1 = s.issue(batch_close_preview(2), TokenTiming::Immediate, T0);
    let (t2, _, _) = expect_second(s.confirm(&t1.token, T0 + 1_000).unwrap());
    assert_eq!(
        s.confirm(&t2.token, T0 + 1_000 + TTL),
        Err(TokenError::Expired)
    );
}

#[test]
fn batch_second_stage_is_always_immediate_even_for_voice() {
    let (s, _) = store();
    let t1 = s.issue(batch_close_preview(4), TokenTiming::AfterReadback, T0);
    s.readback_finished(&t1.token, T0 + 5_000).unwrap();
    let (t2, _, _) = expect_second(s.confirm(&t1.token, T0 + 6_000).unwrap());
    assert_eq!(t2.expires_at_ms, T0 + 6_000 + TTL);
}

#[test]
fn batch_first_stage_expiry_blocks_second_stage() {
    let (s, _) = store();
    let t1 = s.issue(batch_close_preview(2), TokenTiming::Immediate, T0);
    assert_eq!(s.confirm(&t1.token, T0 + TTL), Err(TokenError::Expired));
}

#[test]
fn batch_message_wording_by_action() {
    let (s, _) = store();
    let cases = [
        (batch_close_preview(5), "Confirm again to close 5 positions"),
        (
            batch_cancel_preview(2),
            "Confirm again to cancel 2 pending orders",
        ),
    ];
    for (p, expected) in cases {
        let t = s.issue(p, TokenTiming::Immediate, T0);
        let (_, _, message) = expect_second(s.confirm(&t.token, T0 + 1).unwrap());
        assert_eq!(message, expected);
    }
}

#[test]
fn non_batch_preview_never_needs_second() {
    let (s, _) = store();
    let t = s.issue(single_preview(), TokenTiming::Immediate, T0);
    assert!(matches!(
        s.confirm(&t.token, T0 + 1),
        Ok(ConfirmOutcome::Execute(_))
    ));
}

// ---- cancel -----------------------------------------------------------------------

#[test]
fn cancel_consumes_token() {
    let (s, _) = store();
    let t = s.issue(single_preview(), TokenTiming::Immediate, T0);
    assert_eq!(s.cancel(&t.token, T0 + 1), Ok(()));
    assert_eq!(s.confirm(&t.token, T0 + 2), Err(TokenError::AlreadyUsed));
}

#[test]
fn cancel_twice_reports_already_used() {
    let (s, _) = store();
    let t = s.issue(single_preview(), TokenTiming::Immediate, T0);
    s.cancel(&t.token, T0 + 1).unwrap();
    assert_eq!(s.cancel(&t.token, T0 + 2), Err(TokenError::AlreadyUsed));
}

#[test]
fn cancel_after_confirm_reports_already_used() {
    let (s, _) = store();
    let t = s.issue(single_preview(), TokenTiming::Immediate, T0);
    s.confirm(&t.token, T0 + 1).unwrap();
    assert_eq!(s.cancel(&t.token, T0 + 2), Err(TokenError::AlreadyUsed));
}

#[test]
fn cancel_unknown_and_expired() {
    let (s, _) = store();
    assert_eq!(s.cancel("zzz", T0), Err(TokenError::Unknown));
    let t = s.issue(single_preview(), TokenTiming::Immediate, T0);
    assert_eq!(s.cancel(&t.token, T0 + TTL), Err(TokenError::Expired));
}

#[test]
fn cancel_second_stage_token_cancels_it() {
    let (s, _) = store();
    let t1 = s.issue(batch_close_preview(3), TokenTiming::Immediate, T0);
    let (t2, _, _) = expect_second(s.confirm(&t1.token, T0 + 1).unwrap());
    assert_eq!(s.cancel(&t2.token, T0 + 2), Ok(()));
    assert_eq!(s.confirm(&t2.token, T0 + 3), Err(TokenError::AlreadyUsed));
}

#[test]
fn cancel_via_first_token_after_stage_one_cancels_the_sibling() {
    let (s, _) = store();
    let t1 = s.issue(batch_close_preview(3), TokenTiming::Immediate, T0);
    let (t2, _, _) = expect_second(s.confirm(&t1.token, T0 + 1).unwrap());
    assert_eq!(s.cancel(&t1.token, T0 + 2), Ok(()));
    assert_eq!(s.confirm(&t2.token, T0 + 3), Err(TokenError::AlreadyUsed));
}

#[test]
fn cancel_first_stage_batch_before_confirming() {
    let (s, _) = store();
    let t1 = s.issue(batch_close_preview(3), TokenTiming::Immediate, T0);
    s.cancel(&t1.token, T0 + 1).unwrap();
    assert_eq!(s.confirm(&t1.token, T0 + 2), Err(TokenError::AlreadyUsed));
}

#[test]
fn cancel_does_not_touch_other_previews() {
    let (s, _) = store();
    let a = s.issue(single_preview(), TokenTiming::Immediate, T0);
    let b = s.issue(single_preview(), TokenTiming::Immediate, T0);
    s.cancel(&a.token, T0 + 1).unwrap();
    assert!(s.confirm(&b.token, T0 + 2).is_ok());
}

#[test]
fn cancel_still_works_while_kill_switch_engaged() {
    let (s, ks) = store();
    let t = s.issue(single_preview(), TokenTiming::Immediate, T0);
    ks.engage("test", T0);
    assert_eq!(s.cancel(&t.token, T0 + 1), Ok(()));
}

// ---- kill switch (rule 6) ---------------------------------------------------------

#[test]
fn kill_switch_blocks_confirm_in_flight() {
    let (s, ks) = store();
    let t = s.issue(single_preview(), TokenTiming::Immediate, T0);
    ks.engage("panic", T0 + 1);
    assert_eq!(
        s.confirm(&t.token, T0 + 2),
        Err(TokenError::KillSwitchEngaged)
    );
}

#[test]
fn kill_switch_checked_before_token_lookup() {
    let (s, ks) = store();
    ks.engage("panic", T0);
    assert_eq!(
        s.confirm("unknown", T0 + 1),
        Err(TokenError::KillSwitchEngaged)
    );
}

#[test]
fn kill_switch_revokes_token_for_good() {
    let (s, ks) = store();
    let t = s.issue(single_preview(), TokenTiming::Immediate, T0);
    ks.engage("panic", T0 + 1);
    assert_eq!(
        s.confirm(&t.token, T0 + 2),
        Err(TokenError::KillSwitchEngaged)
    );
    ks.release(T0 + 3);
    assert_eq!(s.confirm(&t.token, T0 + 4), Err(TokenError::AlreadyUsed));
}

#[test]
fn kill_switch_mid_batch_blocks_second_stage() {
    let (s, ks) = store();
    let t1 = s.issue(batch_close_preview(3), TokenTiming::Immediate, T0);
    let (t2, _, _) = expect_second(s.confirm(&t1.token, T0 + 1).unwrap());
    ks.engage("panic", T0 + 2);
    assert_eq!(
        s.confirm(&t2.token, T0 + 3),
        Err(TokenError::KillSwitchEngaged)
    );
    ks.release(T0 + 4);
    assert_eq!(s.confirm(&t2.token, T0 + 5), Err(TokenError::AlreadyUsed));
}

#[test]
fn kill_switch_released_before_confirm_changes_nothing() {
    let (s, ks) = store();
    let t = s.issue(single_preview(), TokenTiming::Immediate, T0);
    ks.engage("blip", T0 + 1);
    ks.release(T0 + 2);
    // never confirmed while engaged, no revoke_all call: still valid
    assert!(s.confirm(&t.token, T0 + 3).is_ok());
}

#[test]
fn kill_switch_initially_engaged_store() {
    let ks = KillSwitch::new(true);
    let s = TokenStore::new(TTL, CAP, ks);
    let t = s.issue(single_preview(), TokenTiming::Immediate, T0);
    assert_eq!(
        s.confirm(&t.token, T0 + 1),
        Err(TokenError::KillSwitchEngaged)
    );
}

// ---- revoke_all / purge ---------------------------------------------------------

#[test]
fn revoke_all_kills_every_live_token() {
    let (s, _) = store();
    let a = s.issue(single_preview(), TokenTiming::Immediate, T0);
    let b = s.issue(batch_close_preview(2), TokenTiming::AfterReadback, T0);
    let c = s.issue(batch_close_preview(2), TokenTiming::Immediate, T0);
    let (c2, _, _) = expect_second(s.confirm(&c.token, T0 + 1).unwrap());
    s.revoke_all();
    for token in [&a.token, &b.token, &c2.token] {
        assert_eq!(
            s.confirm(token, T0 + 2),
            Err(TokenError::AlreadyUsed),
            "token {token}"
        );
    }
}

#[test]
fn revoke_all_on_empty_store_is_fine() {
    let (s, _) = store();
    s.revoke_all();
}

#[test]
fn tokens_issued_after_revoke_all_work() {
    let (s, _) = store();
    s.revoke_all();
    let t = s.issue(single_preview(), TokenTiming::Immediate, T0);
    assert!(s.confirm(&t.token, T0 + 1).is_ok());
}

#[test]
fn purge_keeps_tombstones_for_five_minutes() {
    let (s, _) = store();
    let used = s.issue(single_preview(), TokenTiming::Immediate, T0);
    s.confirm(&used.token, T0 + 1).unwrap();
    let expired = s.issue(single_preview(), TokenTiming::Immediate, T0);
    let cancelled = s.issue(single_preview(), TokenTiming::Immediate, T0);
    s.cancel(&cancelled.token, T0 + 1).unwrap();

    // just under 5 minutes past expiry: replays still classified
    let early = T0 + TTL + 299_999;
    s.purge(early);
    assert_eq!(s.confirm(&used.token, early), Err(TokenError::AlreadyUsed));
    assert_eq!(s.confirm(&expired.token, early), Err(TokenError::Expired));
    assert_eq!(
        s.cancel(&cancelled.token, early),
        Err(TokenError::AlreadyUsed)
    );

    // after 5 minutes: gone
    let late = T0 + TTL + 300_000;
    s.purge(late);
    assert_eq!(s.confirm(&used.token, late), Err(TokenError::Unknown));
    assert_eq!(s.confirm(&expired.token, late), Err(TokenError::Unknown));
    assert_eq!(s.confirm(&cancelled.token, late), Err(TokenError::Unknown));
}

#[test]
fn purge_does_not_touch_live_tokens() {
    let (s, _) = store();
    let t = s.issue(single_preview(), TokenTiming::Immediate, T0);
    s.purge(T0 + 1_000);
    assert!(s.confirm(&t.token, T0 + 2_000).is_ok());
}

#[test]
fn purge_on_empty_store_is_fine() {
    let (s, _) = store();
    s.purge(T0);
}

// ---- concurrency ------------------------------------------------------------------

#[test]
fn concurrent_confirms_execute_exactly_once() {
    let (s, _) = store();
    let s = Arc::new(s);
    let t = s.issue(single_preview(), TokenTiming::Immediate, T0);
    let handles: Vec<_> = (0..16)
        .map(|_| {
            let s = Arc::clone(&s);
            let token = t.token.clone();
            std::thread::spawn(move || s.confirm(&token, T0 + 1))
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let executed = results
        .iter()
        .filter(|r| matches!(r, Ok(ConfirmOutcome::Execute(_))))
        .count();
    let used = results
        .iter()
        .filter(|r| **r == Err(TokenError::AlreadyUsed))
        .count();
    assert_eq!(executed, 1);
    assert_eq!(used, 15);
}

#[test]
fn concurrent_batch_first_stage_issues_exactly_one_second_token() {
    let (s, _) = store();
    let s = Arc::new(s);
    let t = s.issue(batch_close_preview(3), TokenTiming::Immediate, T0);
    let handles: Vec<_> = (0..16)
        .map(|_| {
            let s = Arc::clone(&s);
            let token = t.token.clone();
            std::thread::spawn(move || s.confirm(&token, T0 + 1))
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let second = results
        .iter()
        .filter(|r| matches!(r, Ok(ConfirmOutcome::NeedsSecond { .. })))
        .count();
    assert_eq!(second, 1);
}
