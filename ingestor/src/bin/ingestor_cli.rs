use ingestor::{Ingestor, RawCommand};
use match_engine::{OrderBook, Side, OrderId};
use std::io::{self, Write};

fn main() {
    let book = OrderBook::new();
    let ig = Ingestor::start_with_book(book, 4096);

    println!("Commands: limit buy|sell <px> <qty> | market buy|sell <qty> | cancel <id> | quit");
    let stdin = io::stdin();

    // spawn printer of trades
    let rx = ig.rx_trade.clone();
    std::thread::spawn(move || {
        while let Ok(t) = rx.recv() {
            println!("trade taker={} maker={} px={} qty={}", t.taker_id.0, t.maker_id.0, t.price, t.qty);
        }
    });

    loop {
        print!("> ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        if stdin.read_line(&mut line).is_err() { break; }
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.is_empty() { continue; }
        match parts[0] {
            "quit" | "exit" => break,
            "limit" if parts.len() == 4 => {
                let side = match parts[1] { "buy" => Side::Buy, "sell" => Side::Sell, _ => { println!("side must be buy|sell"); continue; } };
                let price: u64 = match parts[2].parse() { Ok(v) => v, Err(_) => { println!("invalid price"); continue; } };
                let qty: u64 = match parts[3].parse() { Ok(v) => v, Err(_) => { println!("invalid qty"); continue; } };
                let _ = ig.tx_cmd.send(RawCommand::Limit { side, price, qty });
            }
            "market" if parts.len() == 3 => {
                let side = match parts[1] { "buy" => Side::Buy, "sell" => Side::Sell, _ => { println!("side must be buy|sell"); continue; } };
                let qty: u64 = match parts[2].parse() { Ok(v) => v, Err(_) => { println!("invalid qty"); continue; } };
                let _ = ig.tx_cmd.send(RawCommand::Market { side, qty });
            }
            "cancel" if parts.len() == 2 => {
                let id = match parts[1].parse::<u64>() { Ok(v) => OrderId(v), Err(_) => { println!("invalid id"); continue; } };
                let _ = ig.tx_cmd.send(RawCommand::Cancel { id });
            }
            _ => println!("unknown command"),
        }
    }
}
