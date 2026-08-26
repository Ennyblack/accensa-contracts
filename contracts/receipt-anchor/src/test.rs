#![cfg(test)]

use super::*;
use soroban_sdk::{Env, Bytes, vec};

#[test]
fn test_initialize() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(ReceiptAnchor, ());
    let client = ReceiptAnchorClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.initialize(&admin);

    assert_eq!(client.get_batch_count(), 0);
    assert_eq!(client.get_max_batch_size(), 1000);
}

#[test]
fn test_anchor_batch_assigns_sequential_ids() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(ReceiptAnchor, ());
    let client = ReceiptAnchorClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let root1 = Bytes::from_slice(&env, &[1u8; 32]);
    let id1 = client.anchor_batch(&root1, &10, &0, &10);
    assert_eq!(id1, 1);

    let root2 = Bytes::from_slice(&env, &[2u8; 32]);
    let id2 = client.anchor_batch(&root2, &10, &11, &20);
    assert_eq!(id2, 2);

    assert_eq!(client.get_batch_count(), 2);
}

#[test]
fn test_prune_batches_stops_at_gap_and_does_not_advance_past_it() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(ReceiptAnchor, ());
    let client = ReceiptAnchorClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let root1 = Bytes::from_slice(&env, &[1u8; 32]);
    let root2 = Bytes::from_slice(&env, &[2u8; 32]);
    let root3 = Bytes::from_slice(&env, &[3u8; 32]);

    client.anchor_batch(&root1, &10, &0, &10);
    client.anchor_batch(&root2, &10, &11, &20);
    client.anchor_batch(&root3, &10, &21, &30);

    // Manually remove/archive batch 1 to simulate TTL archival or missing entry gap
    env.as_contract(&contract_id, || {
        env.storage().persistent().remove(&DataKey::Batch(1));
    });

    // Pruning with high ledger sequence should encounter gap at batch 1 and halt
    env.ledger().set_sequence_number(300);
    let pruned = client.prune_batches(&400);
    
    // PrunedUpTo must stay at 1 because batch 1 was missing (gap encountered)
    assert_eq!(pruned, 1);
}
