use crate::{
    chains::ChainAsset,
    core::get_recent_timestamp,
    factor::Factor,
    internal,
    params::GATEWAY_VOID,
    reason::Reason,
    types::{AssetIndex, CashPrincipalAmount, Quantity, Timestamp, CASH},
    BorrowIndices, CashPrincipals, CashYield, CashYieldNext, Config, Event, GlobalCashIndex,
    LastBlockTimestamp, LastMinerSharePrincipal, LastYieldCashIndex, LastYieldTimestamp,
    MinerCumulative, Module, SupplyIndices, SupportedAssets, TotalBorrowAssets, TotalCashPrincipal,
    TotalSupplyAssets,
};
use frame_support::storage::{IterableStorageMap, StorageMap, StorageValue};

//  初始化区块的钩子
/// Block initialization hook
pub fn on_initialize<T: Config>() -> Result<(), Reason> {
    initialize_block::<T>(get_recent_timestamp::<T>()?)
}

/// Initialize block, given now
pub fn initialize_block<T: Config>(now: Timestamp) -> Result<(), Reason> {
    //  上一次利率修改时间
    let last_yield_timestamp = LastYieldTimestamp::get();
    //  上一次出块时间
    let last_block_timestamp = LastBlockTimestamp::get();

    //  如果上一个区块没有(这是第一个区块)，那么不需要计算利息，直接返回
    // If this is the first block, no interest is accrued, just record the timestamp
    if last_block_timestamp == 0 {
        LastBlockTimestamp::put(now);
        return Ok(());
    }
    //  遍历所有支持的资产，他们上个区块中产生的cash利息或者支付的cash
    // Iterate through listed assets, summing the CASH principal they generated/paid last block
    //  从现在截止上次出块经历的时间
    let dt_since_last_block = now
        .checked_sub(last_block_timestamp)
        .ok_or(Reason::TimeTravelNotAllowed)?;
    //  现在截止上次利率调整经过的时间    
    let dt_since_last_yield = now
        .checked_sub(last_yield_timestamp)
        .ok_or(Reason::TimeTravelNotAllowed)?;
    let mut cash_principal_supply_increase = CashPrincipalAmount::ZERO;
    let mut cash_principal_borrow_increase = CashPrincipalAmount::ZERO;

    let last_block_cash_index = GlobalCashIndex::get();
    let last_yield_cash_index = LastYieldCashIndex::get();
    let cash_yield = CashYield::get();
    //  cash的价格
    let price_cash = internal::assets::get_price_or_zero::<T>(CASH);

    let mut asset_updates: Vec<(ChainAsset, AssetIndex, AssetIndex)> = Vec::new();
    for (asset, asset_info) in SupportedAssets::iter() {
        //  借款年化，存款年化
        let (asset_cost, asset_yield) = internal::assets::get_rates::<T>(asset)?;
        let asset_units = asset_info.units();
        //  资产价格，美元计价
        let price_asset = internal::assets::get_price_or_zero::<T>(asset_units);
        //  资产和cash的比率，一个资产等于多少cash
        let price_ratio = Factor::ratio(price_asset, price_cash)?;
        //  单位资产的cash计价利息，折合单个区块年化，除以cashIndex折合成本金
        let cash_borrow_principal_per_asset = last_block_cash_index
            .cash_principal_per_asset(asset_cost.simple(dt_since_last_block)?, price_ratio)?;
        let cash_hold_principal_per_asset = last_block_cash_index
            .cash_principal_per_asset(asset_yield.simple(dt_since_last_block)?, price_ratio)?;
        //  存款比率
        let supply_index = SupplyIndices::get(&asset);
        //  借款比率
        let borrow_index = BorrowIndices::get(&asset);
        //  获得新的存借款比率，cashIndex和assetIndex有点像合计系数，包含了所有历史利息
        let supply_index_new = supply_index.increment(cash_hold_principal_per_asset)?;
        let borrow_index_new = borrow_index.increment(cash_borrow_principal_per_asset)?;

        let supply_asset = Quantity::new(TotalSupplyAssets::get(asset), asset_units);
        let borrow_asset = Quantity::new(TotalBorrowAssets::get(asset), asset_units);
        //  新增的存款
        cash_principal_supply_increase = cash_principal_supply_increase
            .add(cash_hold_principal_per_asset.cash_principal_amount(supply_asset)?)?;
        //  新增的利息
        cash_principal_borrow_increase = cash_principal_borrow_increase
            .add(cash_borrow_principal_per_asset.cash_principal_amount(borrow_asset)?)?;

        asset_updates.push((asset.clone(), supply_index_new, borrow_index_new));
    }
    //  支付矿工费并且更新cash的利息index
    // Pay miners and update the CASH interest index on CASH itself
    let total_cash_principal = TotalCashPrincipal::get();
    //  复利的增加额度
    let increment = cash_yield.compound(dt_since_last_yield)?;
    //  新的cashIndex
    let cash_index_new = last_yield_cash_index.increment(increment.into())?;
    //  新的cash本金
    let total_cash_principal_new = total_cash_principal.add(cash_principal_borrow_increase)?;
    //  矿工吃的的利息差
    let miner_share_principal =
        cash_principal_borrow_increase.sub(cash_principal_supply_increase)?;

    let last_miner = internal::miner::get_some_miner::<T>(); // Miner not yet set for this block, so this is "last miner"
    //  上一次矿工份额
    let last_miner_share_principal = LastMinerSharePrincipal::get();
    let miner_cash_principal_old = CashPrincipals::get(&last_miner);
    let miner_cash_principal_new =
        miner_cash_principal_old.add_amount(last_miner_share_principal)?;

    //  给矿工加钱
    // Auxiliary cumulative values
    let miner_cumulative = MinerCumulative::get(&last_miner).add(last_miner_share_principal)?;

    // * BEGIN STORAGE ALL CHECKS AND FAILURES MUST HAPPEN ABOVE * //
    //  更新矿工的yue
    CashPrincipals::insert(last_miner, miner_cash_principal_new);
    //  各个资产对应的资产更新
    for (asset, new_supply_index, new_borrow_index) in asset_updates.drain(..) {
        SupplyIndices::insert(asset.clone(), new_supply_index);
        BorrowIndices::insert(asset, new_borrow_index);
    }
    //  总的系数更新
    GlobalCashIndex::put(cash_index_new);
    //  总的cash更新
    TotalCashPrincipal::put(total_cash_principal_new);
    LastMinerSharePrincipal::put(miner_share_principal);
    LastBlockTimestamp::put(now);

    // Auxiliary cumulative values
    MinerCumulative::insert(last_miner, miner_cumulative);

    //  如果到了时间点，更新利率
    // Possibly rotate in any scheduled next CASH rate
    if let Some((next_apr, next_start)) = CashYieldNext::get() {
        if next_start <= now {
            LastYieldTimestamp::put(next_start);
            LastYieldCashIndex::put(cash_index_new);
            CashYield::put(next_apr);
            CashYieldNext::kill();
        }
    }
    //  发送矿工费事件
    if last_miner_share_principal != CashPrincipalAmount::ZERO {
        // No need to emit events when nothing happens
        <Module<T>>::deposit_event(Event::TransferCash(
            GATEWAY_VOID,
            last_miner,
            last_miner_share_principal,
            cash_index_new,
        ));
    }

    <Module<T>>::deposit_event(Event::MinerPaid(last_miner, last_miner_share_principal));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::*;

    #[test]
    fn test_on_initialize() {
        new_test_ext().execute_with(|| {
            let miner = ChainAccount::Eth([0; 20]);
            let asset = Eth;
            let asset_info = AssetInfo {
                rate_model: InterestRateModel::new_kink(0, 2500, Factor::from_nominal("0.5"), 5000),
                miner_shares: MinerShares::from_nominal("0.02"),
                ..AssetInfo::minimal(asset, ETH)
            };
            let last_yield_timestamp = 10;
            let now = last_yield_timestamp + MILLISECONDS_PER_YEAR / 4; // 3 months go by

            Miner::put(miner);
            LastBlockTimestamp::put(last_yield_timestamp);
            LastYieldTimestamp::put(last_yield_timestamp);
            SupportedAssets::insert(&asset, asset_info);
            GlobalCashIndex::put(CashIndex::from_nominal("1.123"));
            LastYieldCashIndex::put(CashIndex::from_nominal("1.123"));
            SupplyIndices::insert(&asset, AssetIndex::from_nominal("1234"));
            BorrowIndices::insert(&asset, AssetIndex::from_nominal("1345"));
            TotalSupplyAssets::insert(asset.clone(), asset_info.as_quantity_nominal("300").value);
            TotalBorrowAssets::insert(asset.clone(), asset_info.as_quantity_nominal("150").value);
            CashYield::put(APR::from_nominal("0.24")); // 24% APR big number for easy to see interest
            TotalCashPrincipal::put(CashPrincipalAmount::from_nominal("450000")); // 450k cash principal
            CashPrincipals::insert(&miner, CashPrincipal::from_nominal("1"));
            pallet_oracle::Prices::insert(
                asset_info.ticker,
                1450_000000 as pallet_oracle::types::AssetPrice,
            ); // $1450 eth

            let result = initialize_block::<Test>(now);
            let shares = CashPrincipalAmount(242097062);
            let cash_index = CashIndex::from_nominal("1.192441828000000000");
            assert_eq!(result, Ok(()));

            assert_eq!(
                SupplyIndices::get(&asset),
                AssetIndex::from_nominal("1273.542520035618878005")
            );
            assert_eq!(
                BorrowIndices::get(&asset),
                AssetIndex::from_nominal("1425.699020480854853072")
            );

            // note - the cash index number below is quite round due to the polynomial nature of
            // our approximation and the fact that the ratio in this case worked out to be a
            // base 10 number that terminates in that many digits.
            assert_eq!(GlobalCashIndex::get(), cash_index);
            assert_eq!(
                TotalCashPrincipal::get(),
                CashPrincipalAmount::from_nominal("462104.853072")
            );
            assert_eq!(CashPrincipals::get(&miner), CashPrincipal::ONE);
            assert_eq!(LastMinerSharePrincipal::get(), shares);
            assert_eq!(MinerCumulative::get(&miner), CashPrincipalAmount(0));

            // Run again to give last block principal to miner
            assert_eq!(initialize_block::<Test>(now), Ok(()));
            assert_eq!(
                CashPrincipals::get(&miner),
                CashPrincipal::from_nominal("243.097062")
            );
            assert_eq!(LastMinerSharePrincipal::get(), CashPrincipalAmount(0));
            assert_eq!(MinerCumulative::get(&miner), shares);

            let mut events_iter = System::events().into_iter();
            let miner_paid_event_1 = events_iter.next().unwrap();
            let transfer_cash_event_1 = events_iter.next().unwrap();
            let miner_paid_event_2 = events_iter.next().unwrap();
            assert_eq!(
                mock::Event::pallet_cash(crate::Event::MinerPaid(miner, CashPrincipalAmount(0))),
                miner_paid_event_1.event
            );
            assert_eq!(
                mock::Event::pallet_cash(crate::Event::TransferCash(
                    GATEWAY_VOID,
                    miner,
                    shares,
                    cash_index
                )),
                transfer_cash_event_1.event
            );
            assert_eq!(
                mock::Event::pallet_cash(crate::Event::MinerPaid(miner, shares)),
                miner_paid_event_2.event
            );
            // should be exactly 3 events
            assert!(events_iter.next().is_none());
        });
    }

    #[test]
    fn test_on_initialize_next_yield_progression() {
        new_test_ext().execute_with(|| {
            let now = MILLISECONDS_PER_YEAR; // we are at year 1 (from epoc)
            let last_yield_timestamp = 3 * MILLISECONDS_PER_YEAR / 4; // last time we set the yield was 3 months ago
            let last_block_timestamp = now - 6 * 1000; // last block was 6 seconds ago
            let next_yield_timestamp = now + 6 * 1000; // we are going to set the next yield in the next block 6 seconds from "now"
            let global_cash_index_initial = CashIndex::from_nominal("1.123");
            let last_yield_cash_index_initial = CashIndex::from_nominal("1.111");
            let cash_yield_initial = APR::from_nominal("0.24");
            let cash_yield_next = APR::from_nominal("0.32");

            LastBlockTimestamp::put(last_block_timestamp);
            LastYieldTimestamp::put(last_yield_timestamp);
            CashYield::put(cash_yield_initial);
            CashYieldNext::put((cash_yield_next, next_yield_timestamp));
            GlobalCashIndex::put(global_cash_index_initial);
            LastYieldCashIndex::put(last_yield_cash_index_initial);
            LastYieldTimestamp::put(last_yield_timestamp);

            let result = initialize_block::<Test>(now);
            assert_eq!(result, Ok(()));

            let increment_expected = cash_yield_initial
                .compound(now - last_yield_timestamp)
                .expect("could not compound interest during expected calc");
            let new_index_expected = last_yield_cash_index_initial
                .increment(increment_expected.into())
                .expect("could not increment index value during expected calc");
            let new_index_actual = GlobalCashIndex::get();
            assert_eq!(
                new_index_expected, new_index_actual,
                "current yield was not used to calculate the cash index increment"
            );
            let (next_yield_actual, next_yield_timestamp_actual) =
                CashYieldNext::get().expect("cash yield next was cleared unexpectedly");
            assert_eq!(
                next_yield_actual, cash_yield_next,
                "cash yield next yield was modified unexpectedly"
            );
            assert_eq!(
                next_yield_timestamp_actual, next_yield_timestamp,
                "cash yield next timestamp was modified unexpectedly"
            );
            assert_eq!(
                LastYieldTimestamp::get(),
                last_yield_timestamp,
                "last yield timestamp changed unexpectedly"
            );
            assert_eq!(
                LastYieldCashIndex::get(),
                last_yield_cash_index_initial,
                "last yield cash index was updated unexpectedly"
            );

            // simulate "a block goes by" in time
            let last_block_timestamp = now;
            let now = next_yield_timestamp;
            LastBlockTimestamp::put(last_block_timestamp);

            let result = initialize_block::<Test>(now);
            assert_eq!(result, Ok(()));

            let increment_expected = cash_yield_initial
                .compound(now - last_yield_timestamp)
                .expect("could not compound interest during expected calc");
            let new_index_expected = last_yield_cash_index_initial
                .increment(increment_expected.into())
                .expect("could not increment index value during expected calc");
            let new_index_actual = GlobalCashIndex::get();
            assert_eq!(
                new_index_expected, new_index_actual,
                "current yield was not used to calculate the cash index increment"
            );
            assert!(CashYieldNext::get().is_none(), "next yield was not cleared");
            let actual_cash_yield = CashYield::get();
            assert_eq!(
                actual_cash_yield, cash_yield_next,
                "the current yield was not updated to next yield"
            );
            assert_eq!(
                LastYieldTimestamp::get(),
                next_yield_timestamp,
                "last yield timestamp was not updated"
            );
            assert_eq!(
                LastYieldCashIndex::get(),
                new_index_actual,
                "last yield cash index was not updated correctly"
            );

            // simulate "a block goes by" in time
            let last_block_timestamp = now;
            let now = now + 6 * 1000;
            let new_cash_index_baseline = new_index_actual;
            LastBlockTimestamp::put(last_block_timestamp);
            LastYieldTimestamp::put(next_yield_timestamp);

            let result = initialize_block::<Test>(now);
            assert_eq!(result, Ok(()));

            let increment_expected = cash_yield_next
                .compound(now - next_yield_timestamp)
                .expect("could not compound interest during expected calc");
            let new_index_expected = new_cash_index_baseline
                .increment(increment_expected.into())
                .expect("could not increment index value during expected calc");
            let new_index_actual = GlobalCashIndex::get();
            assert_eq!(
                new_index_expected, new_index_actual,
                "current yield was not used to calculate the cash index increment"
            );
            assert!(CashYieldNext::get().is_none(), "next yield was not cleared");
            let actual_cash_yield = CashYield::get();
            assert_eq!(
                actual_cash_yield, cash_yield_next,
                "the current yield was not updated to next yield"
            );
            assert_eq!(
                LastYieldTimestamp::get(),
                next_yield_timestamp,
                "last yield timestamp was modified unexpectedly"
            );
            assert_eq!(
                LastYieldCashIndex::get(),
                new_cash_index_baseline,
                "last yield cash index was updated unexpectedly"
            );
        });
    }
}
