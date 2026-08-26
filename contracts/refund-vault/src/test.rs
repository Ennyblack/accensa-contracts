#![cfg(test)]

use super::*;
use soroban_sdk::{Env, Address};

#[test]
fn test_initialize_emits_event()
{
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(RefundVault, ());
    let client = RefundVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    client.initialize(&admin, &token, &100);

    let events = env.events().all();
    assert_eq!(events.len(), 1);

    let event = events.last().unwrap();
    assert_eq!(event.contract_id, contract_id);
    assert_eq!(
        event.topic,
        (soroban_sdk::Symbol::new(&env, "initialize_event"), admin.clone()).into()
    );
}

#[test]
fn test_failed_initialize_emits_no_event()
{
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(RefundVault, ());
    let client = RefundVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    client.initialize(&admin, &token, &100);

    let events_before = env.events().all();
    assert_eq!(events_before.len(), 1);

    let res = client.try_initialize(&admin, &token, &100);
    assert!(res.is_err());

    let events_after = env.events().all();
    assert_eq!(events_after.len(), 1);
}
