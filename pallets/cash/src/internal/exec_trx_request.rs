use crate::{
    chains::{ChainAccount, ChainAccountSignature},
    internal::{
        assets::get_asset,
        extract::{extract_cash_principal_internal, extract_internal},
        liquidate::{
            liquidate_cash_collateral_internal, liquidate_cash_principal_internal,
            liquidate_internal,
        },
        transfer::{transfer_cash_principal_internal, transfer_internal},
    },
    log,
    params::TRANSFER_FEE,
    reason::Reason,
    require,
    symbol::CASH,
    types::{CashIndex, CashOrChainAsset, CashPrincipalAmount, Nonce, Quantity},
    CashPrincipals, Config, GlobalCashIndex, Nonces,
};
use frame_support::storage::{StorageMap, StorageValue};
use our_std::{convert::TryInto, str};

//  预先拼接nonce
pub fn prepend_nonce(payload: &Vec<u8>, nonce: Nonce) -> Vec<u8> {
    let mut result: Vec<u8> = Vec::new();
    result.extend_from_slice(nonce.to_string().as_bytes());
    result.extend_from_slice(b":");
    result.extend_from_slice(&payload[..]);
    result
}

pub fn exec<T: Config>(
    request: Vec<u8>,
    signature: ChainAccountSignature,
    nonce: Nonce,
) -> Result<(), Reason> {
    log!("exec: {}", nonce);
    let request_str: &str = str::from_utf8(&request[..]).map_err(|_| Reason::InvalidUTF8)?;
    //  从字符串恢复发送者
    let sender = signature.recover_account(&prepend_nonce(&request, nonce)[..])?;
    //  执行请求
    exec_trx_request::<T>(request_str, sender, Some(nonce))
}

//  最低验证程度的trx请求
pub fn is_minimally_valid_trx_request<T: Config>(
    request: Vec<u8>,
    signature: ChainAccountSignature,
    nonce: Nonce,
) -> Result<(ChainAccount, Nonce), Reason> {
    if request.len() > crate::params::MAX_TRX_REQUEST_LEN {
        return Err(Reason::TrxRequestTooLong);
    }

    //  针对请求字符串的校验
    // Basic request validity checks - valid symbols and parsable request
    let request_str: &str = str::from_utf8(&request[..]).map_err(|_| Reason::InvalidUTF8)?;
    trx_request::parse_request(request_str)?;

    //  签名验证，恢复发送者
    // Signature check
    let sender = signature
        .recover_account(&prepend_nonce(&request, nonce)[..])
        .map_err(|_| Reason::SignatureAccountMismatch)?;

    let current_nonce = Nonces::get(sender);
    Ok((sender, current_nonce))
}

//  执行trx请求
pub fn exec_trx_request<T: Config>(
    request_str: &str,
    sender: ChainAccount,
    nonce_opt: Option<Nonce>,
) -> Result<(), Reason> {
    log!("exec_trx_request: {}", request_str);
    //  恢复trx请求
    // Match TrxReq against known Transaction Requests
    let trx_request = trx_request::parse_request(request_str)?;
    //  检查nance
    if let Some(nonce) = nonce_opt {
        // Read Require Nonce=Nonce_Account+1
        let current_nonce = Nonces::get(sender);
        require!(
            nonce == current_nonce,
            Reason::IncorrectNonce(nonce, current_nonce)
        );
    }

    match trx_request {
        //  匹配请求类型，如果是提款
        trx_request::TrxRequest::Extract(max_amount, asset, account) => {
            match CashOrChainAsset::from(asset) {
                //  如果是cash
                CashOrChainAsset::Cash => match max_amount {
                    //  如果是提取最大资产
                    trx_request::MaxAmount::Max => {
                        let principal_amount = CashPrincipals::get(sender).amount_withdrawable()?;
                        extract_cash_principal_internal::<T>(
                            sender,
                            account.into(),
                            principal_amount,
                        )?;
                    }
                    //  如果是某个特定数额
                    trx_request::MaxAmount::Amount(amount) => {
                        let index = GlobalCashIndex::get();
                        let principal_amount =
                            index.cash_principal_amount(Quantity::new(amount, CASH))?;
                        extract_cash_principal_internal::<T>(
                            sender,
                            account.into(),
                            principal_amount,
                        )?;
                    }
                }, 
                //  如果是链上资产
                CashOrChainAsset::ChainAsset(chain_asset) => match max_amount {
                    trx_request::MaxAmount::Max => {
                        return Err(Reason::MaxForNonCashAsset);
                    }
                    trx_request::MaxAmount::Amount(amount) => {
                        let asset = get_asset::<T>(chain_asset)?;
                        let asset_amount = asset.as_quantity(amount.into());
                        extract_internal::<T>(asset, sender, account.into(), asset_amount)?;
                    }
                },
            }
        }
        //  如果是转账
        trx_request::TrxRequest::Transfer(max_amount, asset, account) => {
            match CashOrChainAsset::from(asset) {
                //  如果是cash类型
                CashOrChainAsset::Cash => match max_amount {
                    trx_request::MaxAmount::Max => {
                        //  得到cash利率指数
                        let index: CashIndex = GlobalCashIndex::get();
                        let user_principal = CashPrincipals::get(sender);
                        //  转账费计算
                        let fee_principal = index.cash_principal_amount(TRANSFER_FEE)?;
                        //  转移的总资产
                        let transfer_principal: CashPrincipalAmount = user_principal
                            .sub_amount(fee_principal)?
                            .try_into()
                            .map_err(|_| Reason::InsufficientCashForMaxTransfer)?;
                        transfer_cash_principal_internal::<T>(
                            sender,
                            account.into(),
                            transfer_principal,
                        )?;
                    }
                    //  转移适量金额
                    trx_request::MaxAmount::Amount(amount) => {
                        let index = GlobalCashIndex::get();
                        let principal_amount =
                            index.cash_principal_amount(Quantity::new(amount, CASH))?;
                        transfer_cash_principal_internal::<T>(
                            sender,
                            account.into(),
                            principal_amount,
                        )?;
                    }
                },
                //  如果是资产类型
                CashOrChainAsset::ChainAsset(chain_asset) => match max_amount {
                    trx_request::MaxAmount::Max => {
                        return Err(Reason::MaxForNonCashAsset);
                    }

                    trx_request::MaxAmount::Amount(amount) => {
                        let asset = get_asset::<T>(chain_asset)?;
                        let asset_amount = asset.as_quantity(amount.into());
                        transfer_internal::<T>(asset, sender, account.into(), asset_amount)?;
                    }
                },
            }
        }
        //  如果是清算
        trx_request::TrxRequest::Liquidate(
            max_amount,
            trx_borrowed_asset,
            trx_collateral_asset,
            borrower,
        ) => match (
            //  分别匹配借款和抵押品
            CashOrChainAsset::from(trx_borrowed_asset),
            CashOrChainAsset::from(trx_collateral_asset),
        ) {
            //  如果借的资产和抵押的资产一致，那么报错
            (x, y) if x == y => return Err(Reason::InKindLiquidation),
            //  计算本金数额
            //  如果是cash和资产的组合
            (CashOrChainAsset::Cash, CashOrChainAsset::ChainAsset(collateral)) => {
                let collateral_asset = get_asset::<T>(collateral)?;
                //  计算本金金额
                let cash_principal_amount = match max_amount {
                    trx_request::MaxAmount::Max => panic!("Not supported"), // TODO
                    trx_request::MaxAmount::Amount(amount) => {
                        let index = GlobalCashIndex::get();
                        index.cash_principal_amount(Quantity::new(amount, CASH))?
                    }
                };
                //  清算资产
                liquidate_cash_principal_internal::<T>(
                    collateral_asset,
                    sender,
                    borrower.into(),
                    cash_principal_amount,
                )?;
            }
            //  如果是资产和cash
            (CashOrChainAsset::ChainAsset(borrowed), CashOrChainAsset::Cash) => {
                let borrowed_asset = get_asset::<T>(borrowed)?;
                //  接触的资产总额
                let borrowed_asset_amount = match max_amount {
                    trx_request::MaxAmount::Max => panic!("Not supported"), // TODO
                    trx_request::MaxAmount::Amount(amount) => {
                        borrowed_asset.as_quantity(amount.into())
                    }
                };

                liquidate_cash_collateral_internal::<T>(
                    borrowed_asset,
                    sender,
                    borrower.into(),
                    borrowed_asset_amount,
                )?;
            }
            //    如果是双资产
            (CashOrChainAsset::ChainAsset(borrowed), CashOrChainAsset::ChainAsset(collateral)) => {
                let borrowed_asset = get_asset::<T>(borrowed)?;
                let collateral_asset = get_asset::<T>(collateral)?;
                let borrowed_asset_amount = match max_amount {
                    trx_request::MaxAmount::Max => panic!("Not supported"), // TODO
                    trx_request::MaxAmount::Amount(amount) => {
                        borrowed_asset.as_quantity(amount.into())
                    }
                };
                //  清算资产
                liquidate_internal::<T>(
                    borrowed_asset,
                    collateral_asset,
                    sender,
                    borrower.into(),
                    borrowed_asset_amount,
                )?;
            }

            _ => return Err(Reason::InvalidLiquidation), // Probably isn't possible
        },
    }
    //  更新nonce
    if let Some(nonce) = nonce_opt {
        // Update user nonce
        Nonces::insert(sender, nonce + 1);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        chains::*,
        reason::TrxReqParseError,
        tests::mock::*,
        tests::{common::*, *},
        types::*,
        *,
    };

    fn get_empty_signature() -> ChainAccountSignature {
        ChainAccountSignature::Eth([0u8; 20], [0u8; 65])
    }

    #[test]
    fn test_is_minimally_valid_trx_request_fails_when_too_long() {
        let request = [0; crate::params::MAX_TRX_REQUEST_LEN + 1].into();
        let result = is_minimally_valid_trx_request::<Test>(request, get_empty_signature(), 0);
        assert_eq!(result, Err(Reason::TrxRequestTooLong));
    }

    #[test]
    fn exec_trx_request_extract_cash_principal_internal() {
        new_test_ext().execute_with(|| {
            let req_str = "(Extract 3000000 CASH Eth:0x0101010101010101010101010101010101010101)";
            let account = ChainAccount::Eth([20; 20]);
            init_cash(account, CashPrincipal::from_nominal("4"));
            let nonce = Some(0);

            let res = exec_trx_request::<Test>(req_str, account, nonce);
            assert_eq!(res, Ok(()));

            assert_eq!(
                CashPrincipals::get(account),
                CashPrincipal::from_nominal("1")
            );
            assert_eq!(Nonces::get(account), 1);

            let expected_notice_id = NoticeId(0, 1);
            let expected_notice = Notice::CashExtractionNotice(CashExtractionNotice::Eth {
                id: expected_notice_id,
                parent: [0u8; 32],
                account: [1; 20],
                principal: 3000000,
            });

            // Check Notice
            let (actual_notice_id, actual_notice) =
                Notices::iter_prefix(ChainId::Eth).next().unwrap();
            assert_eq!(actual_notice, expected_notice);
            assert_eq!(actual_notice_id, expected_notice_id);
            let (_, state_actual_notice_id, notice_state) = NoticeStates::iter().next().unwrap();
            assert_eq!(state_actual_notice_id, expected_notice_id);
            assert_eq!(
                notice_state,
                NoticeState::Pending {
                    signature_pairs: ChainSignatureList::Eth([].to_vec())
                }
            );
            let (hash_notice_id, _) = LatestNotice::get(ChainId::Eth).unwrap();
            assert_eq!(hash_notice_id, expected_notice_id);
            let (actual_chain_account, actual_notices) = AccountNotices::iter().next().unwrap();
            assert_eq!(actual_chain_account, ChainAccount::Eth([1; 20]));
            assert_eq!(actual_notices, [expected_notice_id]);

            // Check emitted `Notice` event
            let mut events_iter = System::events().into_iter();
            let notice_event = events_iter.next().unwrap();
            let expected_notice_encoded = expected_notice.encode_notice();
            assert_eq!(
                mock::Event::pallet_cash(crate::Event::Notice(
                    expected_notice_id,
                    expected_notice.clone(),
                    expected_notice_encoded
                )),
                notice_event.event
            );

            // Check emitted `ExtractCash` event
            let extract_cash_event = events_iter.next().unwrap();
            let index = GlobalCashIndex::get();
            assert_eq!(
                mock::Event::pallet_cash(crate::Event::ExtractCash(
                    account,
                    ChainAccount::Eth([1; 20]),
                    index
                        .cash_principal_amount(Quantity::from_nominal("3", CASH))
                        .unwrap(),
                    index
                )),
                extract_cash_event.event
            );
        });
    }

    #[test]
    fn exec_trx_request_extract_cash_principal_max_internal() {
        new_test_ext().execute_with(|| {
            let req_str = "(Extract Max CASH Eth:0x0101010101010101010101010101010101010101)";
            let account = ChainAccount::Eth([20; 20]);
            init_cash(account, CashPrincipal::from_nominal("4"));
            let nonce = Some(0);

            let res = exec_trx_request::<Test>(req_str, account, nonce);
            assert_eq!(res, Ok(()));

            assert_eq!(
                CashPrincipals::get(account),
                CashPrincipal::from_nominal("0")
            );
            assert_eq!(Nonces::get(account), 1);

            let expected_notice_id = NoticeId(0, 1);
            let expected_notice = Notice::CashExtractionNotice(CashExtractionNotice::Eth {
                id: expected_notice_id,
                parent: [0u8; 32],
                account: [1; 20],
                principal: 4000000,
            });

            // Check Notice
            let (actual_notice_id, actual_notice) =
                Notices::iter_prefix(ChainId::Eth).next().unwrap();
            assert_eq!(actual_notice, expected_notice);
            assert_eq!(actual_notice_id, expected_notice_id);
            let (_, state_actual_notice_id, notice_state) = NoticeStates::iter().next().unwrap();
            assert_eq!(state_actual_notice_id, expected_notice_id);
            assert_eq!(
                notice_state,
                NoticeState::Pending {
                    signature_pairs: ChainSignatureList::Eth([].to_vec())
                }
            );
            let (hash_notice_id, _) = LatestNotice::get(ChainId::Eth).unwrap();
            assert_eq!(hash_notice_id, expected_notice_id);
            let (actual_chain_account, actual_notices) = AccountNotices::iter().next().unwrap();
            assert_eq!(actual_chain_account, ChainAccount::Eth([1; 20]));
            assert_eq!(actual_notices, [expected_notice_id]);

            // Check emitted `Notice` event
            let mut events_iter = System::events().into_iter();
            let notice_event = events_iter.next().unwrap();
            let expected_notice_encoded = expected_notice.encode_notice();
            assert_eq!(
                mock::Event::pallet_cash(crate::Event::Notice(
                    expected_notice_id,
                    expected_notice.clone(),
                    expected_notice_encoded
                )),
                notice_event.event
            );

            // Check emitted `ExtractCash` event
            let extract_cash_event = events_iter.next().unwrap();
            let index = GlobalCashIndex::get();
            assert_eq!(
                mock::Event::pallet_cash(crate::Event::ExtractCash(
                    account,
                    ChainAccount::Eth([1; 20]),
                    index
                        .cash_principal_amount(Quantity::from_nominal("4", CASH))
                        .unwrap(),
                    index
                )),
                extract_cash_event.event
            );
        });
    }

    #[test]
    fn exec_trx_request_extract_internal() {
        new_test_ext().execute_with(|| {
            let asset = init_eth_asset().unwrap();
            let account = ChainAccount::Eth([20; 20]);
            init_asset_balance(asset, account, Balance::from_nominal("3", ETH).value);
            let req_str =
                "(Extract 1000000000000000000 Eth:0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee \
                Eth:0x0101010101010101010101010101010101010101)";
            let nonce = Some(0);

            let res = exec_trx_request::<Test>(req_str, account, nonce);
            assert_eq!(res, Ok(()));

            assert_eq!(
                AssetBalances::get(asset, account),
                Balance::from_nominal("2", ETH).value
            );
            assert_eq!(Nonces::get(account), 1);

            let eth_asset = [238; 20];
            let expected_notice_id = NoticeId(0, 1);
            let expected_notice = Notice::ExtractionNotice(ExtractionNotice::Eth {
                id: expected_notice_id,
                parent: [0u8; 32],
                asset: eth_asset,
                account: [1; 20],
                amount: 1000000000000000000,
            });

            // Check Notice
            let (actual_notice_id, actual_notice) =
                Notices::iter_prefix(ChainId::Eth).next().unwrap();
            assert_eq!(actual_notice, expected_notice);
            assert_eq!(actual_notice_id, expected_notice_id);
            let (_, state_actual_notice_id, notice_state) = NoticeStates::iter().next().unwrap();
            assert_eq!(state_actual_notice_id, expected_notice_id);
            assert_eq!(
                notice_state,
                NoticeState::Pending {
                    signature_pairs: ChainSignatureList::Eth([].to_vec())
                }
            );
            let (hash_notice_id, _) = LatestNotice::get(ChainId::Eth).unwrap();
            assert_eq!(hash_notice_id, expected_notice_id);
            let (actual_chain_account, actual_notices) = AccountNotices::iter().next().unwrap();
            assert_eq!(actual_chain_account, ChainAccount::Eth([1; 20]));
            assert_eq!(actual_notices, [expected_notice_id]);

            // Check emitted `Notice` event
            let mut events_iter = System::events().into_iter();
            let notice_event = events_iter.next().unwrap();
            let expected_notice_encoded = expected_notice.encode_notice();
            assert_eq!(
                mock::Event::pallet_cash(crate::Event::Notice(
                    expected_notice_id,
                    expected_notice.clone(),
                    expected_notice_encoded
                )),
                notice_event.event
            );

            // Check emitted `Extract` event
            let extract_event = events_iter.next().unwrap();
            assert_eq!(
                mock::Event::pallet_cash(crate::Event::Extract(
                    asset,
                    account,
                    ChainAccount::Eth([1; 20]),
                    1000000000000000000
                )),
                extract_event.event
            );
        });
    }

    #[test]
    fn exec_trx_transfer_internal() {
        new_test_ext().execute_with(|| {
            let asset = init_eth_asset().unwrap();
            let account = ChainAccount::Eth([20; 20]);
            init_asset_balance(asset, account, Balance::from_nominal("3", ETH).value);
            let req_str =
                "(Transfer 2000000000000000000 Eth:0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee \
              Eth:0x0101010101010101010101010101010101010101)";
            let to_account = ChainAccount::Eth([1; 20]);
            let nonce = 0;

            let miner = ChainAccount::Eth([3; 20]);
            Miner::put(miner);

            let res = exec_trx_request::<Test>(req_str, account, Some(nonce));
            assert_eq!(res, Ok(()));

            assert_eq!(
                AssetBalances::get(asset, account),
                Balance::from_nominal("1", ETH).value
            );
            assert_eq!(
                AssetBalances::get(asset, to_account),
                Balance::from_nominal("2", ETH).value
            );
            // Trx fee
            assert_eq!(
                CashPrincipals::get(account),
                CashPrincipal::from_nominal("-0.01")
            );
            assert_eq!(
                CashPrincipals::get(miner),
                CashPrincipal::from_nominal("0.01")
            );
            assert_eq!(Nonces::get(account), nonce + 1);

            // Check emitted `Transfer` event
            let mut events_iter = System::events().into_iter();
            let transfer_event = events_iter.next().unwrap();
            let transfer_fee_event = events_iter.next().unwrap();
            let miner_paid_event = events_iter.next().unwrap();
            let index = GlobalCashIndex::get();
            assert_eq!(
                mock::Event::pallet_cash(crate::Event::Transfer(
                    asset,
                    account,
                    ChainAccount::Eth([1; 20]),
                    2000000000000000000
                )),
                transfer_event.event
            );
            // Check emitted `TransferCash` event
            assert_eq!(
                mock::Event::pallet_cash(crate::Event::TransferCash(
                    account,
                    miner,
                    index.cash_principal_amount(TRANSFER_FEE).unwrap(),
                    index
                )),
                transfer_fee_event.event
            );
            // Check emitted `MinerPaid` event
            assert_eq!(
                mock::Event::pallet_cash(crate::Event::MinerPaid(
                    miner,
                    index.cash_principal_amount(TRANSFER_FEE).unwrap()
                )),
                miner_paid_event.event
            );
        });
    }

    #[test]
    fn exec_trx_transfer_principal_cash_internal() {
        new_test_ext().execute_with(|| {
            let req_str = "(Transfer 3000000 CASH Eth:0x0101010101010101010101010101010101010101)";
            let account = ChainAccount::Eth([20; 20]);
            let to_account = ChainAccount::Eth([1; 20]);
            init_cash(account, CashPrincipal::from_nominal("4"));
            let nonce = Some(0);

            let miner = ChainAccount::Eth([3; 20]);
            Miner::put(miner);

            let res = exec_trx_request::<Test>(req_str, account, nonce);
            assert_eq!(res, Ok(()));

            assert_eq!(
                CashPrincipals::get(account),
                CashPrincipal::from_nominal("0.99")
            );
            assert_eq!(
                CashPrincipals::get(to_account),
                CashPrincipal::from_nominal("3")
            );
            assert_eq!(
                CashPrincipals::get(miner),
                CashPrincipal::from_nominal("0.01")
            );
            assert_eq!(Nonces::get(account), 1);

            // Check emitted `TransferCash` event
            let mut events_iter = System::events().into_iter();
            let transfer_cash_event = events_iter.next().unwrap();
            let transfer_cash_fee_event = events_iter.next().unwrap();
            let miner_paid_event = events_iter.next().unwrap();
            let index = GlobalCashIndex::get();
            assert_eq!(
                mock::Event::pallet_cash(crate::Event::TransferCash(
                    account,
                    ChainAccount::Eth([1; 20]),
                    index
                        .cash_principal_amount(Quantity::from_nominal("3", CASH))
                        .unwrap(),
                    index
                )),
                transfer_cash_event.event
            );
            // Check emitted `TransferCash` event
            assert_eq!(
                mock::Event::pallet_cash(crate::Event::TransferCash(
                    account,
                    miner,
                    index.cash_principal_amount(TRANSFER_FEE).unwrap(),
                    index
                )),
                transfer_cash_fee_event.event
            );
            // Check emitted `MinerPaid` event
            assert_eq!(
                mock::Event::pallet_cash(crate::Event::MinerPaid(
                    miner,
                    index.cash_principal_amount(TRANSFER_FEE).unwrap()
                )),
                miner_paid_event.event
            );
        });
    }

    #[test]
    fn exec_trx_transfer_principal_cash_max_internal() {
        new_test_ext().execute_with(|| {
            let req_str = "(Transfer Max CASH Eth:0x0101010101010101010101010101010101010101)";
            let account = ChainAccount::Eth([20; 20]);
            let to_account = ChainAccount::Eth([1; 20]);
            init_cash(account, CashPrincipal::from_nominal("4"));
            let nonce = Some(0);

            let miner = ChainAccount::Eth([3; 20]);
            Miner::put(miner);

            let res = exec_trx_request::<Test>(req_str, account, nonce);
            assert_eq!(res, Ok(()));

            assert_eq!(
                CashPrincipals::get(account),
                CashPrincipal::from_nominal("0")
            );
            assert_eq!(
                CashPrincipals::get(to_account),
                CashPrincipal::from_nominal("3.99")
            );
            assert_eq!(
                CashPrincipals::get(miner),
                CashPrincipal::from_nominal("0.01")
            );
            assert_eq!(Nonces::get(account), 1);

            // Check emitted `TransferCash` event
            let mut events_iter = System::events().into_iter();
            let transfer_cash_event = events_iter.next().unwrap();
            let transfer_cash_fee_event = events_iter.next().unwrap();
            let index = GlobalCashIndex::get();
            assert_eq!(
                mock::Event::pallet_cash(crate::Event::TransferCash(
                    account,
                    ChainAccount::Eth([1; 20]),
                    index
                        .cash_principal_amount(Quantity::from_nominal("3.99", CASH))
                        .unwrap(),
                    index
                )),
                transfer_cash_event.event
            );
            // Check emitted `TransferCash` event
            assert_eq!(
                mock::Event::pallet_cash(crate::Event::TransferCash(
                    account,
                    miner,
                    index.cash_principal_amount(TRANSFER_FEE).unwrap(),
                    index
                )),
                transfer_cash_fee_event.event
            );
        });
    }

    #[test]
    fn exec_trx_transfer_principal_cash_max_internal_insufficient() {
        new_test_ext().execute_with(|| {
            let req_str = "(Transfer Max CASH Eth:0x0101010101010101010101010101010101010101)";
            let account = ChainAccount::Eth([20; 20]);
            let to_account = ChainAccount::Eth([1; 20]);
            init_cash(account, CashPrincipal::from_nominal("0.005"));
            let nonce = Some(0);

            let res = exec_trx_request::<Test>(req_str, account, nonce);
            assert_eq!(res, Err(Reason::InsufficientCashForMaxTransfer));

            assert_eq!(
                CashPrincipals::get(account),
                CashPrincipal::from_nominal("0.005")
            );
            assert_eq!(
                CashPrincipals::get(to_account),
                CashPrincipal::from_nominal("0")
            );
            assert_eq!(Nonces::get(account), 0);
        });
    }

    #[test]
    fn exec_trx_transfer_principal_cash_max_internal_zero() {
        new_test_ext().execute_with(|| {
            let req_str = "(Transfer Max CASH Eth:0x0101010101010101010101010101010101010101)";
            let account = ChainAccount::Eth([20; 20]);
            let to_account = ChainAccount::Eth([1; 20]);
            let nonce = Some(0);

            let res = exec_trx_request::<Test>(req_str, account, nonce);
            assert_eq!(res, Err(Reason::InsufficientCashForMaxTransfer));

            assert_eq!(
                CashPrincipals::get(account),
                CashPrincipal::from_nominal("0")
            );
            assert_eq!(
                CashPrincipals::get(to_account),
                CashPrincipal::from_nominal("0")
            );
            assert_eq!(Nonces::get(account), 0);
        });
    }

    #[test]
    fn exec_trx_transfer_principal_cash_max_internal_negative() {
        new_test_ext().execute_with(|| {
            let req_str = "(Transfer Max CASH Eth:0x0101010101010101010101010101010101010101)";
            let account = ChainAccount::Eth([20; 20]);
            let to_account = ChainAccount::Eth([1; 20]);
            init_cash(account, CashPrincipal::from_nominal("-100"));
            let nonce = Some(0);

            let res = exec_trx_request::<Test>(req_str, account, nonce);
            assert_eq!(res, Err(Reason::InsufficientCashForMaxTransfer));

            assert_eq!(
                CashPrincipals::get(account),
                CashPrincipal::from_nominal("-100")
            );
            assert_eq!(
                CashPrincipals::get(to_account),
                CashPrincipal::from_nominal("0")
            );
            assert_eq!(Nonces::get(account), 0);
        });
    }

    // TODO: Liquidation Unit Tests

    #[test]
    fn exec_trx_liquidate_in_kind() {
        new_test_ext().execute_with(|| {
            let req_str = "(Liquidate 55 Eth:0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee \
                Eth:0xEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEE Eth:0x0101010101010101010101010101010101010101)";
            let account = ChainAccount::Eth([1; 20]);
            let nonce = Some(0);

            assert_eq!(
                exec_trx_request::<Test>(req_str, account, nonce),
                Err(Reason::InKindLiquidation)
            );
        });
    }

    #[test]
    fn exec_trx_liquidate_cash_collateral_self_transfer() {
        new_test_ext().execute_with(|| {
            let eth_asset = init_eth_asset().unwrap();
            let borrower_account = ChainAccount::Eth([1; 20]);
            let req_str =
                "(Liquidate 555555555555555555 Eth:0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee \
                CASH Eth:0x0101010101010101010101010101010101010101)";
            let account = ChainAccount::Eth([1; 20]);
            let nonce = Some(0);
            init_asset_balance(
                eth_asset,
                borrower_account,
                Balance::from_nominal("-10", ETH).value,
            );

            assert_eq!(
                exec_trx_request::<Test>(req_str, account, nonce),
                Err(Reason::SelfTransfer)
            );
        });
    }

    #[test]
    fn exec_trx_liquidate_cash_collateral_success() {
        new_test_ext().execute_with(|| {
            let eth_asset = init_eth_asset().unwrap();
            let borrower_account = ChainAccount::Eth([1; 20]);
            let liquidator_account = ChainAccount::Eth([2; 20]);
            init_cash(borrower_account, CashPrincipal::from_nominal("2160"));
            init_asset_balance(
                eth_asset,
                borrower_account,
                Balance::from_nominal("-10", ETH).value,
            );
            init_asset_balance(
                eth_asset,
                liquidator_account,
                Balance::from_nominal("3", ETH).value,
            );

            // Liquidate 1e18 Eth
            let req_str =
                "(Liquidate 1000000000000000000 Eth:0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee \
                CASH Eth:0x0101010101010101010101010101010101010101)";
            let nonce = 0;

            assert_ok!(exec_trx_request::<Test>(
                req_str,
                liquidator_account,
                Some(nonce)
            ));

            assert_eq!(
                AssetBalances::get(eth_asset, liquidator_account),
                Balance::from_nominal("2", ETH).value
            );
            assert_eq!(
                AssetBalances::get(eth_asset, borrower_account),
                Balance::from_nominal("-9", ETH).value
            );
            assert_eq!(
                CashPrincipals::get(liquidator_account),
                CashPrincipal::from_nominal("2160")
            );
            assert_eq!(
                CashPrincipals::get(borrower_account),
                CashPrincipal::from_nominal("0")
            );
            assert_eq!(Nonces::get(liquidator_account), nonce + 1);
            assert_eq!(Nonces::get(borrower_account), 0);

            // Check emitted `LiquidateCashCollateral` event
            let liquidate_event = System::events().into_iter().next().unwrap();
            assert_eq!(
                //[asset, liquidator, borrower, amount]
                mock::Event::pallet_cash(crate::Event::LiquidateCashCollateral(
                    eth_asset,
                    liquidator_account,
                    borrower_account,
                    1000000000000000000
                )),
                liquidate_event.event
            );
        });
    }

    // TODO: Implement max
    #[test]
    #[should_panic(expected = "Not supported")]
    fn exec_trx_liquidate_cash_collateral_max() {
        new_test_ext().execute_with(|| {
            let eth_asset = init_eth_asset().unwrap();
            let _borrower_account = ChainAccount::Eth([1; 20]);
            let liquidator_account = ChainAccount::Eth([2; 20]);
            init_asset_balance(
                eth_asset,
                liquidator_account,
                Balance::from_nominal("3", ETH).value,
            );

            // Liquidate 1e18 Eth
            let req_str = "(Liquidate Max Eth:0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee \
                CASH Eth:0x0101010101010101010101010101010101010101)";
            let nonce = 0;

            let _ = exec_trx_request::<Test>(req_str, liquidator_account, Some(nonce));

            // TODO: Check balances
        });
    }

    #[test]
    fn exec_trx_liquidate_cash_borrowed_success() {
        new_test_ext().execute_with(|| {
            let eth_asset = init_eth_asset().unwrap();
            let borrower_account = ChainAccount::Eth([1; 20]);
            let liquidator_account = ChainAccount::Eth([2; 20]);
            init_asset_balance(
                eth_asset,
                borrower_account,
                Balance::from_nominal("0.54", ETH).value,
            );
            init_cash(borrower_account, CashPrincipal::from_nominal("-10000"));
            init_cash(liquidator_account, CashPrincipal::from_nominal("4000"));

            // Liquidate 1000 Cash
            let req_str =
                "(Liquidate 1000000000 CASH Eth:0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee \
                Eth:0x0101010101010101010101010101010101010101)";
            let nonce = 0;

            assert_ok!(exec_trx_request::<Test>(
                req_str,
                liquidator_account,
                Some(nonce)
            ));

            assert_eq!(
                AssetBalances::get(eth_asset, liquidator_account),
                Balance::from_nominal("0.54", ETH).value
            );
            assert_eq!(
                AssetBalances::get(eth_asset, borrower_account),
                Balance::from_nominal("0", ETH).value
            );
            assert_eq!(
                CashPrincipals::get(liquidator_account),
                CashPrincipal::from_nominal("3000")
            );
            assert_eq!(
                CashPrincipals::get(borrower_account),
                CashPrincipal::from_nominal("-9000")
            );
            assert_eq!(Nonces::get(liquidator_account), nonce + 1);
            assert_eq!(Nonces::get(borrower_account), 0);

            // Check emitted `LiquidateCash` event
            let index = GlobalCashIndex::get();
            let liquidate_event = System::events().into_iter().next().unwrap();
            assert_eq!(
                mock::Event::pallet_cash(crate::Event::LiquidateCash(
                    eth_asset,
                    liquidator_account,
                    borrower_account,
                    index
                        .cash_principal_amount(Quantity::from_nominal("1000", CASH))
                        .unwrap(),
                    index
                )),
                liquidate_event.event
            );
        });
    }

    // TODO: Implement max
    #[test]
    #[should_panic(expected = "Not supported")]
    fn exec_trx_liquidate_cash_borrowed_max() {
        new_test_ext().execute_with(|| {
            let eth_asset = init_eth_asset().unwrap();
            let _borrower_account = ChainAccount::Eth([1; 20]);
            let liquidator_account = ChainAccount::Eth([2; 20]);
            init_asset_balance(
                eth_asset,
                liquidator_account,
                Balance::from_nominal("3", ETH).value,
            );

            // Liquidate 1e18 Eth
            let req_str = "(Liquidate Max CASH Eth:0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee \
                Eth:0x0101010101010101010101010101010101010101)";
            let nonce = 0;

            let _ = exec_trx_request::<Test>(req_str, liquidator_account, Some(nonce));

            // TODO: Check balances
        });
    }

    // TODO: Test insufficient collateral
    // TODO: Test close too big

    #[test]
    fn exec_trx_liquidate_asset_for_asset_sufficient_liquidity() {
        new_test_ext().execute_with(|| {
            let wbtc_asset = init_wbtc_asset().unwrap();
            let eth_asset = init_eth_asset().unwrap();
            let borrower_account = ChainAccount::Eth([1; 20]);
            let liquidator_account = ChainAccount::Eth([2; 20]);
            init_asset_balance(
                wbtc_asset,
                borrower_account,
                Balance::from_nominal("5", WBTC).value,
            );
            init_asset_balance(
                wbtc_asset,
                liquidator_account,
                Balance::from_nominal("3", WBTC).value,
            );

            // Liquidate 1 WBTC
            let req_str = "(Liquidate 100000000 Eth:0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
                Eth:0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee \
                Eth:0x0101010101010101010101010101010101010101)";
            let nonce = 0;

            assert_eq!(
                exec_trx_request::<Test>(req_str, liquidator_account, Some(nonce)),
                Err(Reason::SufficientLiquidity)
            );

            assert_eq!(
                AssetBalances::get(eth_asset, liquidator_account),
                Balance::from_nominal("0", ETH).value
            );
            assert_eq!(
                AssetBalances::get(eth_asset, borrower_account),
                Balance::from_nominal("0", ETH).value
            );
            assert_eq!(
                AssetBalances::get(wbtc_asset, liquidator_account),
                Balance::from_nominal("3", WBTC).value
            );
            assert_eq!(
                AssetBalances::get(wbtc_asset, borrower_account),
                Balance::from_nominal("5", WBTC).value
            );
            assert_eq!(Nonces::get(liquidator_account), nonce);
            assert_eq!(Nonces::get(borrower_account), 0);
        });
    }

    #[test]
    fn exec_trx_liquidate_asset_for_asset_success() {
        new_test_ext().execute_with(|| {
            let wbtc_asset = init_wbtc_asset().unwrap();
            let eth_asset = init_eth_asset().unwrap();
            let borrower_account = ChainAccount::Eth([1; 20]);
            let liquidator_account = ChainAccount::Eth([2; 20]);
            init_asset_balance(
                eth_asset,
                borrower_account,
                Balance::from_nominal("32.4", ETH).value,
            );
            init_asset_balance(
                wbtc_asset,
                borrower_account,
                Balance::from_nominal("-5", WBTC).value,
            );
            init_asset_balance(
                wbtc_asset,
                liquidator_account,
                Balance::from_nominal("3", WBTC).value,
            );

            // Liquidate 1 WBTC
            let req_str = "(Liquidate 100000000 Eth:0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
                Eth:0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee \
                Eth:0x0101010101010101010101010101010101010101)";
            let nonce = 0;

            assert_ok!(exec_trx_request::<Test>(
                req_str,
                liquidator_account,
                Some(nonce)
            ));

            assert_eq!(
                AssetBalances::get(eth_asset, liquidator_account),
                Balance::from_nominal("32.4", ETH).value
            );
            assert_eq!(
                AssetBalances::get(eth_asset, borrower_account),
                Balance::from_nominal("0", ETH).value
            );
            assert_eq!(
                AssetBalances::get(wbtc_asset, liquidator_account),
                Balance::from_nominal("2", WBTC).value
            );
            assert_eq!(
                AssetBalances::get(wbtc_asset, borrower_account),
                Balance::from_nominal("-4", WBTC).value
            );
            assert_eq!(Nonces::get(liquidator_account), nonce + 1);
            assert_eq!(Nonces::get(borrower_account), 0);

            // Check emitted `Liquidate` event
            let liquidate_event = System::events().into_iter().next().unwrap();
            assert_eq!(
                mock::Event::pallet_cash(crate::Event::Liquidate(
                    wbtc_asset,
                    eth_asset,
                    liquidator_account,
                    borrower_account,
                    100000000
                )),
                liquidate_event.event
            );
        });
    }

    // TODO: Implement max
    #[test]
    #[should_panic(expected = "Not supported")]
    fn exec_trx_liquidate_asset_for_asset_max() {
        new_test_ext().execute_with(|| {
            let _wbtc_asset = init_wbtc_asset().unwrap();
            let eth_asset = init_eth_asset().unwrap();
            let _borrower_account = ChainAccount::Eth([1; 20]);
            let liquidator_account = ChainAccount::Eth([2; 20]);
            init_asset_balance(
                eth_asset,
                liquidator_account,
                Balance::from_nominal("3", ETH).value,
            );

            // Liquidate 1e18 Eth
            let req_str = "(Liquidate Max Eth:0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
                Eth:0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee \
                Eth:0x0101010101010101010101010101010101010101)";
            let nonce = 0;

            let _ = exec_trx_request::<Test>(req_str, liquidator_account, Some(nonce));

            // TODO: Check balances
        });
    }

    #[test]
    fn exec_trx_request_wrong_nonce() {
        new_test_ext().execute_with(|| {
            let req_str = "(Liquidate 55 Eth:0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee Eth:0x0d8775f648430679a709e98d2b0cb6250d2887ef Eth:0x0101010101010101010101010101010101010101)";
            let account = ChainAccount::Eth([20; 20]);
            let nonce = Some(3);

            Nonces::insert(account, 2);

            assert_eq!(exec_trx_request::<Test>(req_str, account, nonce), Err(Reason::IncorrectNonce(3, 2)));
        });
    }

    #[test]
    fn exec_trx_request_invalid_request() {
        new_test_ext().execute_with(|| {
            let req_str = "(INVALID_REQUEST)";
            let account = ChainAccount::Eth([20; 20]);
            let nonce = Some(3);

            assert_eq!(
                exec_trx_request::<Test>(req_str, account, nonce),
                Err(Reason::TrxRequestParseError(TrxReqParseError::LexError))
            );
        });
    }

    #[test]
    fn exec_trx_request_does_not_crash() {
        new_test_ext().execute_with(|| {
            let req_str = "(Extract 3000000 CASH Eth:0)";
            let account = ChainAccount::Eth([20; 20]);
            init_cash(account, CashPrincipal::from_nominal("4"));
            let nonce = Some(0);
            let res = exec_trx_request::<Test>(req_str, account, nonce);
            assert_eq!(
                res,
                Err(Reason::TrxRequestParseError(
                    TrxReqParseError::InvalidChainAccount
                ))
            );
        });
    }
}
