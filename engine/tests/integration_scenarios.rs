use match_engine::{OrderBook, Side};
use std::time::Instant;

fn seed_book(ob: &mut OrderBook, mid: u64, levels: u64, tick: u64, qty: u64) {
    for i in 1..=levels {
        let bid_px = mid.saturating_sub(i * tick);
        let ask_px = mid + i * tick;
        let _ = ob.submit_limit(Side::Buy, bid_px, qty);
        let _ = ob.submit_limit(Side::Sell, ask_px, qty);
    }
}

#[test]
fn limit_crossing_fifo_and_rest() {
    let mut ob = OrderBook::new();
    let (a1, _, _) = ob.submit_limit(Side::Sell, 100, 3);
    let (a2, _, _) = ob.submit_limit(Side::Sell, 100, 4);
    let (_b, trades, r) = ob.submit_limit(Side::Buy, 105, 5);
    assert_eq!(r, 0);
    assert_eq!(trades.len(), 2);
    assert_eq!(trades[0].maker_id.0, a1.0);
    assert_eq!(trades[0].qty, 3);
    assert_eq!(trades[1].maker_id.0, a2.0);
    assert_eq!(trades[1].qty, 2);
    assert_eq!(ob.best_ask().unwrap().0, 100);
    assert_eq!(ob.best_ask().unwrap().1, 2);
}

#[test]
fn partial_fill_then_rest_bid() {
    let mut ob = OrderBook::new();
    let _ = ob.submit_limit(Side::Sell, 101, 2);
    let (bid_id, trades, remaining) = ob.submit_limit(Side::Buy, 101, 5);
    assert_eq!(trades.iter().map(|t| t.qty).sum::<u64>(), 2);
    assert_eq!(remaining, 3);
    let (px, qty) = ob.best_bid().unwrap();
    assert_eq!(px, 101);
    assert_eq!(qty, 3);
    let canceled = ob.cancel(bid_id).expect("cancel ok");
    assert_eq!(canceled.qty, 3);
}

#[test]
fn market_consumes_across_levels() {
    let mut ob = OrderBook::new();
    seed_book(&mut ob, 10_000, 3, 1, 5);
    let _ = ob.submit_limit(Side::Sell, 9_998, 2);
    let (px, qty) = ob.best_ask().unwrap();
    assert_eq!(px, 10_001);
    assert_eq!(qty, 5);
    let (_id, trades, r) = ob.submit_market(Side::Buy, 9);
    assert_eq!(r, 0);
    assert_eq!(trades.iter().map(|t| t.qty).sum::<u64>(), 9);
    let (px, qty) = ob.best_ask().unwrap();
    assert_eq!(px, 10_002);
    assert_eq!(qty, 1);
}

#[test]
fn top_n_depth_is_aggregated() {
    let mut ob = OrderBook::new();
    let _ = ob.submit_limit(Side::Buy, 100, 1);
    let _ = ob.submit_limit(Side::Buy, 100, 2);
    let _ = ob.submit_limit(Side::Buy, 99, 4);
    let _ = ob.submit_limit(Side::Sell, 101, 3);
    let _ = ob.submit_limit(Side::Sell, 102, 6);
    let (bids, asks) = ob.top_n(2);
    assert_eq!(bids, vec![(100, 3), (99, 4)]);
    assert_eq!(asks, vec![(101, 3), (102, 6)]);
}

#[test]
fn cancel_unknown_returns_error() {
    let mut ob = OrderBook::new();
    let err = ob.cancel(match_engine::OrderId(42)).unwrap_err();
    assert!(format!("{}", err).contains("unknown order id"));
}

#[test]
fn load_scenario_smoke_and_throughput_print() {
    let mut ob = OrderBook::new();
    seed_book(&mut ob, 10_000, 200, 1, 1_000);

    let n: u64 = 50_000;
    let start = Instant::now();
    for i in 0..n {
        if i % 10 == 0 {
            let side = if i % 2 == 0 { Side::Buy } else { Side::Sell };
            let _ = ob.submit_market(side, 1 + (i % 7));
        } else {
            let side = if i % 2 == 0 { Side::Buy } else { Side::Sell };
            if side == Side::Buy {
                if let Some((ask, _)) = ob.best_ask() { let _ = ob.submit_limit(side, ask, 1 + (i % 3)); }
                else { let _ = ob.submit_limit(side, 9_999, 1); }
            } else {
                if let Some((bid, _)) = ob.best_bid() { let _ = ob.submit_limit(side, bid, 1 + (i % 3)); }
                else { let _ = ob.submit_limit(side, 10_001, 1); }
            }
        }
        if i % 777 == 0 {
            if let Some((px, _)) = ob.best_bid() {
                let (id, _, r) = ob.submit_limit(Side::Buy, px, 2);
                if r > 0 { let _ = ob.cancel(id); }
            }
        }
    }
    let dur = start.elapsed().as_secs_f64();
    let ops_per_sec = (n as f64) / dur;
    eprintln!("load_scenario: ops={} duration={:.3}s ops/sec≈{:.0}", n, dur, ops_per_sec);
    let bb = ob.best_bid();
    let ba = ob.best_ask();
    assert!(bb.is_some() || ba.is_some());
}
