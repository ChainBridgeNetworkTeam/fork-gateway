use crate::{
    chains::{ChainAccount, ChainBlockNumber},
    symbol::{CASH, USD},
    types::{CashPrincipal, Quantity, Timestamp},
};
//  链管理中的大数
/// The large value (USD) used for ingesting gov events.
pub const INGRESS_LARGE: Quantity = Quantity::from_nominal("1000000000000", USD);

//  每条链能够能够存贮美元的最大值
/// The maximum value (USD) that can be ingested per underlying chain block.
/// Could become a per-chain quota in the future.
pub const INGRESS_QUOTA: Quantity = Quantity::from_nominal("10000", USD);
//  发送新区块时的最大区块队列
/// Maximum size of the block queue before we back-off sending new blocks.
pub const INGRESS_SLACK: u32 = 50;
//  一年的毫秒数
/// Number of milliseconds in a year.
pub const MILLISECONDS_PER_YEAR: Timestamp = 365 * 24 * 60 * 60 * 1000;
//  最小的在发起事件是需要等待块的数目，为了避免链reorg的风向
/// Minimum number of underlying chain blocks to wait before ingesting any event, due to reorg risk.
pub const MIN_EVENT_BLOCKS: ChainBlockNumber = 3;
//  最大的等待块的数目
/// Maximum number of underlying chain blocks to wait before just ingesting any event.
pub const MAX_EVENT_BLOCKS: ChainBlockNumber = 60;
//  同步一个排入日程的变化的所需最小时间
//  必须在其发生时有足够的时间在l1中广播这个变化
/// Minimum amount of time (milliseconds) into the future that a synchronized change may be scheduled for.
/// Must be sufficient time to propagate changes to L1s before they occur.
pub const MIN_NEXT_SYNC_TIME: Timestamp = 24 * 60 * 60 * 1000; // XXX confirm
//  使用gateway账户所需的最小cash数
//  验证人必须满足最小的条件以便提交会话key的集合
/// Minimum CASH principal required in order to use a Gateway account.
/// Note that validators must meet this minimum in order to submit the set session keys extrinsic.
pub const MIN_PRINCIPAL_GATE: CashPrincipal = CashPrincipal::from_nominal("1");
//  为了同各个协议交互所需的最小美元数
/// Minimum value (USD) required across all protocol interactions.
pub const MIN_TX_VALUE: Quantity = Quantity::from_nominal("1", USD);
//  转账费
/// Flat transfer fee (CASH).
pub const TRANSFER_FEE: Quantity = Quantity::from_nominal("0.01", CASH);
//  每个会话周期中的区块的数目
/// The number of blocks in between periodic sessions.
pub const SESSION_PERIOD: u32 = 14400; // Assuming 6s blocks, ~1 period per day
//  所有未签名事件的优先级
/// Standard priority for all unsigned transactions.
pub const UNSIGNED_TXS_PRIORITY: u64 = 100;
//  标准的持续时间，针对所有的未签名转账
/// Standard longevity for all unsigned transactions.
pub const UNSIGNED_TXS_LONGEVITY: u64 = 32;
//  所有可能提前退出的外部事件的权重，为了避免垃圾调用
/// Weight given to extrinsics that will exit early, to avoid spam.
pub const ERROR_WEIGHT: u64 = 100_000_000;
//  矿工账户用来转出的cash的空账户
/// The void account from whence miner CASH is transferred out of.
pub const GATEWAY_VOID: ChainAccount = ChainAccount::Gate([0u8; 32]);
//  trx请求的最长宽度限制
/// The maximum length of a trx request
pub const MAX_TRX_REQUEST_LEN: usize = 2048;
