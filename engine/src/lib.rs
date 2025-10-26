use std::collections::{BTreeMap, HashMap, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Limit { seq: u64, side: Side, price: u64, qty: u64 },
    Market { seq: u64, side: Side, qty: u64 },
    Cancel { seq: u64, id: OrderId },
}

impl OrderBook {
    pub fn process_commands_batch_checked_into(
        &mut self,
        cmds: &mut [Command],
        trades_out: &mut Vec<Trade>,
    ) -> Result<Vec<(OrderId, u64)>, EngineError> {
        // Ensure strict increasing seq; if not sorted, sort by seq stably.
        let is_sorted = cmds.windows(2).all(|w| seq_of(&w[0]) < seq_of(&w[1]));
        if !is_sorted {
            cmds.sort_by_key(|c| seq_of(c));
        }
        // After sort, check for duplicates
        if cmds.windows(2).any(|w| seq_of(&w[0]) >= seq_of(&w[1])) {
            return Err(EngineError::InvalidSequence);
        }
        let mut results = Vec::with_capacity(cmds.len());
        for &cmd in cmds.iter() {
            match cmd {
                Command::Limit { side, price, qty, .. } => {
                    let start_len = trades_out.len();
                    let (id, remaining) = self.submit_limit_into(side, price, qty, trades_out);
                    let _ = trades_out.len() - start_len;
                    results.push((id, remaining));
                }
                Command::Market { side, qty, .. } => {
                    let start_len = trades_out.len();
                    let (id, remaining) = self.submit_market_into(side, qty, trades_out);
                    let _ = trades_out.len() - start_len;
                    results.push((id, remaining));
                }
                Command::Cancel { id, .. } => {
                    match self.cancel(id) {
                        Ok(_o) => results.push((id, 0)),
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        Ok(results)
    }

    pub fn process_commands_batch_into(
        &mut self,
        cmds: &[Command],
        trades_out: &mut Vec<Trade>,
    ) -> Vec<Result<(OrderId, u64), EngineError>> {
        // Backward-friendly wrapper: copy slice to a Vec, then call checked variant.
        let mut owned: Vec<Command> = cmds.to_vec();
        match self.process_commands_batch_checked_into(&mut owned, trades_out) {
            Ok(res) => res.into_iter().map(Ok).collect(),
            Err(e) => vec![Err(e)],
        }
    }
}

#[inline]
fn seq_of(c: &Command) -> u64 {
    match *c {
        Command::Limit { seq, .. } => seq,
        Command::Market { seq, .. } => seq,
        Command::Cancel { seq, .. } => seq,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrderType {
    Limit,
    Market,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OrderId(pub u64);

#[derive(Debug, Clone)]
pub struct Order {
    pub id: OrderId,
    pub side: Side,
    pub price: u64,
    pub qty: u64,
    pub order_type: OrderType,
    pub ts: u64,
}

#[derive(Debug, Clone)]
pub struct Trade {
    pub taker_id: OrderId,
    pub maker_id: OrderId,
    pub price: u64,
    pub qty: u64,
}

#[derive(thiserror::Error, Debug)]
pub enum EngineError {
    #[error("unknown order id")]
    UnknownOrder,
    #[error("invalid side for operation")]
    InvalidSide,
    #[error("invalid sequence in batch")] 
    InvalidSequence,
}

#[derive(Default)]
pub struct OrderBook {
    bids: BTreeMap<u64, VecDeque<Order>>, // price -> fifo
    asks: BTreeMap<u64, VecDeque<Order>>, // price -> fifo
    index: HashMap<u64, (Side, u64)>,     // id -> (side, price)
    next_id: u64,
    ts: u64,
}

impl OrderBook {
    pub fn new() -> Self { Self::default() }

    pub fn now(&mut self) -> u64 { self.ts += 1; self.ts }

    pub fn next_order_id(&mut self) -> OrderId { self.next_id += 1; OrderId(self.next_id) }

    pub fn submit_limit(&mut self, side: Side, price: u64, qty: u64) -> (OrderId, Vec<Trade>, u64) {
        let id = self.next_order_id();
        let ts = self.now();
        let mut trades = Vec::new();
        let mut remaining = qty;
        match side {
            Side::Buy => {
                loop {
                    if remaining == 0 { break; }
                    let p_opt = self.asks.first_key_value().map(|(p, _)| *p);
                    let p = match p_opt { Some(p) if p <= price => p, _ => break };
                    if let Some(queue) = self.asks.get_mut(&p) {
                        while remaining > 0 {
                            if let Some(maker) = queue.front_mut() {
                                let trade_qty = remaining.min(maker.qty);
                                trades.push(Trade { taker_id: id, maker_id: maker.id, price: p, qty: trade_qty });
                                maker.qty -= trade_qty;
                                remaining -= trade_qty;
                                if maker.qty == 0 { queue.pop_front(); } else { break; }
                            } else { break; }
                        }
                        if queue.is_empty() { self.asks.remove(&p); }
                    } else { break; }
                }
                if remaining > 0 {
                    let o = Order { id, side, price, qty: remaining, order_type: OrderType::Limit, ts };
                    self.bids.entry(price).or_default().push_back(o);
                    self.index.insert(id.0, (Side::Buy, price));
                }
            }
            Side::Sell => {
                loop {
                    if remaining == 0 { break; }
                    let p_opt = self.bids.last_key_value().map(|(p, _)| *p);
                    let p = match p_opt { Some(p) if p >= price => p, _ => break };
                    if let Some(queue) = self.bids.get_mut(&p) {
                        while remaining > 0 {
                            if let Some(maker) = queue.front_mut() {
                                let trade_qty = remaining.min(maker.qty);
                                trades.push(Trade { taker_id: id, maker_id: maker.id, price: p, qty: trade_qty });
                                maker.qty -= trade_qty;
                                remaining -= trade_qty;
                                if maker.qty == 0 { queue.pop_front(); } else { break; }
                            } else { break; }
                        }
                        if queue.is_empty() { self.bids.remove(&p); }
                    } else { break; }
                }
                if remaining > 0 {
                    let o = Order { id, side, price, qty: remaining, order_type: OrderType::Limit, ts };
                    self.asks.entry(price).or_default().push_back(o);
                    self.index.insert(id.0, (Side::Sell, price));
                }
            }
        }
        (id, trades, remaining)
    }

    pub fn submit_market(&mut self, side: Side, qty: u64) -> (OrderId, Vec<Trade>, u64) {
        let id = self.next_order_id();
        let _ts = self.now();
        let mut trades = Vec::new();
        let mut remaining = qty;
        match side {
            Side::Buy => {
                loop {
                    if remaining == 0 { break; }
                    let p_opt = self.asks.first_key_value().map(|(p, _)| *p);
                    let p = match p_opt { Some(p) => p, None => break };
                    if let Some(queue) = self.asks.get_mut(&p) {
                        while remaining > 0 {
                            if let Some(maker) = queue.front_mut() {
                                let trade_qty = remaining.min(maker.qty);
                                trades.push(Trade { taker_id: id, maker_id: maker.id, price: p, qty: trade_qty });
                                maker.qty -= trade_qty;
                                remaining -= trade_qty;
                                if maker.qty == 0 { queue.pop_front(); } else { break; }
                            } else { break; }
                        }
                        if queue.is_empty() { self.asks.remove(&p); }
                    } else { break; }
                }
            }
            Side::Sell => {
                loop {
                    if remaining == 0 { break; }
                    let p_opt = self.bids.last_key_value().map(|(p, _)| *p);
                    let p = match p_opt { Some(p) => p, None => break };
                    if let Some(queue) = self.bids.get_mut(&p) {
                        while remaining > 0 {
                            if let Some(maker) = queue.front_mut() {
                                let trade_qty = remaining.min(maker.qty);
                                trades.push(Trade { taker_id: id, maker_id: maker.id, price: p, qty: trade_qty });
                                maker.qty -= trade_qty;
                                remaining -= trade_qty;
                                if maker.qty == 0 { queue.pop_front(); } else { break; }
                            } else { break; }
                        }
                        if queue.is_empty() { self.bids.remove(&p); }
                    } else { break; }
                }
            }
        }
        (id, trades, remaining)
    }

    // Zero-allocation variants
    pub fn submit_limit_into(&mut self, side: Side, price: u64, qty: u64, trades_out: &mut Vec<Trade>) -> (OrderId, u64) {
        let id = self.next_order_id();
        let ts = self.now();
        let mut remaining = qty;
        match side {
            Side::Buy => {
                loop {
                    if remaining == 0 { break; }
                    let p_opt = self.asks.first_key_value().map(|(p, _)| *p);
                    let p = match p_opt { Some(p) if p <= price => p, _ => break };
                    if let Some(queue) = self.asks.get_mut(&p) {
                        while remaining > 0 {
                            if let Some(maker) = queue.front_mut() {
                                let trade_qty = remaining.min(maker.qty);
                                trades_out.push(Trade { taker_id: id, maker_id: maker.id, price: p, qty: trade_qty });
                                maker.qty -= trade_qty;
                                remaining -= trade_qty;
                                if maker.qty == 0 { queue.pop_front(); } else { break; }
                            } else { break; }
                        }
                        if queue.is_empty() { self.asks.remove(&p); }
                    } else { break; }
                }
                if remaining > 0 {
                    let o = Order { id, side, price, qty: remaining, order_type: OrderType::Limit, ts };
                    self.bids.entry(price).or_default().push_back(o);
                    self.index.insert(id.0, (Side::Buy, price));
                }
            }
            Side::Sell => {
                loop {
                    if remaining == 0 { break; }
                    let p_opt = self.bids.last_key_value().map(|(p, _)| *p);
                    let p = match p_opt { Some(p) if p >= price => p, _ => break };
                    if let Some(queue) = self.bids.get_mut(&p) {
                        while remaining > 0 {
                            if let Some(maker) = queue.front_mut() {
                                let trade_qty = remaining.min(maker.qty);
                                trades_out.push(Trade { taker_id: id, maker_id: maker.id, price: p, qty: trade_qty });
                                maker.qty -= trade_qty;
                                remaining -= trade_qty;
                                if maker.qty == 0 { queue.pop_front(); } else { break; }
                            } else { break; }
                        }
                        if queue.is_empty() { self.bids.remove(&p); }
                    } else { break; }
                }
                if remaining > 0 {
                    let o = Order { id, side, price, qty: remaining, order_type: OrderType::Limit, ts };
                    self.asks.entry(price).or_default().push_back(o);
                    self.index.insert(id.0, (Side::Sell, price));
                }
            }
        }
        (id, remaining)
    }

    pub fn submit_market_into(&mut self, side: Side, qty: u64, trades_out: &mut Vec<Trade>) -> (OrderId, u64) {
        let id = self.next_order_id();
        let _ts = self.now();
        let mut remaining = qty;
        match side {
            Side::Buy => {
                loop {
                    if remaining == 0 { break; }
                    let p_opt = self.asks.first_key_value().map(|(p, _)| *p);
                    let p = match p_opt { Some(p) => p, None => break };
                    if let Some(queue) = self.asks.get_mut(&p) {
                        while remaining > 0 {
                            if let Some(maker) = queue.front_mut() {
                                let trade_qty = remaining.min(maker.qty);
                                trades_out.push(Trade { taker_id: id, maker_id: maker.id, price: p, qty: trade_qty });
                                maker.qty -= trade_qty;
                                remaining -= trade_qty;
                                if maker.qty == 0 { queue.pop_front(); } else { break; }
                            } else { break; }
                        }
                        if queue.is_empty() { self.asks.remove(&p); }
                    } else { break; }
                }
            }
            Side::Sell => {
                loop {
                    if remaining == 0 { break; }
                    let p_opt = self.bids.last_key_value().map(|(p, _)| *p);
                    let p = match p_opt { Some(p) => p, None => break };
                    if let Some(queue) = self.bids.get_mut(&p) {
                        while remaining > 0 {
                            if let Some(maker) = queue.front_mut() {
                                let trade_qty = remaining.min(maker.qty);
                                trades_out.push(Trade { taker_id: id, maker_id: maker.id, price: p, qty: trade_qty });
                                maker.qty -= trade_qty;
                                remaining -= trade_qty;
                                if maker.qty == 0 { queue.pop_front(); } else { break; }
                            } else { break; }
                        }
                        if queue.is_empty() { self.bids.remove(&p); }
                    } else { break; }
                }
            }
        }
        (id, remaining)
    }

    // Simple batch API to reduce call overhead
    pub fn submit_limits_batch(&mut self, orders: &[(Side, u64, u64)], trades_out: &mut Vec<Trade>) {
        for &(side, price, qty) in orders { let _ = self.submit_limit_into(side, price, qty, trades_out); }
    }

    pub fn cancel(&mut self, id: OrderId) -> Result<Order, EngineError> {
        if let Some((side, price)) = self.index.remove(&id.0) {
            let book = match side { Side::Buy => &mut self.bids, Side::Sell => &mut self.asks };
            if let Some(queue) = book.get_mut(&price) {
                let mut idx = None;
                for (i, o) in queue.iter().enumerate() { if o.id == id { idx = Some(i); break; } }
                if let Some(i) = idx {
                    let o = queue.remove(i).unwrap();
                    if queue.is_empty() { book.remove(&price); }
                    return Ok(o);
                }
            }
        }
        Err(EngineError::UnknownOrder)
    }

    pub fn best_bid(&self) -> Option<(u64, u64)> {
        self.bids.iter().rev().next().map(|(p, q)| (*p, q.iter().map(|o| o.qty).sum()))
    }
    pub fn best_ask(&self) -> Option<(u64, u64)> {
        self.asks.iter().next().map(|(p, q)| (*p, q.iter().map(|o| o.qty).sum()))
    }
    pub fn top_n(&self, n: usize) -> (Vec<(u64, u64)>, Vec<(u64, u64)>) {
        let bids = self.bids.iter().rev().take(n).map(|(p, q)| (*p, q.iter().map(|o| o.qty).sum())).collect();
        let asks = self.asks.iter().take(n).map(|(p, q)| (*p, q.iter().map(|o| o.qty).sum())).collect();
        (bids, asks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limit_crosses() {
        let mut ob = OrderBook::new();
        let (_a, _t, _r) = ob.submit_limit(Side::Sell, 101, 5);
        let (_b, trades, r) = ob.submit_limit(Side::Buy, 105, 7);
        assert_eq!(trades.iter().map(|t| t.qty).sum::<u64>(), 5);
        assert_eq!(r, 2);
        assert_eq!(ob.best_bid().unwrap().0, 105);
    }
    #[test]
    fn market_consumes() {
        let mut ob = OrderBook::new();
        let (_a, _t, _r) = ob.submit_limit(Side::Sell, 100, 3);
        let (_b, _t2, _r2) = ob.submit_limit(Side::Sell, 101, 3);
        let (_m, trades, r) = ob.submit_market(Side::Buy, 5);
        assert_eq!(trades.iter().map(|t| t.qty).sum::<u64>(), 5);
        assert_eq!(r, 0);
        assert_eq!(ob.best_ask().unwrap().0, 101);
        assert_eq!(ob.best_ask().unwrap().1, 1);
    }
}
