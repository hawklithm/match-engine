use crossbeam_channel as cb;
use crossbeam_channel::{Receiver, Sender};
use match_engine::{Command, OrderBook, Trade};
use std::collections::HashMap;
use std::time::Duration;

// External producers send unsequenced commands; ingestor assigns seq to guarantee global order
#[derive(Debug, Clone, Copy)]
pub enum RawCommand {
    Limit { side: match_engine::Side, price: u64, qty: u64 },
    Market { side: match_engine::Side, qty: u64 },
    Cancel { id: match_engine::OrderId },
}

// Multi-symbol API
#[derive(Debug, Clone)]
pub struct MultiRawCommand {
    pub symbol: String,
    pub cmd: RawCommand,
}

pub struct MultiIngestor {
    pub tx_cmd: Sender<MultiRawCommand>,
    pub rx_trade: Receiver<(String, Trade)>,
    pub rx_done: Receiver<usize>, // number of commands processed in a batch across workers
    pub routes: HashMap<String, Sender<RawCommand>>, // direct per-symbol senders
}

impl MultiIngestor {
    pub fn start_with_books(books: Vec<(String, OrderBook)>, batch_size: usize) -> Self {
        Self::start_with_books_with_opts(books, batch_size, true)
    }

    pub fn start_with_books_with_opts(books: Vec<(String, OrderBook)>, batch_size: usize, emit_trades: bool) -> Self {
        let opts = Options { batch_size, emit_trades, coalesce_micros: 0 };
        Self::start_with_books_with_config(books, opts)
    }

    pub fn start_with_books_with_config(books: Vec<(String, OrderBook)>, opts: Options) -> Self {
        let (tx_cmd, rx_cmd) = cb::unbounded::<MultiRawCommand>();
        let (tx_trade_all, rx_trade) = cb::unbounded::<(String, Trade)>();
        let (tx_done_all, rx_done) = cb::unbounded::<usize>();

        // Create per-symbol workers and a router
        let mut routes: HashMap<String, Sender<RawCommand>> = HashMap::new();
        for (symbol, book) in books {
            let (tx_raw, rx_raw) = cb::unbounded::<RawCommand>();
            routes.insert(symbol.clone(), tx_raw.clone());
            let tx_trade_all = tx_trade_all.clone();
            let tx_done_all = tx_done_all.clone();
            std::thread::spawn(move || {
                let mut book = book; // move in
                let mut trades_buf: Vec<Trade> = Vec::with_capacity(opts.batch_size * 2);
                let mut batch_raw: Vec<RawCommand> = Vec::with_capacity(opts.batch_size);
                let mut batch: Vec<Command> = Vec::with_capacity(opts.batch_size);
                let mut seq: u64 = 0;
                loop {
                    batch_raw.clear();
                    match rx_raw.recv() { Ok(cmd) => batch_raw.push(cmd), Err(_) => break }
                    // Coalesce additional messages to fill batch or until timeout
                    if opts.coalesce_micros > 0 {
                        let timeout = Duration::from_micros(opts.coalesce_micros as u64);
                        while batch_raw.len() < opts.batch_size {
                            match rx_raw.recv_timeout(timeout) {
                                Ok(cmd) => batch_raw.push(cmd),
                                Err(cb::RecvTimeoutError::Timeout) => break,
                                Err(cb::RecvTimeoutError::Disconnected) => break,
                            }
                        }
                    } else {
                        while batch_raw.len() < opts.batch_size {
                            match rx_raw.try_recv() {
                                Ok(cmd) => batch_raw.push(cmd),
                                Err(cb::TryRecvError::Empty) => break,
                                Err(cb::TryRecvError::Disconnected) => break,
                            }
                        }
                    }
                    batch.clear();
                    for rc in batch_raw.iter().copied() {
                        let s = seq; seq = seq.wrapping_add(1);
                        batch.push(match rc {
                            RawCommand::Limit { side, price, qty } => Command::Limit { seq: s, side, price, qty },
                            RawCommand::Market { side, qty } => Command::Market { seq: s, side, qty },
                            RawCommand::Cancel { id } => Command::Cancel { seq: s, id },
                        });
                    }
                    let start_len = trades_buf.len();
                    let _ = book.process_commands_batch_checked_into(&mut batch, &mut trades_buf);
                    let produced = trades_buf.len() - start_len;
                    if opts.emit_trades {
                        if produced > 0 {
                            // send tagged trades
                            for t in trades_buf.drain(start_len..) {
                                let _ = tx_trade_all.send((symbol.clone(), t));
                            }
                        }
                    } else {
                        // just drop drained trades to avoid per-trade send overhead
                        trades_buf.truncate(start_len);
                    }
                    // notify done by number of commands processed
                    let _ = tx_done_all.send(batch.len());
                }
            });
        }

        let router_routes = routes.clone();
        std::thread::spawn(move || {
            // Router thread: dispatch by symbol
            while let Ok(mcmd) = rx_cmd.recv() {
                if let Some(tx_raw) = router_routes.get(&mcmd.symbol) {
                    let _ = tx_raw.send(mcmd.cmd);
                }
            }
        });

        Self { tx_cmd, rx_trade, rx_done, routes }
    }
}

#[derive(Clone, Copy)]
pub struct Options {
    pub batch_size: usize,
    pub emit_trades: bool,
    pub coalesce_micros: u32,
}

pub struct Ingestor {
    pub tx_cmd: Sender<RawCommand>,
    pub rx_trade: Receiver<Trade>,
}

impl Ingestor {
    pub fn start_with_book(mut book: OrderBook, batch_size: usize) -> Self {
        let (tx_cmd, rx_cmd) = cb::unbounded::<RawCommand>();
        let (tx_trade, rx_trade) = cb::unbounded::<Trade>();

        std::thread::spawn(move || {
            let mut trades_buf: Vec<Trade> = Vec::with_capacity(batch_size * 2);
            let mut batch_raw: Vec<RawCommand> = Vec::with_capacity(batch_size);
            let mut batch: Vec<Command> = Vec::with_capacity(batch_size);
            let mut seq: u64 = 0;
            loop {
                batch_raw.clear();
                // blocking take one to avoid busy loop
                match rx_cmd.recv() {
                    Ok(cmd) => batch_raw.push(cmd),
                    Err(_) => break,
                }
                // try to fill batch without blocking
                while batch_raw.len() < batch_size {
                    match rx_cmd.try_recv() {
                        Ok(cmd) => batch_raw.push(cmd),
                        Err(cb::TryRecvError::Empty) => break,
                        Err(cb::TryRecvError::Disconnected) => break,
                    }
                }
                // assign seq and convert to engine Command
                batch.clear();
                for rc in batch_raw.iter().copied() {
                    let s = seq; seq = seq.wrapping_add(1);
                    batch.push(match rc {
                        RawCommand::Limit { side, price, qty } => Command::Limit { seq: s, side, price, qty },
                        RawCommand::Market { side, qty } => Command::Market { seq: s, side, qty },
                        RawCommand::Cancel { id } => Command::Cancel { seq: s, id },
                    });
                }
                let start_len = trades_buf.len();
                let _ = book.process_commands_batch_checked_into(&mut batch, &mut trades_buf);
                for t in trades_buf.drain(start_len..) {
                    let _ = tx_trade.send(t);
                }
            }
        });

        Self { tx_cmd, rx_trade }
    }
}
