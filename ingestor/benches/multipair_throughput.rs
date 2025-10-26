use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use ingestor::{MultiIngestor, RawCommand};
use match_engine::{OrderBook, Side};
use crossbeam_channel as cb;
use std::thread;

fn make_books(symbols: &[&str], levels: usize, base_price: u64, tick: u64, qty_per_level: u64) -> Vec<(String, OrderBook)> {
    let mut v = Vec::with_capacity(symbols.len());
    for &sym in symbols {
        let mut ob = OrderBook::new();
        for i in 1..=levels as u64 {
            let bid_px = base_price.saturating_sub(i * tick);
            let ask_px = base_price + i * tick;
            let _ = ob.submit_limit(Side::Buy, bid_px, qty_per_level);
            let _ = ob.submit_limit(Side::Sell, ask_px, qty_per_level);
        }
        v.push((sym.to_string(), ob));
    }
    v
}

fn spawn_symbol_producer(tx: cb::Sender<RawCommand>, idx: usize, total_orders: u64, pairs: usize) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        // Each producer generates a disjoint subsequence interleaved by symbol index
        // i iterates all orders but only sends those assigned to this symbol to avoid extra modulo work
        let mut sent = 0u64;
        let mut i = idx as u64;
        while sent < total_orders {
            let side = if i % 2 == 0 { Side::Buy } else { Side::Sell };
            let cmd = if i % 10 < 3 {
                RawCommand::Limit { side, price: 10_000, qty: 1 + (i % 5) }
            } else {
                RawCommand::Market { side, qty: 1 + (i % 5) }
            };
            let _ = tx.send(cmd);
            sent += 1;
            i += pairs as u64;
        }
    })
}

fn bench_multipair(c: &mut Criterion) {
    let mut group = c.benchmark_group("multipair_throughput");
    for &(pairs, orders) in &[(2usize, 50_000u64), (4, 100_000), (8, 200_000)] {
        group.throughput(Throughput::Elements(orders));
        group.bench_with_input(BenchmarkId::from_parameter(format!("{}pairs_{}orders", pairs, orders)), &orders, |b, &n| {
            b.iter_batched(
                || {
                    let symbols: Vec<String> = (0..pairs).map(|i| format!("SYM{}", i)).collect();
                    let books = make_books(&symbols.iter().map(|s| s.as_str()).collect::<Vec<_>>(), 200, 10_000, 1, 1_000);
                    // Use larger batch and disable trade emission to remove per-trade send overhead
                    let ig = MultiIngestor::start_with_books_with_opts(books, 16_384, false);
                    (symbols, ig)
                },
                |(symbols, ig)| {
                    // Spawn one producer per symbol and send directly to per-symbol route (bypass router)
                    let mut joins = Vec::with_capacity(symbols.len());
                    for (idx, sym) in symbols.iter().enumerate() {
                        let tx = ig.routes.get(sym).unwrap().clone();
                        joins.push(spawn_symbol_producer(tx, idx, n / symbols.len() as u64, symbols.len()));
                    }
                    // Wait until all orders processed
                    let mut done = 0u64;
                    while done < n {
                        if let Ok(batch_done) = ig.rx_done.recv() { done += batch_done as u64; }
                    }
                    for j in joins { let _ = j.join(); }
                    // No trade drain needed (we disabled emission)
                },
                BatchSize::PerIteration,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, bench_multipair);
criterion_main!(benches);
