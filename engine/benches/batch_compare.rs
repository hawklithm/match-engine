use criterion::{black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use match_engine::{Command, OrderBook, Side};

fn seed_book(levels: usize, base_price: u64, tick: u64, qty_per_level: u64) -> OrderBook {
    let mut ob = OrderBook::new();
    for i in 1..=levels as u64 {
        let bid_px = base_price.saturating_sub(i * tick);
        let ask_px = base_price + i * tick;
        let _ = ob.submit_limit(Side::Buy, bid_px, qty_per_level);
        let _ = ob.submit_limit(Side::Sell, ask_px, qty_per_level);
    }
    ob
}

fn build_limit_cmds(n: u64, base_px: u64) -> Vec<Command> {
    let mut cmds = Vec::with_capacity(n as usize);
    for i in 0..n {
        let side = if i % 2 == 0 { Side::Buy } else { Side::Sell };
        let cross = i % 10 < 3;
        let px = if side == Side::Buy {
            if cross { base_px + 1 } else { base_px.saturating_sub(5) }
        } else {
            if cross { base_px.saturating_sub(1) } else { base_px + 5 }
        };
        let qty = 1 + (i % 5);
        cmds.push(Command::Limit { seq: i, side, price: px, qty });
    }
    cmds
}

fn bench_batch_compare(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_compare");
    for &orders in &[10_000u64, 50_000u64] {
        group.throughput(Throughput::Elements(orders));

        // Single call mode
        group.bench_with_input(BenchmarkId::new("single_submit", orders), &orders, |b, &n| {
            b.iter_batched(
                || seed_book(200, 10_000, 1, 1_000),
                |mut ob| {
                    for i in 0..n {
                        let side = if i % 2 == 0 { Side::Buy } else { Side::Sell };
                        let px = if side == Side::Buy { 9_995 } else { 10_005 };
                        let qty = 1 + (i % 5);
                        let _ = ob.submit_limit(side, black_box(px), black_box(qty));
                    }
                },
                BatchSize::LargeInput,
            );
        });

        // Zero-allocation into mode
        group.bench_with_input(BenchmarkId::new("into_submit", orders), &orders, |b, &n| {
            b.iter_batched(
                || seed_book(200, 10_000, 1, 1_000),
                |mut ob| {
                    let mut trades = Vec::with_capacity(1024);
                    for i in 0..n {
                        let side = if i % 2 == 0 { Side::Buy } else { Side::Sell };
                        let px = if side == Side::Buy { 9_995 } else { 10_005 };
                        let qty = 1 + (i % 5);
                        let _ = ob.submit_limit_into(side, black_box(px), black_box(qty), &mut trades);
                    }
                    black_box(trades);
                },
                BatchSize::LargeInput,
            );
        });

        // Batch mode with checked seq ordering
        group.bench_with_input(BenchmarkId::new("batch_checked", orders), &orders, |b, &n| {
            b.iter_batched(
                || {
                    let ob = seed_book(200, 10_000, 1, 1_000);
                    let cmds = build_limit_cmds(n, 10_000);
                    (ob, cmds)
                },
                |(mut ob, mut cmds)| {
                    let mut trades = Vec::with_capacity((n as usize).min(4096));
                    let _ = ob.process_commands_batch_checked_into(&mut cmds, &mut trades);
                    black_box(trades);
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, bench_batch_compare);
criterion_main!(benches);
