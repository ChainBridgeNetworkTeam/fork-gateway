use frame_support::{
    storage::{IterableStorageDoubleMap, StorageDoubleMap, StorageMap, StorageValue},
    traits::StoredMap,
};
use our_std::collections::btree_map::BTreeMap;
use our_std::RuntimeDebug;
use sp_core::crypto::AccountId32;

use crate::{
    chains::{ChainAccount, ChainId},
    internal::balance_helpers::*,
    params::MIN_PRINCIPAL_GATE,
    portfolio::Portfolio,
    reason::Reason,
    types::{
        AssetBalance, AssetIndex, AssetInfo, Balance, CashPrincipal, CashPrincipalAmount, Quantity,
    },
    AssetAmount, AssetBalances, AssetsWithNonZeroBalance, BorrowIndices, CashPrincipals,
    ChainAsset, ChainCashPrincipals, Config, GlobalCashIndex, LastIndices, SupplyIndices,
    SupportedAssets, TotalBorrowAssets, TotalCashPrincipal, TotalSupplyAssets,
};

trait Apply {
    fn apply<T: Config>(self: Self, state: State) -> Result<State, Reason>;
}

#[derive(Clone, Eq, PartialEq, RuntimeDebug)]
pub struct State {
    total_supply_asset: BTreeMap<ChainAsset, AssetAmount>,
    total_borrow_asset: BTreeMap<ChainAsset, AssetAmount>,
    asset_balances: BTreeMap<(ChainAsset, ChainAccount), AssetBalance>,
    assets_with_non_zero_balance: BTreeMap<(ChainAsset, ChainAccount), bool>,
    last_indices: BTreeMap<(ChainAsset, ChainAccount), AssetIndex>,
    cash_principals: BTreeMap<ChainAccount, CashPrincipal>,
    total_cash_principal: Option<CashPrincipalAmount>,
    chain_cash_principals: BTreeMap<ChainId, CashPrincipalAmount>,
}

//  账户状态？
//  总的链上状态
impl State {
    pub fn new() -> Self {
        State {
            total_supply_asset: BTreeMap::new(),
            total_borrow_asset: BTreeMap::new(),
            asset_balances: BTreeMap::new(),
            assets_with_non_zero_balance: BTreeMap::new(),
            last_indices: BTreeMap::new(),
            //  各个账户和cash本金的映射表
            cash_principals: BTreeMap::new(),
            //  总的cash本金
            total_cash_principal: None,
            chain_cash_principals: BTreeMap::new(),
        }
    }
    //  得到资产组合，总的balance和各个资产的详细信息
    pub fn build_portfolio<T: Config>(
        self: &Self,
        account: ChainAccount,
    ) -> Result<Portfolio, Reason> {
        let mut principal = self.get_cash_principal::<T>(account);
        let global_cash_index = GlobalCashIndex::get();

        let mut positions = Vec::new();
        //  该账号所有非0的资产
        for asset in self.get_assets_with_non_zero_balance::<T>(account) {
            //  资产信息
            let asset_info = SupportedAssets::get(asset).ok_or(Reason::AssetNotSupported)?;
            //  存款利率
            let supply_index = SupplyIndices::get(asset);
            //  借款指数
            let borrow_index = BorrowIndices::get(asset);
            //  余额
            let balance = self.get_asset_balance::<T>(asset_info, account);
            //  上一个指数
            let last_index = self.get_last_index::<T>(asset_info, account);
            //  返回利息，新的资产利率，余额变化先前的位置和当前的市场利率
            (principal, _) = effect_of_asset_interest_internal(
                balance,
                balance,
                principal,
                last_index,
                supply_index,
                borrow_index,
            )?;
            positions.push((asset_info, balance));
        }
        //  本金乘以cashIndex得到余额
        let cash = global_cash_index.cash_balance(principal)?;
        //  返回总的fcash和位置
        Ok(Portfolio { cash, positions })
    }
    //  获取总的资产
    pub fn get_total_supply_asset<T: Config>(self: &Self, asset_info: AssetInfo) -> Quantity {
        asset_info.as_quantity(
            self.total_supply_asset
                .get(&asset_info.asset)
                .map(|x| *x)
                .unwrap_or_else(|| TotalSupplyAssets::get(asset_info.asset)),
        )
    }
    //  设置总资产
    pub fn set_total_supply_asset<T: Config>(
        self: &mut Self,
        asset_info: AssetInfo,
        quantity: Quantity,
    ) {
        self.total_supply_asset
            .insert(asset_info.asset, quantity.value);
    }
    //  获取总的借的资产
    pub fn get_total_borrow_asset<T: Config>(self: &Self, asset_info: AssetInfo) -> Quantity {
        asset_info.as_quantity(
            self.total_borrow_asset
                .get(&asset_info.asset)
                .map(|x| *x)
                .unwrap_or_else(|| TotalBorrowAssets::get(asset_info.asset)),
        )
    }
    //  设置总的借资产
    pub fn set_total_borrow_asset<T: Config>(
        self: &mut Self,
        asset_info: AssetInfo,
        quantity: Quantity,
    ) {
        self.total_borrow_asset
            .insert(asset_info.asset, quantity.value);
    }
    //  获取资产余额
    pub fn get_asset_balance<T: Config>(
        self: &Self,
        asset_info: AssetInfo,
        account: ChainAccount,
    ) -> Balance {
        asset_info.as_balance(
            self.asset_balances
                .get(&(asset_info.asset, account))
                .map(|x| *x)
                .unwrap_or_else(|| AssetBalances::get(asset_info.asset, account)),
        )
    }
    //  设置资产余额
    pub fn set_asset_balance<T: Config>(
        self: &mut Self,
        asset_info: AssetInfo,
        account: ChainAccount,
        balance: Balance,
    ) {
        self.assets_with_non_zero_balance
            .insert((asset_info.asset, account), balance.value != 0);
        self.asset_balances
            .insert((asset_info.asset, account), balance.value);
    }
    //  结合真实情况和修改后的状态，以非零余额返回当前资产
    // Combines ground truth and modified state to return current assets with non-zero balance
    fn get_assets_with_non_zero_balance<T: Config>(
        self: &Self,
        account: ChainAccount,
    ) -> Vec<ChainAsset> {
        //  如果链上状态里有但是这个克隆实例里没有，就复制一个
        let mut assets = self.assets_with_non_zero_balance.clone();
        for (asset, _) in AssetsWithNonZeroBalance::iter_prefix(account) {
            if !assets.contains_key(&(asset, account)) {
                assets.insert((asset, account), true);
            }
        }
        //  返回所有非0资产
        assets
            .iter()
            .filter_map(|((asset, account_el), is_non_zero)| {
                if account == *account_el && *is_non_zero {
                    Some(*asset)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    }
    //  获取最近的利率
    pub fn get_last_index<T: Config>(
        self: &Self,
        asset_info: AssetInfo,
        account: ChainAccount,
    ) -> AssetIndex {
        self.last_indices
            .get(&(asset_info.asset, account))
            .map(|x| *x)
            .unwrap_or_else(|| LastIndices::get(asset_info.asset, account))
    }
    //  设置最近的利息
    pub fn set_last_index<T: Config>(
        self: &mut Self,
        asset_info: AssetInfo,
        account: ChainAccount,
        last_index: AssetIndex,
    ) {
        self.last_indices
            .insert((asset_info.asset, account), last_index);
    }
    //  获取cash资本
    pub fn get_cash_principal<T: Config>(self: &Self, account: ChainAccount) -> CashPrincipal {
        self.cash_principals
            .get(&account)
            .map(|x| *x)
            .unwrap_or_else(|| CashPrincipals::get(account))
    }
    //  设置cash本金
    pub fn set_cash_principal<T: Config>(
        self: &mut Self,
        account: ChainAccount,
        cash_principal: CashPrincipal,
    ) {
        self.cash_principals.insert(account, cash_principal);
    }
    //  获取总的cash本金
    pub fn get_total_cash_principal<T: Config>(self: &Self) -> CashPrincipalAmount {
        self.total_cash_principal
            .unwrap_or_else(|| TotalCashPrincipal::get())
    }
    //  设置cash聪的本金
    pub fn set_total_cash_principal<T: Config>(
        self: &mut Self,
        total_cash_principal: CashPrincipalAmount,
    ) {
        self.total_cash_principal = Some(total_cash_principal);
    }
    //  获取脸上cash本金
    pub fn get_chain_cash_principal<T: Config>(
        self: &Self,
        chain_id: ChainId,
    ) -> CashPrincipalAmount {
        self.chain_cash_principals
            .get(&chain_id)
            .map(|x| *x)
            .unwrap_or_else(|| ChainCashPrincipals::get(chain_id))
    }
    //  设置链上cash本金
    pub fn set_chain_cash_principal<T: Config>(
        self: &mut Self,
        chain_id: ChainId,
        chain_cash_principal: CashPrincipalAmount,
    ) {
        self.chain_cash_principals
            .insert(chain_id, chain_cash_principal);
    }
    //  提交改动
    pub fn commit<T: Config>(self: &Self) {
        //  把这个state里面的改动同步到链上状态
        //  从这里看，只是同步链上状态，并没有同对等链有交互
        self.total_supply_asset
            .iter()
            .for_each(|(chain_asset, asset_amount)| {
                TotalSupplyAssets::insert(chain_asset, asset_amount);
            });
        self.total_borrow_asset
            .iter()
            .for_each(|(chain_asset, asset_amount)| {
                TotalBorrowAssets::insert(chain_asset, asset_amount);
            });
        self.asset_balances
            .iter()
            .for_each(|((chain_asset, account), balance)| {
                AssetBalances::insert(chain_asset, account, balance);
            });
        self.assets_with_non_zero_balance.iter().for_each(
            |((chain_asset, account), is_non_zero)| {
                if *is_non_zero {
                    AssetsWithNonZeroBalance::insert(account, chain_asset, ());
                } else {
                    AssetsWithNonZeroBalance::remove(account, chain_asset);
                }
            },
        );
        self.last_indices
            .iter()
            .for_each(|((chain_asset, account), last_index)| {
                LastIndices::insert(chain_asset, account, last_index);
            });
        //  同步本金
        self.cash_principals
            .iter()
            .for_each(|(account, cash_principal)| {
                CashPrincipals::insert(account, cash_principal);
                // 存在的最小余额检查
                // Existential balance for Gateway accounts...
                match account {
                    ChainAccount::Gate(gate_address) => {
                        //  从技术上来讲我们可以只使用inc_provider或dec_provider
                        //  但是我们只想在状态真正变化的时候这样做
                        //  现在我们只会在本金变化的时候低效的这样做
                        //  从技术上讲，这些调用有可能失败（尽管那可能是安全），但是还是可能会触发链上错误
                        // Note: Technically we could just inc_provider/dec_provider
                        //  however we only want to do so when the state actually changes.
                        //  For now we just inefficiently write to the StoredMap each time principal changes.
                        // Also note: Technically these StoredMap calls can fail (though probably provably safe),
                        //  which would presumably trigger the underlying panic this is meant to avoid.
                        if cash_principal >= &MIN_PRINCIPAL_GATE {
                            _ = T::AccountStore::insert(&AccountId32::new(*gate_address), ());
                        } else {
                            _ = T::AccountStore::remove(&AccountId32::new(*gate_address));
                        }
                    }

                    _ => {}
                }
            });
        if let Some(total_cash_principal_new) = self.total_cash_principal {
            TotalCashPrincipal::put(total_cash_principal_new);
        }
        self.chain_cash_principals
            .iter()
            .for_each(|(chain_id, chain_cash_principal)| {
                ChainCashPrincipals::insert(chain_id, chain_cash_principal);
            });
    }
}
//  准备扩充资产，用来打钱？
//  锁定了金额，所以要记账，表现为打钱
fn prepare_augment_asset<T: Config>(
    mut st: State,
    recipient: ChainAccount,
    asset: ChainAsset,
    quantity: Quantity,
) -> Result<State, Reason> {
    let asset_info = SupportedAssets::get(asset).ok_or(Reason::AssetNotSupported)?;
    let supply_index = SupplyIndices::get(asset);
    let borrow_index = BorrowIndices::get(asset);
    let total_supply_pre = st.get_total_supply_asset::<T>(asset_info);
    let total_borrow_pre = st.get_total_borrow_asset::<T>(asset_info);
    let recipient_balance_pre = st.get_asset_balance::<T>(asset_info, recipient);
    let recipient_last_index_pre = st.get_last_index::<T>(asset_info, recipient);
    let recipient_cash_principal_pre = st.get_cash_principal::<T>(recipient);
    //  返回需要偿还的金额和注入的余额
    let (recipient_repay_amount, recipient_supply_amount) =
        repay_and_supply_amount(recipient_balance_pre.value, quantity)?;
    //  计算新的总存款
    let total_supply_new = total_supply_pre.add(recipient_supply_amount)?;
    //  计算新的总欠款
    let total_borrow_new = total_borrow_pre
        .sub(recipient_repay_amount)
        .map_err(|_| Reason::TotalBorrowUnderflow)?;
    //  扩充后的余额
    let recipient_balance_post = recipient_balance_pre.add_quantity(quantity)?;
    //  处理过后的cash本金，和最近一次的利息
    let (recipient_cash_principal_post, recipient_last_index_post) =
        effect_of_asset_interest_internal(
            recipient_balance_pre,
            recipient_balance_post,
            recipient_cash_principal_pre,
            recipient_last_index_pre,
            supply_index,
            borrow_index,
        )?;
    //   更新各种信息
    st.set_total_supply_asset::<T>(asset_info, total_supply_new);
    st.set_total_borrow_asset::<T>(asset_info, total_borrow_new);
    st.set_asset_balance::<T>(asset_info, recipient, recipient_balance_post);
    st.set_last_index::<T>(asset_info, recipient, recipient_last_index_post);
    st.set_cash_principal::<T>(recipient, recipient_cash_principal_post);

    Ok(st)
}

//  准备减资产
fn prepare_reduce_asset<T: Config>(
    mut st: State,
    sender: ChainAccount,
    asset: ChainAsset,
    quantity: Quantity,
) -> Result<State, Reason> {
    //  通用的资产信息
    let asset_info = SupportedAssets::get(asset).ok_or(Reason::AssetNotSupported)?;
    let supply_index = SupplyIndices::get(asset);
    let borrow_index = BorrowIndices::get(asset);
    let total_supply_pre = st.get_total_supply_asset::<T>(asset_info);
    let total_borrow_pre = st.get_total_borrow_asset::<T>(asset_info);
    let sender_balance_pre = st.get_asset_balance::<T>(asset_info, sender);
    let sender_last_index_pre = st.get_last_index::<T>(asset_info, sender);
    let sender_cash_principal_pre = st.get_cash_principal::<T>(sender);
    //  判断这个账号是最终是减少余额还是借钱
    let (sender_withdraw_amount, sender_borrow_amount) =
        withdraw_and_borrow_amount(sender_balance_pre.value, quantity)?;
    //  余额做减法  
    let total_supply_new = total_supply_pre
        .sub(sender_withdraw_amount)
        .map_err(|_| Reason::InsufficientTotalFunds)?;
    //  欠款做加法
    let total_borrow_new = total_borrow_pre.add(sender_borrow_amount)?;

    let sender_balance_post = sender_balance_pre.sub_quantity(quantity)?;
    //  计算过后的cash本金，这里其实指的是资产本金
    let (sender_cash_principal_post, sender_last_index_post) = effect_of_asset_interest_internal(
        sender_balance_pre,
        sender_balance_post,
        sender_cash_principal_pre,
        sender_last_index_pre,
        supply_index,
        borrow_index,
    )?;

    st.set_total_supply_asset::<T>(asset_info, total_supply_new);
    st.set_total_borrow_asset::<T>(asset_info, total_borrow_new);
    st.set_asset_balance::<T>(asset_info, sender, sender_balance_post);
    st.set_last_index::<T>(asset_info, sender, sender_last_index_post);
    st.set_cash_principal::<T>(sender, sender_cash_principal_post);

    Ok(st)
}

//  准备增加cash
fn prepare_augment_cash<T: Config>(
    mut st: State,
    recipient: ChainAccount,
    principal: CashPrincipalAmount,
    from_external: bool,
) -> Result<State, Reason> {
    let recipient_cash_pre = st.get_cash_principal::<T>(recipient);

    let (recipient_repay_principal, _recipient_supply_principal) =
        repay_and_supply_principal(recipient_cash_pre, principal)?;
    //  处理过后的cash
    let recipient_cash_post = recipient_cash_pre.add_amount(principal)?;
    let total_cash_post = st
        .get_total_cash_principal::<T>()
        .sub(recipient_repay_principal)
        .map_err(|_| Reason::InsufficientChainCash)?;

    st.set_cash_principal::<T>(recipient, recipient_cash_post);
    st.set_total_cash_principal::<T>(total_cash_post);
    //  如果来自外部，要给整个链减去对应的cash余额
    if from_external {
        let chain_id = recipient.chain_id();
        let chain_cash_principal_post = st
            .get_chain_cash_principal::<T>(chain_id)
            .sub(principal)
            .map_err(|_| Reason::NegativeChainCash)?;
        st.set_chain_cash_principal::<T>(chain_id, chain_cash_principal_post);
    }

    Ok(st)
}
//  准备减少cash
fn prepare_reduce_cash<T: Config>(
    mut st: State,
    sender: ChainAccount,
    principal: CashPrincipalAmount,
    to_external: bool,
) -> Result<State, Reason> {
    let sender_cash_pre = st.get_cash_principal::<T>(sender);
    //  看看是减少余额还是借款
    let (_sender_withdraw_principal, sender_borrow_principal) =
        withdraw_and_borrow_principal(sender_cash_pre, principal)?;
    //  过后发送者的cash
    let sender_cash_post = sender_cash_pre.sub_amount(principal)?;
    let total_cash_post = st
        .get_total_cash_principal::<T>()
        .add(sender_borrow_principal)?;

    st.set_cash_principal::<T>(sender, sender_cash_post);
    st.set_total_cash_principal::<T>(total_cash_post);
    //  如果是外部的，总的链上cash也需要增加
    if to_external {
        let chain_id = sender.chain_id();
        let chain_cash_principal_post =
            st.get_chain_cash_principal::<T>(chain_id).add(principal)?;
        st.set_chain_cash_principal::<T>(chain_id, chain_cash_principal_post);
    }

    Ok(st)
}

//  修改状态的Effect的枚举定义
#[derive(Clone, Eq, PartialEq, RuntimeDebug)]
pub enum Effect {
    //  定义资产和cash的四种操作
    AugmentAsset {
        recipient: ChainAccount,
        asset: ChainAsset,
        quantity: Quantity,
    },
    ReduceAsset {
        sender: ChainAccount,
        asset: ChainAsset,
        quantity: Quantity,
    },
    AugmentCash {
        recipient: ChainAccount,
        principal: CashPrincipalAmount,
        from_external: bool,
    },
    ReduceCash {
        sender: ChainAccount,
        principal: CashPrincipalAmount,
        to_external: bool,
    },
}

//  实现对应的四种操作
impl Apply for Effect {
    fn apply<T: Config>(self: Self, state: State) -> Result<State, Reason> {
        match self {
            Effect::AugmentAsset {
                recipient,
                asset,
                quantity,
            } => prepare_augment_asset::<T>(state, recipient, asset, quantity),
            Effect::ReduceAsset {
                sender,
                asset,
                quantity,
            } => prepare_reduce_asset::<T>(state, sender, asset, quantity),
            Effect::AugmentCash {
                recipient,
                principal,
                from_external,
            } => prepare_augment_cash::<T>(state, recipient, principal, from_external),
            Effect::ReduceCash {
                sender,
                principal,
                to_external,
            } => prepare_reduce_cash::<T>(state, sender, principal, to_external),
        }
    }
}

//  定义cash pipeline
/// Type for representing a set of positions for an account.
#[derive(Clone, Eq, PartialEq, RuntimeDebug)]
pub struct CashPipeline {
    pub effects: Vec<Effect>,
    pub state: State,
}

impl CashPipeline {
    pub fn new() -> Self {
        CashPipeline {
            effects: vec![],
            state: State::new(),
        }
    }
    //  启动apply, push一个effect
    fn apply_effect<T: Config>(mut self: Self, effect: Effect) -> Result<Self, Reason> {
        self.state = effect.clone().apply::<T>(self.state)?;
        self.effects.push(effect);
        Ok(self)
    }
    //  z转移资产
    pub fn transfer_asset<T: Config>(
        self: Self,
        sender: ChainAccount,
        recipient: ChainAccount,
        asset: ChainAsset,
        quantity: Quantity,
    ) -> Result<Self, Reason> {
        if sender == recipient {
            Err(Reason::SelfTransfer)?
        }
        //  给目标加资产
        self.apply_effect::<T>(Effect::AugmentAsset {
            recipient,
            asset,
            quantity,
        })?
        //  转账人减资产
        .apply_effect::<T>(Effect::ReduceAsset {
            sender,
            asset,
            quantity,
        })
    }
    //  锁定资产
    //  单纯给账户转钱
    pub fn lock_asset<T: Config>(
        self: Self,
        recipient: ChainAccount,
        asset: ChainAsset,
        quantity: Quantity,
    ) -> Result<Self, Reason> {
        self.apply_effect::<T>(Effect::AugmentAsset {
            recipient,
            asset,
            quantity,
        })
    }
    //  提款
    pub fn extract_asset<T: Config>(
        self: Self,
        sender: ChainAccount,
        asset: ChainAsset,
        quantity: Quantity,
    ) -> Result<Self, Reason> {
        self.apply_effect::<T>(Effect::ReduceAsset {
            sender,
            asset,
            quantity,
        })
    }
    //  cash转账
    pub fn transfer_cash<T: Config>(
        self: Self,
        sender: ChainAccount,
        recipient: ChainAccount,
        principal: CashPrincipalAmount,
    ) -> Result<Self, Reason> {
        if sender == recipient {
            Err(Reason::SelfTransfer)?
        }
        self.apply_effect::<T>(Effect::ReduceCash {
            sender,
            principal,
            to_external: false,
        })?
        .apply_effect::<T>(Effect::AugmentCash {
            recipient,
            principal,
            from_external: false,
        })
    }
    //  给cash锁定
    pub fn lock_cash<T: Config>(
        self: Self,
        recipient: ChainAccount,
        principal: CashPrincipalAmount,
    ) -> Result<Self, Reason> {
        self.apply_effect::<T>(Effect::AugmentCash {
            recipient,
            principal,
            from_external: true,
        })
    }
    //  提取cash
    pub fn extract_cash<T: Config>(
        self: Self,
        sender: ChainAccount,
        principal: CashPrincipalAmount,
    ) -> Result<Self, Reason> {
        self.apply_effect::<T>(Effect::ReduceCash {
            sender,
            principal,
            to_external: true,
        })
    }
    //  检查抵押，检查流动性
    pub fn check_collateralized<T: Config>(
        self: Self,
        account: ChainAccount,
    ) -> Result<Self, Reason> {
        let liquidity = self
            .state
            .build_portfolio::<T>(account)?
            .get_liquidity::<T>()?;
        if liquidity.value < 0 {
            Err(Reason::InsufficientLiquidity)?
        } else {
            Ok(self)
        }
    }
    //  检查是否有足够的钱
    pub fn check_underwater<T: Config>(self: Self, account: ChainAccount) -> Result<Self, Reason> {
        let liquidity = self
            .state
            .build_portfolio::<T>(account)?
            .get_liquidity::<T>()?;
        if liquidity.value > 0 {
            Err(Reason::SufficientLiquidity)?
        } else {
            Ok(self)
        }
    }
    //  检查资产余额
    pub fn check_asset_balance<T: Config, F>(
        self: Self,
        account: ChainAccount,
        asset: AssetInfo,
        check_fn: F,
    ) -> Result<Self, Reason>
    where
        F: FnOnce(Balance) -> Result<(), Reason>,
    {
        let balance = self.state.get_asset_balance::<T>(asset, account);
        check_fn(balance)?;
        Ok(self)
    }
    //  检查cash本金
    pub fn check_cash_principal<T: Config, F>(
        self: Self,
        account: ChainAccount,
        check_fn: F,
    ) -> Result<Self, Reason>
    where
        F: FnOnce(CashPrincipal) -> Result<(), Reason>,
    {
        let principal = self.state.get_cash_principal::<T>(account);
        check_fn(principal)?;
        Ok(self)
    }

    //  检查是否有足够的总资产
    // TODO: Do we need this check on other functions?
    pub fn check_sufficient_total_funds<T: Config>(
        self: Self,
        asset_info: AssetInfo,
    ) -> Result<Self, Reason> {
        let total_supply_asset = self.state.get_total_supply_asset::<T>(asset_info);
        let total_borrow_asset = self.state.get_total_borrow_asset::<T>(asset_info);
        //  如果总借出比总存款高，会报错
        if total_borrow_asset > total_supply_asset {
            Err(Reason::InsufficientTotalFunds)?
        }
        Ok(self)
    }
    //  更新状态，把链上state的改动同步到对应的全局变量中去
    pub fn commit<T: Config>(self: Self) {
        self.state.commit::<T>();
    }
}
//  返回包含资产利息的cash本金，以及一个新的资产指数
//  传入余额差，先前的位置（头寸）以及当前的市场指数
/// Return CASH Principal including asset interest, and a new asset index,
///  given a balance change, the previous position, and current market global indices.
fn effect_of_asset_interest_internal(
    balance_old: Balance,
    balance_new: Balance,
    cash_principal_pre: CashPrincipal,
    last_index: AssetIndex,
    supply_index: AssetIndex,
    borrow_index: AssetIndex,
) -> Result<(CashPrincipal, AssetIndex), Reason> {
    //  判断是借款还是存款利率or下标
    let cash_index = if balance_old.value >= 0 {
        supply_index
    } else {
        borrow_index
    };
    //  cash本金的变化，这个index像是一个用来计算利息周期的标号
    //  计算本金的变化
    let cash_principal_delta = cash_index.cash_principal_since(last_index, balance_old)?;
    //  处理过后的cash本金
    let cash_principal_post = cash_principal_pre.add(cash_principal_delta)?;
    //  更新后续的标号，存款或者借款的标号
    let last_index_post = if balance_new.value >= 0 {
        supply_index
    } else {
        borrow_index
    };
    Ok((cash_principal_post, last_index_post))
}
//  通过构建一个实例来创建资产组合
pub fn load_portfolio<T: Config>(account: ChainAccount) -> Result<Portfolio, Reason> {
    CashPipeline::new().state.build_portfolio::<T>(account)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        chains::*,
        tests::{assert_ok, assets::*, common::*, mock::*},
        types::*,
    };
    use our_std::convert::TryInto;

    #[allow(non_upper_case_globals)]
    const account_a: ChainAccount = ChainAccount::Eth([1u8; 20]);
    #[allow(non_upper_case_globals)]
    const account_b: ChainAccount = ChainAccount::Eth([2u8; 20]);

    #[test]
    fn test_transfer_asset_success_state() {
        new_test_ext().execute_with(|| {
            assert_ok!(init_eth_asset());

            let quantity = eth.as_quantity_nominal("1");
            let amount = quantity.value as i128;

            let state = CashPipeline::new()
                .transfer_asset::<Test>(account_a, account_b, Eth, quantity)
                .expect("transfer_asset failed")
                .state;

            assert_eq!(
                state,
                State {
                    total_supply_asset: vec![(Eth, quantity.value)].into_iter().collect(),
                    total_borrow_asset: vec![(Eth, quantity.value)].into_iter().collect(),
                    asset_balances: vec![((Eth, account_a), -amount), ((Eth, account_b), amount)]
                        .into_iter()
                        .collect(),
                    assets_with_non_zero_balance: vec![
                        ((Eth, account_a), true),
                        ((Eth, account_b), true)
                    ]
                    .into_iter()
                    .collect(),
                    last_indices: vec![
                        ((Eth, account_a), AssetIndex::from_nominal("0")),
                        ((Eth, account_b), AssetIndex::from_nominal("0"))
                    ]
                    .into_iter()
                    .collect(),
                    cash_principals: vec![
                        (account_a, CashPrincipal::from_nominal("0")),
                        (account_b, CashPrincipal::from_nominal("0")),
                    ]
                    .into_iter()
                    .collect(),
                    total_cash_principal: None,
                    chain_cash_principals: vec![].into_iter().collect(),
                }
            );
        })
    }

    #[test]
    fn test_lock_asset_success_state() {
        new_test_ext().execute_with(|| {
            assert_ok!(init_eth_asset());

            let quantity = eth.as_quantity_nominal("1");
            let amount = quantity.value as i128;

            let state = CashPipeline::new()
                .lock_asset::<Test>(account_a, Eth, quantity)
                .expect("lock_asset failed")
                .state;

            assert_eq!(
                state,
                State {
                    total_supply_asset: vec![(Eth, quantity.value)].into_iter().collect(),
                    total_borrow_asset: vec![(Eth, 0)].into_iter().collect(),
                    asset_balances: vec![((Eth, account_a), amount)].into_iter().collect(),
                    assets_with_non_zero_balance: vec![((Eth, account_a), true)]
                        .into_iter()
                        .collect(),
                    last_indices: vec![((Eth, account_a), AssetIndex::from_nominal("0"))]
                        .into_iter()
                        .collect(),
                    cash_principals: vec![(account_a, CashPrincipal::from_nominal("0")),]
                        .into_iter()
                        .collect(),
                    total_cash_principal: None,
                    chain_cash_principals: vec![].into_iter().collect(),
                }
            );
        })
    }

    #[test]
    fn test_extract_asset_success_state() {
        new_test_ext().execute_with(|| {
            assert_ok!(init_eth_asset());

            let quantity = eth.as_quantity_nominal("1");
            let amount = quantity.value as i128;

            let state = CashPipeline::new()
                .extract_asset::<Test>(account_a, Eth, quantity)
                .expect("extract_asset failed")
                .state;

            assert_eq!(
                state,
                State {
                    total_supply_asset: vec![(Eth, 0)].into_iter().collect(),
                    total_borrow_asset: vec![(Eth, quantity.value)].into_iter().collect(),
                    asset_balances: vec![((Eth, account_a), -amount)].into_iter().collect(),
                    assets_with_non_zero_balance: vec![((Eth, account_a), true)]
                        .into_iter()
                        .collect(),
                    last_indices: vec![((Eth, account_a), AssetIndex::from_nominal("0"))]
                        .into_iter()
                        .collect(),
                    cash_principals: vec![(account_a, CashPrincipal::from_nominal("0")),]
                        .into_iter()
                        .collect(),
                    total_cash_principal: None,
                    chain_cash_principals: vec![].into_iter().collect(),
                }
            );
        })
    }

    #[test]
    fn test_transfer_cash_success_state() {
        new_test_ext().execute_with(|| {
            let quantity = CashPrincipalAmount::from_nominal("1");

            let state = CashPipeline::new()
                .transfer_cash::<Test>(account_a, account_b, quantity)
                .expect("transfer_cash failed")
                .state;

            assert_eq!(
                state,
                State {
                    total_supply_asset: vec![].into_iter().collect(),
                    total_borrow_asset: vec![].into_iter().collect(),
                    asset_balances: vec![].into_iter().collect(),
                    assets_with_non_zero_balance: vec![].into_iter().collect(),
                    last_indices: vec![].into_iter().collect(),
                    cash_principals: vec![
                        (account_a, CashPrincipal::from_nominal("-1")),
                        (account_b, CashPrincipal::from_nominal("1")),
                    ]
                    .into_iter()
                    .collect(),
                    total_cash_principal: Some(quantity),
                    chain_cash_principals: vec![].into_iter().collect(),
                }
            );
        })
    }

    #[test]
    fn test_lock_cash_success_state() {
        new_test_ext().execute_with(|| {
            ChainCashPrincipals::insert(ChainId::Eth, CashPrincipalAmount::from_nominal("3"));
            let quantity = CashPrincipalAmount::from_nominal("1");

            let state = CashPipeline::new()
                .lock_cash::<Test>(account_a, quantity)
                .expect("lock_cash failed")
                .state;

            assert_eq!(
                state,
                State {
                    total_supply_asset: vec![].into_iter().collect(),
                    total_borrow_asset: vec![].into_iter().collect(),
                    asset_balances: vec![].into_iter().collect(),
                    assets_with_non_zero_balance: vec![].into_iter().collect(),
                    last_indices: vec![].into_iter().collect(),
                    cash_principals: vec![(account_a, CashPrincipal::from_nominal("1")),]
                        .into_iter()
                        .collect(),
                    total_cash_principal: Some(CashPrincipalAmount::from_nominal("0")),
                    chain_cash_principals: vec![(
                        ChainId::Eth,
                        CashPrincipalAmount::from_nominal("2")
                    )]
                    .into_iter()
                    .collect(),
                }
            );
        })
    }

    #[test]
    fn test_extract_cash_success_state() {
        new_test_ext().execute_with(|| {
            ChainCashPrincipals::insert(ChainId::Eth, CashPrincipalAmount::from_nominal("3"));
            let quantity = CashPrincipalAmount::from_nominal("1");

            let state = CashPipeline::new()
                .extract_cash::<Test>(account_a, quantity)
                .expect("extract_cash failed")
                .state;

            assert_eq!(
                state,
                State {
                    total_supply_asset: vec![].into_iter().collect(),
                    total_borrow_asset: vec![].into_iter().collect(),
                    asset_balances: vec![].into_iter().collect(),
                    assets_with_non_zero_balance: vec![].into_iter().collect(),
                    last_indices: vec![].into_iter().collect(),
                    cash_principals: vec![(account_a, CashPrincipal::from_nominal("-1")),]
                        .into_iter()
                        .collect(),
                    total_cash_principal: Some(CashPrincipalAmount::from_nominal("1")),
                    chain_cash_principals: vec![(
                        ChainId::Eth,
                        CashPrincipalAmount::from_nominal("4")
                    )]
                    .into_iter()
                    .collect(),
                }
            );
        })
    }

    #[test]
    fn test_build_portfolio() {
        new_test_ext().execute_with(|| {
            assert_ok!(init_eth_asset());
            assert_ok!(init_wbtc_asset());

            CashPrincipals::insert(account_a, CashPrincipal(10000000)); // 10 CASH
            AssetsWithNonZeroBalance::insert(account_a, Wbtc, ());

            let eth_quantity = eth.as_quantity_nominal("1");
            let eth_amount = eth_quantity.value as i128;

            let wbtc_quantity = wbtc.as_quantity_nominal("0.02");
            let wbtc_amount = wbtc_quantity.value as i128;

            let state = CashPipeline::new()
                .transfer_asset::<Test>(account_a, account_b, Eth, eth_quantity)
                .expect("transfer_asset(eth) failed")
                .transfer_asset::<Test>(account_b, account_a, Wbtc, wbtc_quantity)
                .expect("transfer_asset(wbtc) failed")
                .state;

            let portfolio_a = state.clone().build_portfolio::<Test>(account_a);

            assert_eq!(
                portfolio_a,
                Ok(Portfolio {
                    cash: Balance {
                        value: 10000000,
                        units: CASH
                    },
                    positions: vec![
                        (
                            wbtc,
                            Balance {
                                value: wbtc_amount,
                                units: WBTC
                            }
                        ),
                        (
                            eth,
                            Balance {
                                value: -eth_amount,
                                units: ETH
                            }
                        ),
                    ]
                })
            );

            let portfolio_b = state.build_portfolio::<Test>(account_b);

            assert_eq!(
                portfolio_b,
                Ok(Portfolio {
                    cash: Balance {
                        value: 0,
                        units: CASH
                    },
                    positions: vec![
                        (
                            wbtc,
                            Balance {
                                value: -wbtc_amount,
                                units: WBTC
                            }
                        ),
                        (
                            eth,
                            Balance {
                                value: eth_amount,
                                units: ETH
                            }
                        ),
                    ]
                })
            );
        })
    }

    #[test]
    fn test_check_collateralized() {
        new_test_ext().execute_with(|| {
            assert_ok!(init_eth_asset());
            assert_ok!(init_wbtc_asset());

            CashPrincipals::insert(account_a, CashPrincipal(100000000000)); // 100,000 CASH
            AssetsWithNonZeroBalance::insert(account_a, Wbtc, ());

            let eth_quantity = eth.as_quantity_nominal("1");
            let wbtc_quantity = wbtc.as_quantity_nominal("0.02");

            let res = CashPipeline::new()
                .transfer_asset::<Test>(account_a, account_b, Eth, eth_quantity)
                .expect("transfer_asset(eth) failed")
                .transfer_asset::<Test>(account_b, account_a, Wbtc, wbtc_quantity)
                .expect("transfer_asset(wbtc) failed")
                .check_collateralized::<Test>(account_a)
                .expect("account_a should be liquid")
                .check_collateralized::<Test>(account_b);

            assert_eq!(res, Err(Reason::InsufficientLiquidity));
        })
    }

    #[test]
    fn test_check_underwater() {
        new_test_ext().execute_with(|| {
            assert_ok!(init_eth_asset());
            assert_ok!(init_wbtc_asset());

            CashPrincipals::insert(account_a, CashPrincipal(100000000000)); // 100,000 CASH
            AssetsWithNonZeroBalance::insert(account_a, Wbtc, ());

            let eth_quantity = eth.as_quantity_nominal("1");
            let wbtc_quantity = wbtc.as_quantity_nominal("0.02");

            let res = CashPipeline::new()
                .transfer_asset::<Test>(account_a, account_b, Eth, eth_quantity)
                .expect("transfer_asset(eth) failed")
                .transfer_asset::<Test>(account_b, account_a, Wbtc, wbtc_quantity)
                .expect("transfer_asset(wbtc) failed")
                .check_underwater::<Test>(account_b)
                .expect("account_b should be underwater")
                .check_underwater::<Test>(account_a);

            assert_eq!(res, Err(Reason::SufficientLiquidity));
        })
    }

    #[test]
    fn test_check_asset_balance() {
        new_test_ext().execute_with(|| {
            assert_ok!(init_eth_asset());
            assert_ok!(init_wbtc_asset());

            let eth_quantity = eth.as_quantity_nominal("1");
            let wbtc_quantity = wbtc.as_quantity_nominal("0.02");

            let res = CashPipeline::new()
                .transfer_asset::<Test>(account_a, account_b, Eth, eth_quantity)
                .expect("transfer_asset(eth) failed")
                .transfer_asset::<Test>(account_b, account_a, Wbtc, wbtc_quantity)
                .expect("transfer_asset(wbtc) failed")
                .check_asset_balance::<Test, _>(account_a, eth, |balance| {
                    if balance == eth_quantity.negate().unwrap() {
                        Ok(())
                    } else {
                        Err(Reason::None)
                    }
                })
                .expect("account_a balance should be -eth_quantity")
                .check_asset_balance::<Test, _>(account_b, eth, |balance| {
                    if balance == eth_quantity.try_into().unwrap() {
                        Ok(())
                    } else {
                        Err(Reason::None)
                    }
                })
                .expect("account_b balance should be eth_quantity")
                .check_asset_balance::<Test, _>(account_a, wbtc, |balance| {
                    if balance == wbtc_quantity.try_into().unwrap() {
                        Ok(())
                    } else {
                        Err(Reason::None)
                    }
                })
                .expect("account_a balance should be wbtc_quantity")
                .check_asset_balance::<Test, _>(account_b, wbtc, |balance| {
                    if balance == wbtc_quantity.negate().unwrap() {
                        Ok(())
                    } else {
                        Err(Reason::None)
                    }
                })
                .expect("account_b balance should be -wbtc_quantity")
                .check_asset_balance::<Test, _>(account_a, wbtc, |_| Err(Reason::None));

            assert_eq!(res, Err(Reason::None));
        })
    }

    #[test]
    fn test_check_cash_principal() {
        new_test_ext().execute_with(|| {
            let quantity = CashPrincipalAmount::from_nominal("1");

            let res = CashPipeline::new()
                .transfer_cash::<Test>(account_a, account_b, quantity)
                .expect("transfer_cash failed")
                .check_cash_principal::<Test, _>(account_a, |principal| {
                    if principal.eq(-1000000) {
                        Ok(())
                    } else {
                        Err(Reason::None)
                    }
                })
                .expect("account_a principal should be -1 CASH")
                .check_cash_principal::<Test, _>(account_b, |principal| {
                    if principal.eq(1000000) {
                        Ok(())
                    } else {
                        Err(Reason::None)
                    }
                })
                .expect("account_b principal should be 1 CASH")
                .check_cash_principal::<Test, _>(account_a, |_| Err(Reason::None));

            assert_eq!(res, Err(Reason::None));
        })
    }

    #[test]
    fn test_transfer_two_assets_success_state() {
        new_test_ext().execute_with(|| {
            assert_ok!(init_eth_asset());
            assert_ok!(init_wbtc_asset());

            let eth_quantity = eth.as_quantity_nominal("1");
            let eth_amount = eth_quantity.value as i128;

            let wbtc_quantity = wbtc.as_quantity_nominal("0.1");
            let wbtc_amount = wbtc_quantity.value as i128;

            let state = CashPipeline::new()
                .transfer_asset::<Test>(account_a, account_b, Eth, eth_quantity)
                .expect("transfer_asset(eth) failed")
                .transfer_asset::<Test>(account_b, account_a, Wbtc, wbtc_quantity)
                .expect("transfer_asset(wbtc) failed")
                .state;

            assert_eq!(
                state,
                State {
                    total_supply_asset: vec![
                        (Eth, eth_quantity.value),
                        (Wbtc, wbtc_quantity.value)
                    ]
                    .into_iter()
                    .collect(),
                    total_borrow_asset: vec![
                        (Eth, eth_quantity.value),
                        (Wbtc, wbtc_quantity.value)
                    ]
                    .into_iter()
                    .collect(),
                    asset_balances: vec![
                        ((Eth, account_a), -eth_amount),
                        ((Eth, account_b), eth_amount),
                        ((Wbtc, account_a), wbtc_amount),
                        ((Wbtc, account_b), -wbtc_amount)
                    ]
                    .into_iter()
                    .collect(),
                    assets_with_non_zero_balance: vec![
                        ((Eth, account_a), true),
                        ((Eth, account_b), true),
                        ((Wbtc, account_a), true),
                        ((Wbtc, account_b), true)
                    ]
                    .into_iter()
                    .collect(),
                    last_indices: vec![
                        ((Eth, account_a), AssetIndex::from_nominal("0")),
                        ((Eth, account_b), AssetIndex::from_nominal("0")),
                        ((Wbtc, account_a), AssetIndex::from_nominal("0")),
                        ((Wbtc, account_b), AssetIndex::from_nominal("0"))
                    ]
                    .into_iter()
                    .collect(),
                    cash_principals: vec![
                        (account_a, CashPrincipal::from_nominal("0")),
                        (account_b, CashPrincipal::from_nominal("0")),
                    ]
                    .into_iter()
                    .collect(),
                    total_cash_principal: None,
                    chain_cash_principals: vec![].into_iter().collect(),
                }
            );
        })
    }

    #[test]
    fn test_transfer_asset_success_commit() {
        new_test_ext().execute_with(|| {
            assert_ok!(init_eth_asset());

            let quantity = eth.as_quantity_nominal("1");
            let amount = quantity.value as i128;

            CashPipeline::new()
                .transfer_asset::<Test>(account_a, account_b, Eth, quantity)
                .expect("transfer_asset failed")
                .commit::<Test>();

            assert_eq!(TotalSupplyAssets::get(Eth), quantity.value);
            assert_eq!(TotalBorrowAssets::get(Eth), quantity.value);
            assert_eq!(AssetBalances::get(Eth, account_a), -amount);
            assert_eq!(AssetBalances::get(Eth, account_b), amount);
            assert_eq!(
                AssetsWithNonZeroBalance::iter_prefix(account_a).collect::<Vec<_>>(),
                vec![(Eth, ())]
            );
            assert_eq!(
                AssetsWithNonZeroBalance::iter_prefix(account_b).collect::<Vec<_>>(),
                vec![(Eth, ())]
            );
            assert_eq!(
                LastIndices::get(Eth, account_a),
                AssetIndex::from_nominal("0")
            );
            assert_eq!(
                LastIndices::get(Eth, account_b),
                AssetIndex::from_nominal("0")
            );
            assert_eq!(
                LastIndices::get(Wbtc, account_a),
                AssetIndex::from_nominal("0")
            );
            assert_eq!(
                LastIndices::get(Wbtc, account_b),
                AssetIndex::from_nominal("0")
            );
            assert_eq!(
                CashPrincipals::get(account_a),
                CashPrincipal::from_nominal("0")
            );
            assert_eq!(
                CashPrincipals::get(account_b),
                CashPrincipal::from_nominal("0")
            );
            assert_eq!(
                ChainCashPrincipals::get(ChainId::Eth),
                CashPrincipalAmount::from_nominal("0")
            );
        })
    }

    #[test]
    fn test_transfer_two_assets_success_commit() {
        new_test_ext().execute_with(|| {
            assert_ok!(init_eth_asset());
            assert_ok!(init_wbtc_asset());

            let eth_quantity = eth.as_quantity_nominal("1");
            let eth_amount = eth_quantity.value as i128;

            let wbtc_quantity = wbtc.as_quantity_nominal("0.1");
            let wbtc_amount = wbtc_quantity.value as i128;

            CashPipeline::new()
                .transfer_asset::<Test>(account_a, account_b, Eth, eth_quantity)
                .expect("transfer_asset(eth) failed")
                .transfer_asset::<Test>(account_b, account_a, Wbtc, wbtc_quantity)
                .expect("transfer_asset(wbtc) failed")
                .commit::<Test>();

            assert_eq!(TotalSupplyAssets::get(Eth), eth_quantity.value);
            assert_eq!(TotalBorrowAssets::get(Eth), eth_quantity.value);
            assert_eq!(TotalSupplyAssets::get(Wbtc), wbtc_quantity.value);
            assert_eq!(TotalBorrowAssets::get(Wbtc), wbtc_quantity.value);
            assert_eq!(AssetBalances::get(Eth, account_a), -eth_amount);
            assert_eq!(AssetBalances::get(Eth, account_b), eth_amount);
            assert_eq!(AssetBalances::get(Wbtc, account_a), wbtc_amount);
            assert_eq!(AssetBalances::get(Wbtc, account_b), -wbtc_amount);
            assert_eq!(
                AssetsWithNonZeroBalance::iter_prefix(account_a).collect::<Vec<_>>(),
                vec![(Eth, ()), (Wbtc, ())]
            );
            assert_eq!(
                AssetsWithNonZeroBalance::iter_prefix(account_b).collect::<Vec<_>>(),
                vec![(Eth, ()), (Wbtc, ())]
            );
            assert_eq!(
                LastIndices::get(Eth, account_a),
                AssetIndex::from_nominal("0")
            );
            assert_eq!(
                LastIndices::get(Eth, account_b),
                AssetIndex::from_nominal("0")
            );
            assert_eq!(
                LastIndices::get(Wbtc, account_a),
                AssetIndex::from_nominal("0")
            );
            assert_eq!(
                LastIndices::get(Wbtc, account_b),
                AssetIndex::from_nominal("0")
            );
            assert_eq!(
                CashPrincipals::get(account_a),
                CashPrincipal::from_nominal("0")
            );
            assert_eq!(
                CashPrincipals::get(account_b),
                CashPrincipal::from_nominal("0")
            );
            assert_eq!(
                ChainCashPrincipals::get(ChainId::Eth),
                CashPrincipalAmount::from_nominal("0")
            );
        })
    }

    #[test]
    fn test_commit() {
        new_test_ext().execute_with(|| {
            AssetsWithNonZeroBalance::insert(account_a, Eth, ());
            AssetsWithNonZeroBalance::insert(account_b, Eth, ());

            let state = State {
                total_supply_asset: vec![(Eth, 1000), (Wbtc, 2000)].into_iter().collect(),
                total_borrow_asset: vec![(Eth, 3000), (Wbtc, 4000)].into_iter().collect(),
                asset_balances: vec![
                    ((Eth, account_a), 5000),
                    ((Eth, account_b), -6000),
                    ((Wbtc, account_a), -7000),
                    ((Wbtc, account_b), 8000),
                ]
                .into_iter()
                .collect(),
                assets_with_non_zero_balance: vec![
                    ((Eth, account_a), true),
                    ((Eth, account_b), false),
                    ((Wbtc, account_a), false),
                    ((Wbtc, account_b), true),
                ]
                .into_iter()
                .collect(),
                last_indices: vec![
                    ((Eth, account_a), AssetIndex::from_nominal("9000")),
                    ((Eth, account_b), AssetIndex::from_nominal("10000")),
                    ((Wbtc, account_a), AssetIndex::from_nominal("11000")),
                    ((Wbtc, account_b), AssetIndex::from_nominal("12000")),
                ]
                .into_iter()
                .collect(),
                cash_principals: vec![
                    (account_a, CashPrincipal::from_nominal("13000")),
                    (account_b, CashPrincipal::from_nominal("14000")),
                ]
                .into_iter()
                .collect(),
                total_cash_principal: Some(CashPrincipalAmount::from_nominal("15000")),
                chain_cash_principals: vec![
                    (ChainId::Eth, CashPrincipalAmount::from_nominal("16000")),
                    (ChainId::Dot, CashPrincipalAmount::from_nominal("17000")),
                ]
                .into_iter()
                .collect(),
            };

            state.commit::<Test>();

            assert_eq!(TotalSupplyAssets::get(Eth), 1000);
            assert_eq!(TotalSupplyAssets::get(Wbtc), 2000);
            assert_eq!(TotalBorrowAssets::get(Eth), 3000);
            assert_eq!(TotalBorrowAssets::get(Wbtc), 4000);
            assert_eq!(AssetBalances::get(Eth, account_a), 5000);
            assert_eq!(AssetBalances::get(Eth, account_b), -6000);
            assert_eq!(AssetBalances::get(Wbtc, account_a), -7000);
            assert_eq!(AssetBalances::get(Wbtc, account_b), 8000);
            assert_eq!(
                AssetsWithNonZeroBalance::iter_prefix(account_a).collect::<Vec<_>>(),
                vec![(Eth, ())]
            );
            assert_eq!(
                AssetsWithNonZeroBalance::iter_prefix(account_b).collect::<Vec<_>>(),
                vec![(Wbtc, ())]
            );
            assert_eq!(
                LastIndices::get(Eth, account_a),
                AssetIndex::from_nominal("9000")
            );
            assert_eq!(
                LastIndices::get(Eth, account_b),
                AssetIndex::from_nominal("10000")
            );
            assert_eq!(
                LastIndices::get(Wbtc, account_a),
                AssetIndex::from_nominal("11000")
            );
            assert_eq!(
                LastIndices::get(Wbtc, account_b),
                AssetIndex::from_nominal("12000")
            );
            assert_eq!(
                CashPrincipals::get(account_a),
                CashPrincipal::from_nominal("13000")
            );
            assert_eq!(
                CashPrincipals::get(account_b),
                CashPrincipal::from_nominal("14000")
            );
            assert_eq!(
                TotalCashPrincipal::get(),
                CashPrincipalAmount::from_nominal("15000")
            );
            assert_eq!(
                ChainCashPrincipals::get(ChainId::Eth),
                CashPrincipalAmount::from_nominal("16000")
            );
            assert_eq!(
                ChainCashPrincipals::get(ChainId::Dot),
                CashPrincipalAmount::from_nominal("17000")
            );
        })
    }

    // #[test]
    // fn test_liquidate_internal_asset_repay_and_supply_amount_overflow() {
    //     new_test_ext().execute_with(|| {
    //         let amount: AssetQuantity = eth.as_quantity_nominal("1");

    //         init_eth_asset().unwrap();
    //         init_wbtc_asset().unwrap();

    //         init_asset_balance(Eth, borrower, Balance::from_nominal("3", ETH).value); // 3 * 2000 * 0.8 = 4800
    //         init_asset_balance(Wbtc, borrower, Balance::from_nominal("-1", WBTC).value); // 1 * 60000 / 0.6 = -100000
    //         init_cash(borrower, CashPrincipal::from_nominal("95000")); // 95000 + 4800 - 1000000 = -200

    //         init_cash(liquidator, CashPrincipal::from_nominal("1000000"));
    //         init_asset_balance(Eth, liquidator, i128::MIN);

    //         assert_eq!(
    //             liquidate_internal::<Test>(asset, collateral_asset, liquidator, borrower, amount),
    //             Err(Reason::MathError(MathError::Overflow))
    //         );
    //     })
    // }

    // #[test]
    // fn test_liquidate_internal_collateral_asset_repay_and_supply_amount_overflow() {
    //     new_test_ext().execute_with(|| {
    //         let amount: AssetQuantity = eth.as_quantity_nominal("1");

    //         init_eth_asset().unwrap();
    //         init_wbtc_asset().unwrap();

    //         init_asset_balance(Eth, borrower, Balance::from_nominal("-3", ETH).value);

    //         init_cash(liquidator, CashPrincipal::from_nominal("1000000"));
    //         init_asset_balance(Wbtc, liquidator, i128::MIN);

    //         assert_eq!(
    //             liquidate_internal::<Test>(asset, collateral_asset, liquidator, borrower, amount),
    //             Err(Reason::MathError(MathError::Overflow))
    //         );
    //     })
    // }
}
