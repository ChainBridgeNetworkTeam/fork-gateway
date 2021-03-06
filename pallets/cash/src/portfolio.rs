use crate::{
    internal::assets::get_price,
    reason::Reason,
    symbol::CASH,
    types::{AssetInfo, Balance},
    Config,
};
use codec::{Decode, Encode};
use our_std::RuntimeDebug;
use types_derive::Types;

//  投资组合
/// Type for representing a set of positions for an account.
#[derive(Clone, Eq, PartialEq, Encode, Decode, RuntimeDebug, Types)]
pub struct Portfolio {
    pub cash: Balance,
    pub positions: Vec<(AssetInfo, Balance)>,
}

impl Portfolio {
    //  获得假定的流动性的值，返回总的资产的价值，cash加上各个其他资产的价值
    /// Get the hypothetical liquidity value.
    pub fn get_liquidity<T: Config>(&self) -> Result<Balance, Reason> {
        //  获得美元计价的流动性
        let mut liquidity = self.cash.mul_price(get_price::<T>(CASH)?)?;
        //  遍历多个头寸点位
        for (info, balance) in &self.positions {
            //  单价
            let price = get_price::<T>(balance.units)?;
            let worth = (*balance).mul_price(price)?;
            if worth.value >= 0 {
                //  大于0乘以流动性因子
                liquidity = liquidity.add(worth.mul_factor(info.liquidity_factor)?)?
            } else {
                liquidity = liquidity.add(worth.div_factor(info.liquidity_factor)?)?
            }
        }
        Ok(liquidity)
    }
}

#[cfg(test)]
pub mod tests {
    use crate::{pipeline, tests::*};

    struct TestAsset {
        asset: u8,
        ticker: &'static str,
        decimals: u8,
        balance: &'static str,
        price: &'static str,
        liquidity_factor: &'static str,
    }

    struct GetLiquidityTestCase {
        cash_index: &'static str,
        cash_principal: Option<&'static str>,
        expected_liquidity: Result<&'static str, Reason>,
        test_assets: Vec<TestAsset>,
        error_message: &'static str,
    }

    fn test_get_liquidity_test_case(case: GetLiquidityTestCase) {
        // todo: xxx finish this test
        new_test_ext().execute_with(|| {
            let account = ChainAccount::Eth([0; 20]);

            for asset_case in case.test_assets {
                let asset = ChainAsset::Eth([asset_case.asset; 20]);
                let decimals = asset_case.decimals;
                let liquidity_factor = LiquidityFactor::from_nominal(asset_case.liquidity_factor);
                let miner_shares: MinerShares = Default::default();
                let rate_model: InterestRateModel = Default::default();
                let supply_cap = AssetAmount::MAX;
                let symbol = Symbol::new(&asset_case.ticker);
                let ticker = Ticker::new(&asset_case.ticker);

                let asset_info = AssetInfo {
                    asset,
                    decimals,
                    liquidity_factor,
                    miner_shares,
                    rate_model,
                    supply_cap,
                    symbol,
                    ticker,
                };
                SupportedAssets::insert(asset, asset_info);

                let price = Price::from_nominal(ticker, asset_case.price);
                pallet_oracle::Prices::insert(ticker, price.value);

                let units = Units::from_ticker_str(&asset_case.ticker, asset_case.decimals);

                let balance = Balance::from_nominal(asset_case.balance, units);
                AssetBalances::insert(&asset, &account, balance.value);
                AssetsWithNonZeroBalance::insert(&account, &asset, ());
            }

            GlobalCashIndex::put(CashIndex::from_nominal(case.cash_index));
            if let Some(cash_principal) = case.cash_principal {
                CashPrincipals::insert(&account, CashPrincipal::from_nominal(cash_principal));
            }

            let actual = pipeline::load_portfolio::<Test>(account)
                .unwrap()
                .get_liquidity::<Test>();
            let expected = match case.expected_liquidity {
                Ok(liquidity_str) => Ok(Balance::from_nominal(liquidity_str, USD)),
                Err(e) => Err(e),
            };

            assert_eq!(expected, actual, "{}", case.error_message);
        });
    }

    fn get_test_liquidity_cases() -> Vec<GetLiquidityTestCase> {
        // todo: xxx finish these test cases
        vec![
            GetLiquidityTestCase {
                cash_index: "1",
                cash_principal: None,
                expected_liquidity: Ok("0"),
                test_assets: vec![],
                error_message: "Empty account has zero liquidity",
            },
            GetLiquidityTestCase {
                cash_index: "1.1234",
                cash_principal: Some("123.123456"),
                expected_liquidity: Ok("138.316890"),
                test_assets: vec![],
                error_message: "Cash only applies principal correctly",
            },
            GetLiquidityTestCase {
                cash_index: "1.1234",
                cash_principal: Some("-123.123456"),
                expected_liquidity: Ok("-138.316890"),
                test_assets: vec![],
                error_message: "Negative cash is negative liquidity",
            },
            GetLiquidityTestCase {
                cash_index: "1.1234",
                cash_principal: None,
                expected_liquidity: Ok("82556.557312"), // last digit calcs as a 3 in gsheet
                test_assets: vec![TestAsset {
                    asset: 1,
                    ticker: "abc",
                    decimals: 6,
                    balance: "123.123456",
                    price: "987.654321",
                    liquidity_factor: "0.6789",
                }],
                error_message: "Singe asset supplied liquidity",
            },
            GetLiquidityTestCase {
                cash_index: "1.1234",
                cash_principal: None,
                expected_liquidity: Ok("-21.261329"), // sheet says -21.261334 diff -0.0000052
                test_assets: vec![
                    TestAsset {
                        asset: 1,
                        ticker: "abc",
                        decimals: 6,
                        balance: "123.123456",
                        price: "987.654321",
                        liquidity_factor: "0.6789",
                    },
                    TestAsset {
                        asset: 2,
                        ticker: "def",
                        decimals: 6,
                        balance: "-12.123456",
                        price: "987.654321",
                        liquidity_factor: "0.1450",
                    },
                ],
                error_message: "Slightly undercollateralized account",
            },
            GetLiquidityTestCase {
                cash_index: "1.1234",
                cash_principal: Some("123.123456"),
                expected_liquidity: Ok("117.055561"),
                test_assets: vec![
                    TestAsset {
                        asset: 1,
                        ticker: "abc",
                        decimals: 6,
                        balance: "123.123456",
                        price: "987.654321",
                        liquidity_factor: "0.6789",
                    },
                    TestAsset {
                        asset: 2,
                        ticker: "def",
                        decimals: 6,
                        balance: "-12.123456",
                        price: "987.654321",
                        liquidity_factor: "0.1450",
                    },
                ],
                error_message:
                    "Slightly undercollateralized by assets but with some offsetting positive cash",
            },
        ]
    }

    #[test]
    fn test_get_liquidity_all_cases() {
        get_test_liquidity_cases()
            .drain(..)
            .for_each(test_get_liquidity_test_case);
    }
}
