# match-engine

单机内存撮合引擎（Rust）。工程采用 Cargo workspace，包含：

- engine：撮合核心库（单簿）。
- ingestor：并发入站指令的通道化层，支持单簿与多交易对路由，以及 CLI 与基准测试。

## 功能特性

- **价格优先、时间优先（FIFO）**：内部使用 BTreeMap(price) + VecDeque(FIFO)。
- **限价/市价/撤单**：支持三种基本指令，返回生成的订单 ID、成交明细及剩余数量。
- **零分配路径**：提供 `*_into` API，将成交写入外部 `Vec<Trade>`，减少分配/拷贝。
- **批处理接口**：`process_commands_batch_checked_into` 支持带 `seq` 的严格顺序校验与稳定排序，便于强一致重放。
- **可重放/强一致**：同一序列的 `Command` 在任何单机顺序处理结果一致；多 server 可依赖 `seq` 全局单调保证跨机一致。

## 目录结构

- engine
  - src/lib.rs：核心数据结构与 API
  - benches/throughput.rs：单簿基准（限价/市价吞吐）
  - benches/batch_compare.rs：单条 vs 零分配 vs 批处理对比
  - tests/integration_scenarios.rs：集成测试
- ingestor
  - src/lib.rs：单簿 `Ingestor` 与多簿 `MultiIngestor` 路由
  - src/bin/ingestor_cli.rs：交互式 CLI 示例
  - benches/multipair_throughput.rs：多交易对吞吐基准

## 引擎 API（engine）

- 订单方向：`Side::{Buy, Sell}`
- 订单/成交类型：`OrderType::{Limit, Market}`、`Trade { taker_id, maker_id, price, qty }`
- 错误类型：`EngineError::{UnknownOrder, InvalidSide, InvalidSequence}`
- 基本方法（简要）：
  - `OrderBook::new()`：创建新订单簿
  - `submit_limit(side, price, qty) -> (OrderId, Vec<Trade>, u64)`
  - `submit_market(side, qty) -> (OrderId, Vec<Trade>, u64)`
  - `submit_limit_into(side, price, qty, &mut trades) -> (OrderId, u64)`
  - `submit_market_into(side, qty, &mut trades) -> (OrderId, u64)`
  - `process_commands_batch_checked_into(&mut [Command], &mut trades) -> Result<Vec<(OrderId, u64)>, EngineError>`
    - `Command` 带 `seq: u64` 字段：`Limit { seq, side, price, qty } | Market { seq, side, qty } | Cancel { seq, id }`
  - `cancel(id) -> Result<Order, EngineError>`
  - `best_bid()/best_ask()/top_n(n)`：查询报价与聚合深度

## 并发入站与多交易对（ingestor）

- 单簿 `Ingestor`：
  - 外部发送 `RawCommand`（不带 seq），内部按接收顺序赋 `seq`，成批调用引擎。
- 多簿 `MultiIngestor`：
  - `MultiRawCommand { symbol, cmd }` 通过 Router 分发到对应 symbol 的 worker。
  - 每个 symbol 独立线程、独立 `OrderBook`、独立 `seq` 递增，保证簿内强一致。
  - 产出通道：
    - `rx_trade: Receiver<(String, Trade)>`（可选，emit_trades=false 时关闭发送以提升吞吐）
    - `rx_done: Receiver<usize>`：每批完成后上报处理的指令数
  - 直连路由：`routes: HashMap<String, Sender<RawCommand>>` 允许绕过 Router 直接按 symbol 发送。
  - 启动（带配置）：
    - `start_with_books_with_config(books, Options { batch_size, emit_trades, coalesce_micros })`

## 使用说明

1) 构建与测试

```bash
cargo build
cargo test -p match-engine
```

2) 运行 CLI（单簿示例）

```bash
cargo run -p ingestor --bin ingestor_cli
```

命令格式：

```
limit buy|sell <price> <qty>
market buy|sell <qty>
cancel <order_id>
quit
```

## 压测与基准

1) 单簿吞吐（engine）

```bash
# 限价/市价吞吐
cargo bench -p match-engine --bench throughput

# 单条 vs 零分配 vs 批处理 对比
cargo bench -p match-engine --bench batch_compare
```

2) 多交易对吞吐（ingestor）

```bash
cargo bench -p ingestor --bench multipair_throughput
```

基准输出包含 Elements/sec（orders/sec）。若需要更稳定的数据可延长测量时间：

```bash
cargo bench -p ingestor --bench multipair_throughput -- --measurement-time 10
```

HTML 报告在 `target/criterion/**/report/index.html`。

## 性能优化选项

- 批量大小：`Options.batch_size`（推荐范围 4K–64K）
- 合并等待：`Options.coalesce_micros`（微秒级，低流量时提高批量利用率）
- 关闭成交广播：`Options.emit_trades = false`（减少跨线程发送）
- 直连路由：使用 `MultiIngestor.routes` 直接向每个 symbol 的 sender 发送，绕过 Router。

## 强一致与重放

- 批处理接口会稳定排序并校验 `seq` 严格递增，检测重复/乱序将报错。
- 为跨机一致，建议在上游统一分配全局单调 `seq` 并确保按序传递给引擎/ingestor。