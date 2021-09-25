use crate::{
    chains::{
        ChainAsset, ChainBlock, ChainBlockEvent, ChainBlockEvents, ChainBlockNumber,
        ChainBlockTally, ChainBlocks, ChainHash, ChainId, ChainReorg, ChainReorgTally,
        ChainSignature, ChainStarport,
    },
    core::{
        self, get_current_validator, get_event_queue, get_first_block, get_last_block,
        get_starport, get_validator_set, recover_validator, validator_sign,
    },
    debug, error,
    events::{fetch_chain_block, fetch_chain_block_by_hash, fetch_chain_blocks},
    internal::assets::{get_cash_quantity, get_quantity, get_value},
    log,
    params::{INGRESS_LARGE, INGRESS_QUOTA, INGRESS_SLACK, MAX_EVENT_BLOCKS, MIN_EVENT_BLOCKS},
    reason::{MathError, Reason},
    require,
    types::{CashPrincipalAmount, Quantity, USDQuantity, USD},
    Call, Config, Event as EventT, IngressionQueue, LastProcessedBlock, Module, PendingChainBlocks,
    PendingChainReorgs,
};
use codec::Encode;
use ethereum_client::EthereumEvent;
use frame_support::storage::StorageMap;
use frame_system::offchain::SubmitTransaction;
use our_std::{cmp::max, convert::TryInto};
use sp_core::offchain::Duration;
use sp_runtime::offchain::{
    storage::StorageValueRef,
    storage_lock::{StorageLock, Time},
};

//  定义翻转方法
trait CollectRev: Iterator {
    fn collect_rev(self) -> Vec<Self::Item>
    where
        Self: Sized,
    {
        let mut v: Vec<Self::Item> = self.into_iter().collect();
        v.reverse();
        v
    }
}

impl<I: Iterator> CollectRev for I {}

//  判断准入队列中能够进入的区块数
//  允许处理的区块数有限，同时最多有INGRESS_SLACK个，这个周期能够进入的就是INGRESS_SLACK减去已经在队里列的
//  确定仍然可以容纳在入口队列中的块数。
/// Determine the number of blocks which can still fit on an ingression queue.
pub fn queue_slack(event_queue: &ChainBlockEvents) -> u32 {
    let queue_len: u32 = event_queue.len().try_into().unwrap_or(u32::MAX);
    //  最少一个
    max(INGRESS_SLACK.saturating_sub(queue_len), 1)
}
//  决定一个特定事件的风控调整值，同时也给定一个区块数，感觉类似风控保证金
/// Determine the risk-adjusted value of a particular event, given the current block number.
pub fn risk_adjusted_value<T: Config>(
    block_event: &ChainBlockEvent,
    block_number: ChainBlockNumber,
) -> Result<USDQuantity, Reason> {
    //  过去了多少个区块
    let elapsed_blocks = block_number
        .checked_sub(block_event.block_number())
        .ok_or(Reason::Unreachable)?;
    match block_event {
        ChainBlockEvent::Reserved => panic!("reserved"),
        ChainBlockEvent::Eth(_block_num, eth_event) => match eth_event {
            EthereumEvent::Lock { asset, amount, .. } => {
                let quantity = get_quantity::<T>(ChainAsset::Eth(*asset), *amount)?;
                let usd_quantity = get_value::<T>(quantity)?;
                //  返回对应过去区块数目的资产衰减值
                Ok(usd_quantity.decay(elapsed_blocks)?)
            }

            EthereumEvent::LockCash { principal, .. } => {
                let quantity = get_cash_quantity::<T>(CashPrincipalAmount(*principal))?;
                let usd_quantity = get_value::<T>(quantity)?;
                Ok(usd_quantity.decay(elapsed_blocks)?)
            }

            EthereumEvent::ExecuteProposal { .. } => {
                let usd_quantity = get_value::<T>(INGRESS_LARGE)?;
                Ok(usd_quantity.decay(elapsed_blocks)?)
            }

            _ => Ok(Quantity::new(0, USD)),
        },
        ChainBlockEvent::Matic(_block_num, eth_event) => match eth_event {
            EthereumEvent::Lock { asset, amount, .. } => {
                let quantity = get_quantity::<T>(ChainAsset::Matic(*asset), *amount)?;
                let usd_quantity = get_value::<T>(quantity)?;
                debug!("matic lock detected usd_quantity={:?}", usd_quantity);
                Ok(usd_quantity.decay(elapsed_blocks)?)
            }

            EthereumEvent::LockCash { principal, .. } => {
                let quantity = get_cash_quantity::<T>(CashPrincipalAmount(*principal))?;
                let usd_quantity = get_value::<T>(quantity)?;
                Ok(usd_quantity.decay(elapsed_blocks)?)
            }

            EthereumEvent::ExecuteProposal { .. } => {
                let usd_quantity = get_value::<T>(INGRESS_LARGE)?;
                Ok(usd_quantity.decay(elapsed_blocks)?)
            }

            _ => Ok(Quantity::new(0, USD)),
        },
    }
}

//  判断对应链的智能合约是否开启
/// Detect if a starport is enabled for the given chain_id.
/// If a starport isn't available, we consider the chain disabled, instead of erring.
fn is_starport_enabled<T: Config>(chain_id: ChainId) -> bool {
    get_starport::<T>(chain_id).is_ok()
}

//  增量执行所有对等链的追踪事件
/// Incrementally perform the next step of tracking events from all the underlying chains.
pub fn track_chain_events<T: Config>() -> Result<(), Reason> {
    //  这段逻辑看上去无关紧要，但是实际上对于锁定非常重要
    // 感觉像是防止竞争锁？
    // Note: The way this is written might look pointless, but its very important to the lock
    //  在经过充分的讨论和测试之前，不要修改这段逻辑
    //  Do not modify lightly and without discussion / further testing.
    let deadline = Duration::from_millis(120_000);
    let mut lock = StorageLock::<Time>::with_deadline(b"cash::track_chain_events", deadline);
    let result = match lock.try_lock() {
        Ok(_guard) => {
            // Note: chains could be parallelized
            track_chain_events_on::<T>(ChainId::Eth)?;

            if is_starport_enabled::<T>(ChainId::Matic) {
                track_chain_events_on::<T>(ChainId::Matic)?;
            }

            Ok(())
        }

        _ => Err(Reason::WorkerBusy),
    };
    result
}

//  执行下一步对等链上的事件追踪操作
/// Perform the next step of tracking events from an underlying chain.
pub fn track_chain_events_on<T: Config>(chain_id: ChainId) -> Result<(), Reason> {
    let starport = get_starport::<T>(chain_id)?;
    let me = get_current_validator::<T>()?;
    let last_block = get_last_block::<T>(chain_id)?;
    let next_block_number = last_block
        .number()
        .checked_add(1)
        .ok_or(MathError::Overflow)?;
    //  获取下一个区块
    let next_block = fetch_chain_block(chain_id, next_block_number, starport)?;
    //  如果上个区块的hash等于下个区块的父hash，表示没有分叉
    if last_block.hash() == next_block.parent_hash() {
        debug!(
            "Worker sees the same fork: next={:?} last={:?}",
            next_block, last_block
        );
        let pending_blocks = PendingChainBlocks::get(chain_id);
        let event_queue = get_event_queue::<T>(chain_id)?;
        //  这个周期可以新加入的事件数目
        let slack = queue_slack(&event_queue) as u64;
        //  计算得出这一次需要处理的对等连区块数组
        let blocks = next_block
            .concat(fetch_chain_blocks(
                chain_id,
                next_block_number
                    .checked_add(1)
                    .ok_or(MathError::Overflow)?,
                next_block_number
                    .checked_add(1)
                    .ok_or(MathError::Overflow)?
                    .checked_add(slack)
                    .ok_or(MathError::Overflow)?,
                starport,
            )?)?
            //  过滤掉已经被这个节点处理过的区块
            .filter_already_supported(&me.substrate_id, pending_blocks);
        // 把取款缓存区来    
        memorize_chain_blocks::<T>(&blocks)?;
        //  提交区块发起交易
        submit_chain_blocks::<T>(&blocks)
    } else {
        //  发生分叉
        debug!(
            "Worker sees a different fork: next={:?} last={:?}",
            next_block, last_block
        );
        //  获取真实的上一个区块
        let true_block = fetch_chain_block(chain_id, last_block.number(), starport)?;
        //  阻塞的对应链的reorg
        let pending_reorgs = PendingChainReorgs::get(chain_id);
        //  形成上一个区块到新区块的路径，计算需要重新处理的区块路径
        let reorg = formulate_reorg::<T>(chain_id, &last_block, &true_block)?;
        //  如果没有都签名，那么提交reorg
        if !reorg.is_already_signed(&me.substrate_id, pending_reorgs) {
            memorize_chain_blocks::<T>(&reorg.forward_blocks())?;
            submit_chain_reorg::<T>(&reorg)
        } else {
            debug!("Worker already submitted... waiting");
            // just wait for the reorg to succeed or fail,
            //  or we change our minds in another pass (noop)
            Ok(())
        }
    }
}
//  进入单轮
/// Ingress a single round (quota per underlying chain block ingested).
pub fn ingress_queue<T: Config>(
    last_block: &ChainBlock,
    event_queue: &mut ChainBlockEvents,
) -> Result<(), Reason> {
    //  每条链可引入的最大价值（美元计算）
    let mut available = INGRESS_QUOTA;
    let block_num = last_block.number();
    //  类似于filter，只留下满足条件的内容
    event_queue.retain(|event| {
        let delta_blocks = block_num.saturating_sub(event.block_number());
        //  如果事件发生大于最小验证区块 3
        if delta_blocks >= MIN_EVENT_BLOCKS {
            //  如果大于最大验证区块60，直接认为是无风险的
            // If we're beyond max risk block, then simply accept event
            let risk_result = if delta_blocks > MAX_EVENT_BLOCKS {
                Ok(Quantity::new(0, USD))
            } else {
                //  否则计算一个风控值
                risk_adjusted_value::<T>(event, block_num)
            };

            match risk_result {
                Ok(value) => {
                    debug!(
                        "Computed risk adjusted value ({:?} / {:?} @ {}) of {:?}",
                        value, available, block_num, event
                    );
                    //  如果风控值小于单个区块限额
                    if value <= available {
                        available = available.sub(value).unwrap();
                        //  执行这个事件，发出event
                        match core::apply_chain_event_internal::<T>(event) {
                            Ok(()) => {
                                <Module<T>>::deposit_event(EventT::ProcessedChainBlockEvent(
                                    event.clone(),
                                ));
                            }

                            Err(reason) => {
                                <Module<T>>::deposit_event(
                                    EventT::FailedProcessingChainBlockEvent(event.clone(), reason),
                                );
                            }
                        }
                        return false; // remove from queue
                    } else {
                        return true; // retain on queue
                    }
                }

                Err(err) => {
                    error!(
                        "Could not compute risk adjusted value ({}) of {:?}",
                        err, event
                    );
                    // note that we keep the event if we cannot compute the risk adjusted value,
                    //  there's not an obviously more reasonable thing to do right now
                    // there's no reason this should fail normally but it can
                    //  e.g. if somehow someone locks an unsupported asset in the starport
                    // we need to take separate measures to forcefully limit the queue size
                    //  e.g. reject new blocks once the event queue reaches a certain size
                    return true; // retain on queue
                }
            }
        } else {
            debug!(
                "Event not old enough to process (@ {:?}) {:?}",
                block_num, event
            );
            //  区块数小于3，继续在队列里
            return true; // retain on queue
        }
    });
    Ok(())
}
//  提交对等链计算出的接下来需要使用的区块
/// Submit the underlying chain blocks the worker calculates are needed by the chain next.
pub fn submit_chain_blocks<T: Config>(blocks: &ChainBlocks) -> Result<(), Reason> {
    if blocks.len() > 0 {
        log!("Submitting chain blocks extrinsic: {:?}", blocks);
        let signature = validator_sign::<T>(&blocks.encode()[..])?;
        //  生成调用
        let call = Call::receive_chain_blocks(blocks.clone(), signature);
        if let Err(e) = SubmitTransaction::<T, Call<T>>::submit_unsigned_transaction(call.into()) {
            log!("Error while submitting chain blocks: {:?}", e);
            return Err(Reason::FailedToSubmitExtrinsic);
        }
    }
    Ok(())
}

//  记录我们提交的区块，以便我们可以reorg
/// Remember whatever blocks we submit, so we can formulate reorgs if needed.
pub fn memorize_chain_blocks<T: Config>(blocks: &ChainBlocks) -> Result<(), Reason> {
    // Note: grows unboundedly, but pruning history can happen independently / later
    for block in blocks.blocks() {
        let key = format!("cash::memorize_chain_blocks::{}", block.hash());
        let krf = StorageValueRef::persistent(key.as_bytes());
        krf.set(&block);
    }
    Ok(())
}

//  向后遍历本地存储的块，以制定重组路径
//  重新拿回目标区块，根据hash
/// Walk backwards through the locally stored blocks, in order to formulate a reorg path.
pub fn recall_chain_block<T: Config>(
    chain_id: ChainId,
    hash: ChainHash,
    starport: ChainStarport,
) -> Result<ChainBlock, Reason> {
    let key = format!("cash::memorize_chain_blocks::{}", hash);
    let krf = StorageValueRef::persistent(key.as_bytes());
    match krf.get::<ChainBlock>() {
        Some(Some(block)) => Ok(block),
        _ => match fetch_chain_block_by_hash(chain_id, hash, starport) {
            Ok(block) => Ok(block),
            Err(_) => Err(Reason::MissingBlock),
        },
    }
}

//  找到最后区块和真实区块之间的路径
/// Try to form a path from the last block to the new true block.
pub fn formulate_reorg<T: Config>(
    chain_id: ChainId,
    last_block: &ChainBlock,
    true_block: &ChainBlock,
) -> Result<ChainReorg, Reason> {
    let starport = get_starport::<T>(chain_id)?;
    let first_block = get_first_block::<T>(chain_id)?;
    //  正序的反转块
    let mut reverse_blocks: Vec<ChainBlock> = vec![]; // reverse blocks in correct order
    //  逆序的正向块
    let mut drawrof_blocks: Vec<ChainBlock> = vec![]; // forward blocks in reverse order
    let mut reverse_block_next = last_block.clone();
    let mut drawrof_block_next = true_block.clone();

    reverse_blocks.push(reverse_block_next.clone());
    drawrof_blocks.push(drawrof_block_next.clone());

    loop {
        //  两个区块必须等高
        // these blocks must be at the same height, or fail
        if reverse_block_next.number() != drawrof_block_next.number() {
            return Err(Reason::BlockMismatch);
        }

        let next_block_number = drawrof_block_next
            .number()
            .checked_sub(1)
            .ok_or(MathError::Underflow)?;
        //  reversie用hash回溯
        reverse_block_next =
            recall_chain_block::<T>(chain_id, reverse_block_next.parent_hash(), starport)?;
        //  dray直接用区块数减一
        drawrof_block_next = fetch_chain_block(chain_id, next_block_number, starport)?;

        reverse_blocks.push(reverse_block_next.clone());
        drawrof_blocks.push(drawrof_block_next.clone());

        //  如果有同一个父hash，目标达成
        // these blocks have a common ancestor, so we are done
        if reverse_block_next.parent_hash() == drawrof_block_next.parent_hash() {
            break;
        }
        //  在第一个区块之前没有内容了，所以没有影响
        // we do not have blocks before the first, which would have no impact
        if reverse_block_next.number() == first_block.number() {
            break;
        }
    }

    match (last_block.hash(), true_block.hash()) {
        (ChainHash::Eth(from_hash), ChainHash::Eth(to_hash)) => Ok(ChainReorg::Eth {
            from_hash,
            to_hash,
            reverse_blocks: reverse_blocks
                .into_iter()
                .filter_map(|b| match b {
                    ChainBlock::Eth(eth_block) => Some(eth_block),
                    ChainBlock::Matic(block) => Some(block),
                })
                .collect(),
            forward_blocks: drawrof_blocks
                .into_iter()
                .filter_map(|b| match b {
                    ChainBlock::Eth(eth_block) => Some(eth_block),
                    ChainBlock::Matic(block) => Some(block),
                })
                //  这里给反过来
                .collect_rev(),
        }),
        //  没法抵达
        _ => return Err(Reason::Unreachable),
    }
}
//  上报reorg的消息
/// Submit a reorg message from a worker to the chain.
pub fn submit_chain_reorg<T: Config>(reorg: &ChainReorg) -> Result<(), Reason> {
    log!("Submitting chain reorg extrinsic: {:?}", reorg);
    let signature = validator_sign::<T>(&reorg.encode()[..])?;
    let call = Call::receive_chain_reorg(reorg.clone(), signature);
    if let Err(e) = SubmitTransaction::<T, Call<T>>::submit_unsigned_transaction(call.into()) {
        log!("Error while submitting chain blocks: {:?}", e);
        return Err(Reason::FailedToSubmitExtrinsic);
    }
    Ok(())
}
//  接收从worker中得到的消息，在必要的时候记数并执行
/// Receive a blocks message from a worker, tallying it and applying as necessary.
pub fn receive_chain_blocks<T: Config>(
    blocks: ChainBlocks,
    signature: ChainSignature,
) -> Result<(), Reason> {
    let validator_set = get_validator_set::<T>()?;
    let validator = recover_validator::<T>(&blocks.encode(), signature)?;
    let chain_id = blocks.chain_id();
    let mut event_queue = get_event_queue::<T>(chain_id)?;
    let mut last_block = get_last_block::<T>(chain_id)?;
    let mut pending_blocks = PendingChainBlocks::get(chain_id);

    debug!("Pending blocks: {:?}", pending_blocks);
    debug!("Event queue: {:?}", event_queue);

    for block in blocks.blocks() {
        if block.number() >= last_block.number() + 1 {
            let offset = (block.number() - last_block.number() - 1) as usize;
            if let Some(prior) = pending_blocks.get_mut(offset) {
                if block != prior.block {
                    //  区块不一样默认有冲突
                    debug!(
                        "Received conflicting block, dissenting: {:?} ({:?})",
                        block, prior
                    );
                    //  实际上是一个tally
                    //  郑加一个反对者
                    prior.add_dissent(&validator);
                } else {
                    debug!("Received support for existing block: {:?}", block);
                    //  增加一个支持者
                    prior.add_support(&validator);
                }
            } else if offset == 0 {
                //  如果父hash不一致，表示区块有分叉
                if block.parent_hash() != last_block.hash() {
                    debug!(
                        "Received block which would require fork: {:?} ({:?})",
                        block, last_block
                    );
                    //  worker需要reorg如果你需要做什么的话，但这就会是一个滞后的消息
                    //  如果先前的存在，这将是一个反对票，但是这样也ok因为worker会reorg
                    // worker should reorg if it wants to build something else
                    //  but could just be a laggard message
                    // this *would* be a dissenting vote if prior existed
                    //  but that's ok bc worker will try to reorg instead
                    continue;
                } else {
                    debug!("Received valid first next pending block: {:?}", block);
                    // write to pending_blocks[offset]
                    //  我们已经检查过了偏移量不存在，这就是第一个元素
                    //  we already checked offset doesn't exist, this is the first element
                    pending_blocks.push(ChainBlockTally::new(block, &validator));
                }
                //  拿前一个
            } else if let Some(parent) = pending_blocks.get(offset - 1) {
                if block.parent_hash() != parent.block.hash() {
                    debug!(
                        "Received invalid derivative block: {:?} ({:?})",
                        block, parent
                    );
                    //  workewr正在提交父区块有冲突的内容，如果还是要提交，那么就会是一个反对票（如果先前的存在的话
                    //  但是也ok因为worker会提交reorg）
                    // worker is submitting block for parent that conflicts
                    //  if its also submitting the parent, which it should be,
                    //   that one will vote against and propagate forwards
                    // this *would* be a dissenting vote if prior existed
                    //  but that's ok bc worker should submit parent first
                    continue;
                } else {
                    debug!("Received valid pending block: {:?}", block);
                    // write to pending_blocks[offset]
                    //  当前偏移的不存在，但是偏移-1存在
                    //  we already checked offset doesn't exist, but offset - 1 does
                    pending_blocks.push(ChainBlockTally::new(block, &validator));
                }
            } else {
                debug!("Received disconnected block: {:?} ({:?})", block, offset);
                //  我们没有区块，其父区块也不存在
                //  worker将不会提交类似这样的内容
                //  理论上不会有这种情况，直接忽略，如果遇到这种情况，我们不处理
                // we don't have the block, nor a parent for it
                //  the worker shouldn't submit stuff like this
                // blocks should be in order in which case this wouldn't happen
                //  but just ignore, if workers aren't connecting blocks we won't make progress
                continue;
            }
        } else {
            //  接收到不相关的过去区块
            debug!(
                "Received irrelevant past block: {:?} ({:?})",
                block, last_block
            );
            continue;
        }
    }
    //  每个投票计数器拉出来
    for tally in pending_blocks.clone().iter() {
        //  如果投票通过
        if tally.has_enough_support(&validator_set) {
            //  从区块队列中移除
            //  将事件加入事件队列，推进区块，处理一轮事件
            // remove tally from block queue
            //  add events to event queue, advance the block, and process a round of events
            pending_blocks.remove(0); // note: tally is first on queue
            event_queue.push(&tally.block);
            last_block = tally.block.clone();
            ingress_queue::<T>(&last_block, &mut event_queue)?;
            continue;
        } else if tally.has_enough_dissent(&validator_set) {
            //  如果投票反对，直接清空
            //  这里为什么都清掉了？
            // remove tally and everything after from queue
            pending_blocks = vec![];
            break;
        } else {
            break;
        }
    }
    //  上一个处理的区块
    LastProcessedBlock::insert(chain_id, last_block);
    //  pending的区块更新
    PendingChainBlocks::insert(chain_id, pending_blocks);
    //  许可事件的更新
    IngressionQueue::insert(chain_id, event_queue);

    Ok(())
}

//  从worker接收reorg消息，需要的时候计数并且开始执行操作
/// Receive a reorg message from a worker, tallying it and applying as necessary.
pub fn receive_chain_reorg<T: Config>(
    reorg: ChainReorg,
    signature: ChainSignature,
) -> Result<(), Reason> {
    let validator_set = get_validator_set::<T>()?;
    let validator = recover_validator::<T>(&reorg.encode(), signature)?;
    let chain_id = reorg.chain_id();
    let mut event_queue = get_event_queue::<T>(chain_id)?;
    let mut last_block = get_last_block::<T>(chain_id)?;
    let mut pending_reorgs = PendingChainReorgs::get(chain_id);

    //  这个检查失败后将停止消息传播
    // Note: can reject / stop propagating once this check fails
    require!(reorg.from_hash() == last_block.hash(), Reason::HashMismatch);
    //  找到匹配的计数，并且新增一个支持的验证人
    let tally = if let Some(prior) = pending_reorgs.iter_mut().find(|r| r.reorg == reorg) {
        prior.add_support(&validator);
        prior
    } else {
        //  如果过没有找到，新建一个并且返回
        pending_reorgs.push(ChainReorgTally::new(chain_id, reorg, &validator));
        pending_reorgs.last_mut().unwrap()
    };
    //  无论最后一个签名者是否有竞赛，这里都会有次优选
    //  我们目前不保留最近处理过重组的墓碑标记
    // Note: whenever there's a race to be the last signer, this will be suboptimal
    //  we don't currently keep a tombstone marking that the reorg was recently processed
    if tally.has_enough_support(&validator_set) {
        // if we have enough support, perform actual reorg
        // for each block going backwards
        //  remove events from queue, or unapply them if already applied
        for block in tally.reorg.reverse_blocks().blocks() {
            for event in block.events() {
                //  这里有优化空间
                // Note: this could be made significantly more efficient
                //  at the cost of significant complexity
                if let Some(pos) = event_queue.position(&event) {
                    //  如果事件队列中有这个事件，直接拿掉
                    event_queue.remove(pos);
                } else {
                    //  撤回事件的执行
                    core::unapply_chain_event_internal::<T>(&event)?
                }
            }
        }
        //  对于前向的区块，新增到事件队列，区块提前，处理一轮事件
        // for each block going forwards
        //  add events to event queue, advance the block, and process a round of events
        for block in tally.reorg.forward_blocks().blocks() {
            event_queue.push(&block);
            last_block = block.clone();
            ingress_queue::<T>(&last_block, &mut event_queue)?;
        }
        //  将新的状态同步到链上存储
        // write the new state back to storage
        //  最近处理的区块更新
        LastProcessedBlock::insert(chain_id, last_block);
        //  新增pending区块
        PendingChainBlocks::insert(chain_id, Vec::<ChainBlockTally>::new());
        //  新增pending的reorg
        PendingChainReorgs::insert(chain_id, Vec::<ChainReorgTally>::new());
        //  许可队列
        IngressionQueue::insert(chain_id, event_queue);
    } else {
        //  否则只更新pending的reorg
        // otherwise just update the stored reorg tallies
        PendingChainReorgs::insert(chain_id, pending_reorgs);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::*;
    use ethereum_client::EthereumBlock;

    fn gen_blocks(start_block: u64, until_block: u64, pad: u8) -> Vec<EthereumBlock> {
        let mut hash = [0u8; 32];
        let mut v: Vec<ethereum_client::EthereumBlock> = vec![];
        for i in start_block..until_block {
            let parent_hash = hash;
            let mut hashvec = i.to_le_bytes().to_vec();
            hashvec.extend_from_slice(&[pad; 24]);
            hash = hashvec.try_into().unwrap();
            v.push(EthereumBlock {
                hash,
                parent_hash,
                number: i,
                events: vec![],
            });
        }
        return v;
    }

    #[test]
    fn test_track_chain_events_on_eth_reorg_and_back() {
        let old_chain: Vec<EthereumBlock> = gen_blocks(0, 10, 0);
        let new_chain: Vec<EthereumBlock> = gen_blocks(1, 10, 1);
        let common_ancestor_block = old_chain[0].clone();
        let last_block = old_chain.last().unwrap().clone();
        let true_block = new_chain.last().unwrap().clone();

        let mut fetched_blocks = vec![EthereumBlock {
            hash: [10u8; 32],
            parent_hash: true_block.hash,
            number: 10,
            events: vec![],
        }];
        fetched_blocks.extend(new_chain[0..9].iter().rev().cloned().collect::<Vec<_>>());
        fetched_blocks.push(EthereumBlock {
            hash: [10u8; 32],
            parent_hash: last_block.hash,
            number: 10,
            events: vec![],
        });
        fetched_blocks.extend(old_chain[1..10].iter().rev().cloned());
        let calls = gen_mock_calls(&fetched_blocks, ETH_STARPORT_ADDR);
        let (mut t, _, _) = new_test_ext_with_http_calls(calls);

        t.execute_with(|| {
            initialize_storage_with_blocks(vec![ChainBlock::Eth(common_ancestor_block)]);

            LastProcessedBlock::insert(ChainId::Eth, ChainBlock::Eth(last_block));
            memorize_chain_blocks::<Test>(&ChainBlocks::Eth(old_chain.clone())).unwrap();
            track_chain_events_on::<Test>(ChainId::Eth).unwrap();

            LastProcessedBlock::insert(ChainId::Eth, ChainBlock::Eth(true_block));
            track_chain_events_on::<Test>(ChainId::Eth).unwrap();
        });
    }

    #[test]
    fn test_formulate_reorg() {
        let old_chain: Vec<EthereumBlock> = gen_blocks(0, 10, 0);
        let new_chain: Vec<EthereumBlock> = gen_blocks(1, 10, 1);
        let common_ancestor_block = old_chain[0].clone();
        let last_block = old_chain.last().unwrap().clone();
        let true_block = new_chain.last().unwrap().clone();

        // new_chain blocks -> 1...9, excluding true block -> 1...8 -> indices 0..8
        let fetched_blocks = new_chain[0..8].iter().rev().cloned().collect::<Vec<_>>();
        let calls = gen_mock_calls(&fetched_blocks, ETH_STARPORT_ADDR);
        let (mut t, _, _) = new_test_ext_with_http_calls(calls);

        t.execute_with(|| {
            initialize_storage_with_blocks(vec![ChainBlock::Eth(common_ancestor_block)]);
            memorize_chain_blocks::<Test>(&ChainBlocks::Eth(old_chain.clone())).unwrap();
            let reorg = formulate_reorg::<Test>(
                ChainId::Eth,
                &ChainBlock::Eth(last_block.clone()),
                &ChainBlock::Eth(true_block.clone()),
            )
            .unwrap();

            match reorg {
                ChainReorg::Eth {
                    from_hash,
                    to_hash,
                    reverse_blocks,
                    forward_blocks,
                } => {
                    assert_eq!(from_hash, last_block.hash);
                    assert_eq!(to_hash, true_block.hash);
                    assert_eq!(
                        reverse_blocks,
                        old_chain[1..10].iter().rev().cloned().collect::<Vec<_>>()
                    );
                    assert_eq!(
                        forward_blocks.iter().map(|x| x.hash).collect::<Vec<_>>(),
                        new_chain.iter().map(|x| x.hash).collect::<Vec<_>>()
                    );
                }
                _ => unreachable!(),
            }
        });
    }

    #[test]
    fn test_formulate_reorg_height_mismatch() {
        let old_chain: Vec<EthereumBlock> = gen_blocks(0, 10, 0);
        let new_chain: Vec<EthereumBlock> = gen_blocks(1, 9, 1);
        let common_ancestor_block = old_chain[0].clone();
        let last_block = old_chain.last().unwrap().clone();
        let true_block = new_chain.last().unwrap().clone();

        let fetched_blocks = vec![];
        let calls = gen_mock_calls(&fetched_blocks, ETH_STARPORT_ADDR);
        let (mut t, _, _) = new_test_ext_with_http_calls(calls);

        t.execute_with(|| {
            initialize_storage_with_blocks(vec![ChainBlock::Eth(common_ancestor_block)]);
            memorize_chain_blocks::<Test>(&ChainBlocks::Eth(vec![])).unwrap();
            assert_eq!(
                formulate_reorg::<Test>(
                    ChainId::Eth,
                    &ChainBlock::Eth(last_block.clone()),
                    &ChainBlock::Eth(true_block.clone()),
                ),
                Err(Reason::BlockMismatch)
            );
        });
    }

    #[test]
    fn test_formulate_reorg_missing_data() {
        let old_chain: Vec<EthereumBlock> = gen_blocks(0, 10, 0);
        let new_chain: Vec<EthereumBlock> = gen_blocks(1, 10, 1);
        let common_ancestor_block = old_chain[0].clone();
        let last_block = old_chain.last().unwrap().clone();
        let true_block = new_chain.last().unwrap().clone();

        let fetched_blocks = new_chain[7..8].iter().rev().cloned().collect::<Vec<_>>();
        let mut calls = gen_mock_calls(&fetched_blocks, ETH_STARPORT_ADDR);
        calls.push(gen_mock_call_block_by_hash_fail(&old_chain[7]));
        let (mut t, _, _) = new_test_ext_with_http_calls(calls);

        t.execute_with(|| {
            initialize_storage_with_blocks(vec![ChainBlock::Eth(common_ancestor_block)]);
            memorize_chain_blocks::<Test>(&ChainBlocks::Eth(old_chain[8..10].to_vec())).unwrap();
            assert_eq!(
                formulate_reorg::<Test>(
                    ChainId::Eth,
                    &ChainBlock::Eth(last_block.clone()),
                    &ChainBlock::Eth(true_block.clone()),
                ),
                Err(Reason::MissingBlock)
            );
        });
    }

    #[test]
    fn test_formulate_reorg_before_first() {
        let old_chain: Vec<EthereumBlock> = gen_blocks(0, 10, 0);
        let new_chain: Vec<EthereumBlock> = gen_blocks(0, 10, 1);
        let last_block = old_chain.last().unwrap().clone();
        let true_block = new_chain.last().unwrap().clone();

        let fetched_blocks = new_chain[0..9].iter().rev().cloned().collect::<Vec<_>>();
        let calls = gen_mock_calls(&fetched_blocks, ETH_STARPORT_ADDR);
        let (mut t, _, _) = new_test_ext_with_http_calls(calls);

        t.execute_with(|| {
            initialize_storage_with_blocks(vec![ChainBlock::Eth(old_chain[0].clone())]);
            memorize_chain_blocks::<Test>(&ChainBlocks::Eth(old_chain.clone())).unwrap();
            let reorg = formulate_reorg::<Test>(
                ChainId::Eth,
                &ChainBlock::Eth(last_block.clone()),
                &ChainBlock::Eth(true_block.clone()),
            )
            .unwrap();

            match reorg {
                ChainReorg::Eth {
                    from_hash,
                    to_hash,
                    reverse_blocks,
                    forward_blocks,
                } => {
                    assert_eq!(from_hash, last_block.hash);
                    assert_eq!(to_hash, true_block.hash);
                    assert_eq!(
                        reverse_blocks,
                        old_chain.iter().rev().cloned().collect::<Vec<_>>()
                    );
                    assert_eq!(
                        forward_blocks.iter().map(|x| x.hash).collect::<Vec<_>>(),
                        new_chain.iter().map(|x| x.hash).collect::<Vec<_>>()
                    );
                }
                _ => unreachable!(),
            }
        });
    }

    #[test]
    fn test_receive_chain_reorg() -> Result<(), Reason> {
        new_test_ext().execute_with(|| {
            initialize_storage();
            pallet_oracle::Prices::insert(
                ETH.ticker,
                Price::from_nominal(ETH.ticker, "2000.00").value,
            );

            let reorg_block_hash = [3; 32];
            let real_block_hash = [5; 32];

            let reorg_event = EthereumEvent::Lock {
                asset: [238; 20],
                sender: [3; 20],
                chain: String::from("ETH"),
                recipient: [4; 32],
                amount: qty!("10", ETH).value,
            };

            let real_event = EthereumEvent::Lock {
                sender: [3; 20],
                chain: String::from("ETH"),
                recipient: [5; 32],
                amount: qty!("9", ETH).value,
                asset: [238; 20],
            };

            let reorg_block = ethereum_client::EthereumBlock {
                hash: reorg_block_hash,
                parent_hash: premined_block().hash,
                number: 2,
                events: vec![reorg_event.clone()],
            };

            let real_block = ethereum_client::EthereumBlock {
                hash: real_block_hash,
                parent_hash: premined_block().hash,
                number: 2,
                events: vec![real_event.clone()],
            };

            let latest_hash = [10; 32];

            // mine dummy blocks to get past limit
            let blocks_3 = ChainBlocks::Eth(vec![
                ethereum_client::EthereumBlock {
                    hash: [3; 32],
                    parent_hash: reorg_block_hash,
                    number: 3,
                    events: vec![],
                },
                ethereum_client::EthereumBlock {
                    hash: [4; 32],
                    parent_hash: [3; 32],
                    number: 4,
                    events: vec![],
                },
                ethereum_client::EthereumBlock {
                    hash: latest_hash,
                    parent_hash: [4; 32],
                    number: 5,
                    events: vec![],
                },
            ]);

            let reorg = ChainReorg::Eth {
                from_hash: latest_hash,
                to_hash: real_block_hash,
                reverse_blocks: vec![reorg_block.clone()],
                forward_blocks: vec![real_block.clone()],
            };

            // apply the to-be reorg'd block and a dummy block so that it is ingressed, show that the event was applied
            assert_ok!(all_receive_chain_blocks(&ChainBlocks::Eth(vec![
                reorg_block
            ])));
            let event_queue = get_event_queue::<Test>(ChainId::Eth)?;
            assert_eq!(event_queue, ChainBlockEvents::Eth(vec![(2, reorg_event)]));

            assert_ok!(all_receive_chain_blocks(&blocks_3));
            assert_eq!(
                AssetBalances::get(&Eth, ChainAccount::Eth([4; 20])),
                bal!("10", ETH).value
            );
            assert_eq!(PendingChainReorgs::get(ChainId::Eth), vec![]);

            // val a sends reorg, tally started
            assert_ok!(a_receive_chain_reorg(&reorg), ());
            assert_eq!(
                PendingChainReorgs::get(ChainId::Eth),
                vec![ChainReorgTally::new(ChainId::Eth, reorg.clone(), &val_a())]
            );

            // val b sends reorg and show reorg is executed and the new event is applied and the old one is reverted
            assert_ok!(b_receive_chain_reorg(&reorg), ());
            assert_eq!(
                LastProcessedBlock::get(ChainId::Eth),
                Some(ChainBlock::Eth(real_block))
            );
            assert_eq!(
                PendingChainBlocks::get(ChainId::Eth),
                Vec::<ChainBlockTally>::new()
            );
            assert_eq!(PendingChainReorgs::get(ChainId::Eth), vec![]);
            let event_queue = get_event_queue::<Test>(ChainId::Eth)?;
            assert_eq!(
                event_queue,
                ChainBlockEvents::Eth(vec![(2, real_event.clone())])
            );

            // mine a block so that block is ingressed
            // mine dummy blocks to get past limit
            let blocks_4 = ChainBlocks::Eth(vec![
                ethereum_client::EthereumBlock {
                    hash: [3; 32],
                    parent_hash: real_block_hash,
                    number: 3,
                    events: vec![],
                },
                ethereum_client::EthereumBlock {
                    hash: [4; 32],
                    parent_hash: [3; 32],
                    number: 4,
                    events: vec![],
                },
                ethereum_client::EthereumBlock {
                    hash: [5; 32],
                    parent_hash: [4; 32],
                    number: 5,
                    events: vec![],
                },
            ]);
            assert_ok!(all_receive_chain_blocks(&blocks_4));

            assert_eq!(
                AssetBalances::get(&Eth, ChainAccount::Eth([5; 20])),
                bal!("9", ETH).value
            );

            assert_eq!(
                AssetBalances::get(&Eth, ChainAccount::Eth([4; 20])),
                bal!("0", ETH).value
            );

            Ok(())
        })
    }

    #[test]
    fn test_collect_rev() {
        let x = vec![1, 2, 3];
        let y = x.iter().map(|v| v + 1).collect_rev();
        assert_eq!(y, vec![4, 3, 2]);
    }

    #[test]
    fn test_receive_chain_blocks_fails_for_signed_origin() {
        new_test_ext().execute_with(|| {
            let blocks = ChainBlocks::Eth(vec![]);
            let signature = ChainSignature::Eth([0u8; 65]);
            assert_err!(
                CashModule::receive_chain_blocks(
                    Origin::signed(Default::default()),
                    blocks,
                    signature
                ),
                DispatchError::BadOrigin
            );
        });
    }

    #[test]
    fn test_receive_chain_blocks_fails_for_invalid_signature() {
        new_test_ext().execute_with(|| {
            let blocks = ChainBlocks::Eth(vec![]);
            let signature = ChainSignature::Eth([0u8; 65]);
            assert_err!(
                CashModule::receive_chain_blocks(Origin::none(), blocks, signature),
                Reason::CryptoError(gateway_crypto::CryptoError::RecoverError),
            );
        });
    }

    #[test]
    fn test_receive_chain_blocks_fails_if_not_validator() {
        new_test_ext().execute_with(|| {
            let blocks = ChainBlocks::Eth(vec![]);
            let signature = ChainId::Eth.sign(&blocks.encode()).unwrap();
            assert_err!(
                CashModule::receive_chain_blocks(Origin::none(), blocks, signature),
                Reason::UnknownValidator
            );
        });
    }

    #[test]
    fn test_receive_chain_blocks_happy_path() -> Result<(), Reason> {
        sp_tracing::try_init_simple(); // Tip: add/remove from tests for logging

        new_test_ext().execute_with(|| {
            initialize_storage();

            pallet_oracle::Prices::insert(
                ETH.ticker,
                Price::from_nominal(ETH.ticker, "2000.00").value,
            );

            let event = ethereum_client::EthereumEvent::Lock {
                asset: [238; 20],
                sender: [3; 20],
                chain: String::from("ETH"),
                recipient: [2; 32],
                amount: qty!("75", ETH).value,
            };
            let blocks_2 = ChainBlocks::Eth(vec![ethereum_client::EthereumBlock {
                hash: [2; 32],
                parent_hash: premined_block().hash,
                number: 2,
                events: vec![event.clone()],
            }]);
            let blocks_3 = ChainBlocks::Eth(vec![
                ethereum_client::EthereumBlock {
                    hash: [3; 32],
                    parent_hash: [2; 32],
                    number: 3,
                    events: vec![],
                },
                ethereum_client::EthereumBlock {
                    hash: [4; 32],
                    parent_hash: [3; 32],
                    number: 4,
                    events: vec![],
                },
                ethereum_client::EthereumBlock {
                    hash: [5; 32],
                    parent_hash: [4; 32],
                    number: 5,
                    events: vec![],
                },
            ]);
            let blocks_4 = ChainBlocks::Eth(vec![ethereum_client::EthereumBlock {
                hash: [6; 32],
                parent_hash: [5; 32],
                number: 6,
                events: vec![],
            }]);

            assert_eq!(
                AssetBalances::get(&Eth, ChainAccount::Eth([2; 20])),
                bal!("0", ETH).value
            );

            // Sign and dispatch from first validator
            assert_ok!(a_receive_chain_blocks(&blocks_2));

            // Block should be pending, nothing in event queue yet
            let pending_blocks = PendingChainBlocks::get(ChainId::Eth);
            assert_eq!(pending_blocks.len(), 1);
            let event_queue = get_event_queue::<Test>(ChainId::Eth)?;
            assert_eq!(event_queue, ChainBlockEvents::Eth(vec![]));

            // Sign and dispatch from second validator
            assert_ok!(b_receive_chain_blocks(&blocks_2));

            // First round is too new - not yet processed
            let pending_blocks = PendingChainBlocks::get(ChainId::Eth);
            assert_eq!(pending_blocks.len(), 0);
            let event_queue = get_event_queue::<Test>(ChainId::Eth)?;
            assert_eq!(event_queue, ChainBlockEvents::Eth(vec![(2, event)]));

            // Receive enough blocks to ingress another round
            assert_ok!(all_receive_chain_blocks(&blocks_3));

            // Second round should still be over quota - not yet processed
            let pending_blocks = PendingChainBlocks::get(ChainId::Eth);
            assert_eq!(pending_blocks.len(), 0);
            let event_queue = get_event_queue::<Test>(ChainId::Eth)?;
            assert_eq!(event_queue.len(), 1);

            // Receive enough blocks to ingress another round
            assert_ok!(all_receive_chain_blocks(&blocks_4));

            // Third round should process
            let pending_blocks = PendingChainBlocks::get(ChainId::Eth);
            assert_eq!(pending_blocks.len(), 0);
            let event_queue = get_event_queue::<Test>(ChainId::Eth)?;
            assert_eq!(event_queue.len(), 0);

            // Check the final balance
            assert_eq!(
                AssetBalances::get(&Eth, ChainAccount::Eth([2; 20])),
                bal!("75", ETH).value
            );

            Ok(())
        })
    }
}
