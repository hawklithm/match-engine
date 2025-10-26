use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput, black_box};
use match_engine::{OrderBook, Side};

fn setup_book(levels: usize, base_price: u64, tick: u64, qty_per_level: u64) -> OrderBook {
    let mut ob = OrderBook::new();
    for i in 1..=levels as u64 {
        let bid_px = base_price.saturating_sub(i * tick);
        let ask_px = base_price + i * tick;
        let _ = ob.submit_limit(Side::Buy, bid_px, qty_per_level);
        let _ = ob.submit_limit(Side::Sell, ask_px, qty_per_level);
    }
    ob
}

fn gen_price(i: u64, base: u64, tick: u64, span: u64, _side: Side) -> u64 {
    let offset = (i % (2 * span + 1)) as i64 - span as i64;
    let px = base as i64 + offset * tick as i64;
    px.max(1) as u64
}

fn bench_limit_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("limit_throughput");
    for &orders in &[10_000u64, 50_000u64, 100_000u64] {
        group.throughput(Throughput::Elements(orders));
        group.bench_with_input(BenchmarkId::from_parameter(orders), &orders, |b, &n| {
            b.iter_batched(
                || setup_book(200, 10_000, 1, 1_000),
                |mut ob| {
                    let base = 10_000u64;
                    for i in 0..n {
                        let side = if i % 2 == 0 { Side::Buy } else { Side::Sell };
                        let cross = i % 10 < 3;
                        let px = match side {
                            Side::Buy => {
                                let bb = ob.best_bid().map(|(p, _)| p).unwrap_or(base - 10);
                                if cross { ob.best_ask().map(|(p, _)| p).unwrap_or(base + 1) }
                                else { gen_price(i, bb.saturating_sub(5), 1, 3, side) }
                            }
                            Side::Sell => {
                                let ba = ob.best_ask().map(|(p, _)| p).unwrap_or(base + 10);
                                if cross { ob.best_bid().map(|(p, _)| p).unwrap_or(base - 1) }
                                else { gen_price(i, ba + 5, 1, 3, side) }
                            }
                        };
                        let qty = 1 + (i % 5);
                        let _ = ob.submit_limit(side, black_box(px), black_box(qty));
                    }
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_market_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("market_throughput");
    for &orders in &[10_000u64, 50_000u64] {
        group.throughput(Throughput::Elements(orders));
        group.bench_with_input(BenchmarkId::from_parameter(orders), &orders, |b, &n| {
            b.iter_batched(
                || setup_book(500, 10_000, 1, 2_000),
                |mut ob| {
                    for i in 0..n {
                        let side = if i % 2 == 0 { Side::Buy } else { Side::Sell };
                        let qty = 1 + (i % 10);
                        let _ = ob.submit_market(side, black_box(qty));
                    }
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, bench_limit_throughput, bench_market_throughput);
criterion_main!(benches);
