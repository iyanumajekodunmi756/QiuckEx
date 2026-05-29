use crate::{types::{FeeConfig, PerAssetFeeConfig}, QuickexContract, QuickexContractClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Bytes, Env,
};

fn setup_test(
    env: &Env,
) -> (
    QuickexContractClient<'_>,
    Address,
    Address,
    Address,
    Address,
) {
    let admin = Address::generate(env);
    let platform_wallet = Address::generate(env);
    let owner = Address::generate(env);
    let recipient = Address::generate(env);

    let contract_id = env.register(QuickexContract, ());
    let client = QuickexContractClient::new(env, &contract_id);

    client.initialize(&admin);

    (client, admin, platform_wallet, owner, recipient)
}

#[test]
fn test_fee_admin() {
    let env = Env::default();
    let (client, admin, platform_wallet, _, _) = setup_test(&env);

    env.mock_all_auths();

    // Set fee config
    let fee_config = FeeConfig { fee_bps: 250 }; // 2.5%
    client.set_fee_config(&admin, &fee_config);

    assert_eq!(client.get_fee_config().fee_bps, 250);

    // Set platform wallet
    client.set_platform_wallet(&admin, &platform_wallet);
    assert_eq!(client.get_platform_wallet(), Some(platform_wallet));
}

#[test]
fn test_withdrawal_with_fee() {
    let env = Env::default();
    env.ledger().with_mut(|li| li.timestamp = 1000);

    let (client, admin, platform_wallet, owner, _recipient) = setup_test(&env);

    // Setup token
    let token_admin = Address::generate(&env);
    let token_id = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = token::Client::new(&env, &token_id);
    let token_admin_client = token::StellarAssetClient::new(&env, &token_id);

    env.mock_all_auths();

    token_admin_client.mint(&owner, &10000);

    // Configure fees
    client.set_fee_config(&admin, &FeeConfig { fee_bps: 1000 }); // 10%
    client.set_platform_wallet(&admin, &platform_wallet);

    // Deposit
    let amount = 1000i128;
    let salt = Bytes::from_array(&env, &[1; 32]);
    let commitment = client.deposit(&token_id, &amount, &owner, &salt, &3600, &None);

    assert_eq!(token_client.balance(&owner), 9000);
    assert_eq!(token_client.balance(&client.address), 1000);

    // Withdraw (payout to recipient)
    // Salt must match the one used during deposit.
    // Commitment is recomputed from recipient, amount, and salt.
    // Wait, the commitment is recomputed from recipient during withdrawal in `escrow::withdraw`.
    // So the recipient must be the one whose address was used to create the commitment.
    // In `QuickexContract::deposit`, the commitment is created using `owner`.
    // Wait, let's check `escrow::deposit`:
    // `let commitment = commitment::create_amount_commitment(env, owner.clone(), amount, salt)?;`
    // And `escrow::withdraw`:
    // `let commitment = commitment::create_amount_commitment(env, to.clone(), amount, salt)?;`
    // This means the `owner` in `deposit` is the RECIPIENT who can withdraw.
    // Let me re-read `deposit`.
    // `pub fn deposit(..., owner: Address, salt: Bytes, ...)`
    // The `owner` is the one who can authorize the transfer AND whose address is in the commitment.
    // So if Alice deposits for Bob, Bob's address should be used in the commitment if Bob is to withdraw.
    // But `deposit` takes `amount` FROM `owner`.
    // Let's re-verify:
    // `owner.require_auth(); ... token_client.transfer(&owner, env.current_contract_address(), &amount);`
    // So `owner` is the depositor. And `withdraw` uses `to.require_auth()` and checks the commitment with `to`.
    // This means by default, only the depositor can withdraw to themselves using the commitment.
    // If they want someone else to withdraw, they'd need a different flow or use a different address in the commitment.
    // Actually, the commitment is `SHA256(owner || amount || salt)`.
    // If Alice deposits, the commitment is `SHA256(Alice || amount || salt)`. Only Alice can withdraw using this commitment.

    // Let's proceed with Alice (owner) withdrawing to herself.
    client.withdraw(&token_id, &amount, &commitment, &owner, &salt);

    // Fee is 10% of 1000 = 100.
    // Alice should get 1000 - 100 = 900.
    // Total balance for Alice: 9000 + 900 = 9900.
    assert_eq!(token_client.balance(&owner), 9900);
    assert_eq!(token_client.balance(&platform_wallet), 100);
    assert_eq!(token_client.balance(&client.address), 0);
}

#[test]
fn test_zero_fee() {
    let env = Env::default();
    let (client, admin, platform_wallet, owner, _) = setup_test(&env);

    let token_admin = Address::generate(&env);
    let token_id = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = token::Client::new(&env, &token_id);
    let token_admin_client = token::StellarAssetClient::new(&env, &token_id);

    env.mock_all_auths();

    token_admin_client.mint(&owner, &10000);

    // 0 Fee bps
    client.set_fee_config(&admin, &FeeConfig { fee_bps: 0 });
    client.set_platform_wallet(&admin, &platform_wallet);

    let amount = 1000i128;
    let salt = Bytes::from_array(&env, &[1; 32]);
    let commitment = client.deposit(&token_id, &amount, &owner, &salt, &3600, &None);

    client.withdraw(&token_id, &amount, &commitment, &owner, &salt);

    assert_eq!(token_client.balance(&owner), 10000);
    assert_eq!(token_client.balance(&platform_wallet), 0);
}

#[test]
fn test_fee_config_max_constraint() {
    let env = Env::default();
    let (client, admin, platform_wallet, _, _) = setup_test(&env);

    env.mock_all_auths();

    // Test that fee config exceeding MAX_FEE_BPS (1000 = 10%) fails
    let excessive_fee = FeeConfig { fee_bps: 1001 }; // Exceeds 10%
    let result = client.try_set_fee_config(&admin, &excessive_fee);
    assert!(result.is_err()); // Should fail with InvalidAmount error

    // Test that fee config at exactly MAX_FEE_BPS succeeds
    let max_fee = FeeConfig { fee_bps: 1000 }; // Exactly 10%
    client.set_fee_config(&admin, &max_fee);
    assert_eq!(client.get_fee_config().fee_bps, 1000);

    // Test that reasonable fee config succeeds
    let normal_fee = FeeConfig { fee_bps: 250 }; // 2.5%
    client.set_fee_config(&admin, &normal_fee);
    assert_eq!(client.get_fee_config().fee_bps, 250);
}

#[test]
fn test_per_asset_fee_max_constraint() {
    let env = Env::default();
    let (client, admin, _, _, _) = setup_test(&env);

    env.mock_all_auths();

    let token = Address::generate(&env);

    // Test that per-asset fee config exceeding MAX_FEE_BPS fails
    let excessive_per_asset = PerAssetFeeConfig { 
        fee_bps: 1001, // Exceeds 10%
        arbiter_bps: 0 
    };
    let result = client.try_set_per_asset_fee(&admin, &token, &excessive_per_asset);
    assert!(result.is_err()); // Should fail with InvalidAmount error

    // Test that per-asset fee config at exactly MAX_FEE_BPS succeeds
    let max_per_asset = PerAssetFeeConfig { 
        fee_bps: 1000, // Exactly 10%
        arbiter_bps: 0 
    };
    client.set_per_asset_fee(&admin, &token, &max_per_asset);
    
    let retrieved = client.get_per_asset_fee(&token);
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().fee_bps, 1000);
}

#[test]
fn test_extreme_fee_values() {
    let env = Env::default();
    let (client, admin, platform_wallet, owner, recipient) = setup_test(&env);

    // Setup token
    let token_admin = Address::generate(&env);
    let token_id = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = token::Client::new(&env, &token_id);
    let token_admin_client = token::StellarAssetClient::new(&env, &token_id);

    env.mock_all_auths();

    // Test with maximum allowed fee (10%)
    client.set_fee_config(&admin, &FeeConfig { fee_bps: 1000 });
    client.set_platform_wallet(&admin, &platform_wallet);

    token_admin_client.mint(&owner, &10000);

    let amount = 1000i128;
    let salt = Bytes::from_array(&env, &[1; 32]);
    let commitment = client.deposit(&token_id, &amount, &owner, &salt, &3600, &None);

    client.withdraw(&token_id, &amount, &commitment, &owner, &salt);

    // Fee should be exactly 10%: 1000 * 1000 / 10000 = 100
    assert_eq!(token_client.balance(&owner), 9900);
    assert_eq!(token_client.balance(&platform_wallet), 100);
    assert_eq!(token_client.balance(&client.address), 0);
}

#[test]
fn test_small_amount_fee_calculation() {
    let env = Env::default();
    let (client, admin, platform_wallet, owner, _) = setup_test(&env);

    // Setup token
    let token_admin = Address::generate(&env);
    let token_id = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = token::Client::new(&env, &token_id);
    let token_admin_client = token::StellarAssetClient::new(&env, &token_id);

    env.mock_all_auths();

    // Test with very small amounts to ensure floor division works correctly
    client.set_fee_config(&admin, &FeeConfig { fee_bps: 100 }); // 1%
    client.set_platform_wallet(&admin, &platform_wallet);

    // Test with amount = 1 (smallest possible positive amount)
    token_admin_client.mint(&owner, &100);

    let amount = 1i128;
    let salt = Bytes::from_array(&env, &[1; 32]);
    let commitment = client.deposit(&token_id, &amount, &owner, &salt, &3600, &None);

    client.withdraw(&token_id, &amount, &commitment, &owner, &salt);

    // Fee should be floor(1 * 100 / 10000) = floor(0.01) = 0
    assert_eq!(token_client.balance(&owner), 100);
    assert_eq!(token_client.balance(&platform_wallet), 0);
}

#[test]
fn test_rounding_determinism() {
    let env = Env::default();
    let (client, admin, platform_wallet, owner, _) = setup_test(&env);

    // Setup token
    let token_admin = Address::generate(&env);
    let token_id = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = token::Client::new(&env, &token_id);
    let token_admin_client = token::StellarAssetClient::new(&env, &token_id);

    env.mock_all_auths();

    // Test with a fee that produces fractional results
    client.set_fee_config(&admin, &FeeConfig { fee_bps: 333 }); // 3.33%
    client.set_platform_wallet(&admin, &platform_wallet);

    token_admin_client.mint(&owner, &10000);

    // Test amount that would produce fractional fee: 1000 * 333 / 10000 = 33.3
    // With floor division, this should be 33
    let amount = 1000i128;
    let salt = Bytes::from_array(&env, &[1; 32]);
    let commitment = client.deposit(&token_id, &amount, &owner, &salt, &3600, &None);

    client.withdraw(&token_id, &amount, &commitment, &owner, &salt);

    // Fee should be floor(33.3) = 33, net payout = 967
    assert_eq!(token_client.balance(&owner), 967);
    assert_eq!(token_client.balance(&platform_wallet), 33);
    assert_eq!(token_client.balance(&client.address), 0);
}
