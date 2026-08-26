#![no_std]

use soroban_sdk::{contract, contractimpl, contractevent, contracterror, Address, BytesN, Env};

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitializeEvent {
    #[topic]
    pub admin: Address,
    pub version: soroban_sdk::String,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnchorEvent {
    #[topic]
    pub batch_id: u64,
    pub root: BytesN<32>,
    pub count: u32,
    pub period_start: u64,
    pub period_end: u64,
    pub anchored_ledger: u32,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PruneEvent {
    #[topic]
    pub start_batch_id: u64,
    pub end_batch_id: u64,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    BatchTooLarge = 4,
    BatchNotFound = 5,
    InvalidRange = 6,
}

#[derive(Clone)]
#[soroban_sdk::contracttype]
pub struct BatchRecord {
    pub root: BytesN<32>,
    pub count: u32,
    pub period_start: u64,
    pub period_end: u64,
    pub anchored_ledger: u32,
}

#[derive(Clone)]
#[soroban_sdk::contracttype]
pub enum DataKey {
    Admin,
    BatchCount,
    Batch(u64),
    PrunedUpTo,
}

#[contract]
pub struct ReceiptAnchor;

#[contractimpl]
impl ReceiptAnchor {
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if env.storage().persistent().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage().persistent().set(&DataKey::BatchCount, &0u64);
        env.storage().persistent().set(&DataKey::PrunedUpTo, &1u64);

        env.events().publish(
            (
                soroban_sdk::Symbol::new(&env, "initialize_event"),
                admin.clone(),
            ),
            InitializeEvent {
                admin,
                version: soroban_sdk::String::from_str(&env, env.contract_version()),
            },
        );

        Ok(())
    }

    pub fn anchor_batch(
        env: Env,
        root: BytesN<32>,
        count: u32,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, Error> {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();

        if count == 0 || count > Self::get_max_batch_size(env.clone()) {
            return Err(Error::BatchTooLarge);
        }

        let mut batch_count: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::BatchCount)
            .unwrap_or(0);
        batch_count += 1;

        let anchored_ledger = env.ledger().sequence();
        let record = BatchRecord {
            root: root.clone(),
            count,
            period_start,
            period_end,
            anchored_ledger,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Batch(batch_count), &record);
        env.storage()
            .persistent()
            .set(&DataKey::BatchCount, &batch_count);

        env.events().publish(
            (
                soroban_sdk::Symbol::new(&env, "anchor_event"),
                batch_count,
            ),
            AnchorEvent {
                batch_id: batch_count,
                root,
                count,
                period_start,
                period_end,
                anchored_ledger,
            },
        );

        Ok(batch_count)
    }

    pub fn get_batch(env: Env, batch_id: u64) -> Result<BatchRecord, Error> {
        let pruned_up_to: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::PrunedUpTo)
            .unwrap_or(1);
        if batch_id < pruned_up_to {
            return Err(Error::BatchNotFound);
        }
        env.storage()
            .persistent()
            .get(&DataKey::Batch(batch_id))
            .ok_or(Error::BatchNotFound)
    }

    pub fn get_batch_count(env: Env) -> Result<u64, Error> {
        if !env.storage().persistent().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }
        Ok(env
            .storage()
            .persistent()
            .get(&DataKey::BatchCount)
            .unwrap_or(0))
    }

    pub fn get_max_batch_size(_env: Env) -> u32 {
        1000
    }

    pub fn verify_receipt(
        env: Env,
        batch_id: u64,
        leaf: BytesN<32>,
        proof: soroban_sdk::Vec<BytesN<32>>,
    ) -> Result<bool, Error> {
        let record = Self::get_batch(env.clone(), batch_id)?;
        let mut current = leaf;

        for sibling in proof.iter() {
            let mut combined = soroban_sdk::Bytes::new(&env);
            if current < sibling {
                combined.append(&current.into());
                combined.append(&sibling.into());
            } else {
                combined.append(&sibling.into());
                combined.append(&current.into());
            }
            let hash = env.crypto().sha256(&combined);
            current = hash.into();
        }

        Ok(current == record.root)
    }

    pub fn extend_batch_ttl(env: Env, batch_id: u64) -> Result<(), Error> {
        let pruned_up_to: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::PrunedUpTo)
            .unwrap_or(1);
        if batch_id < pruned_up_to {
            return Err(Error::BatchNotFound);
        }
        if !env.storage().persistent().has(&DataKey::Batch(batch_id)) {
            return Err(Error::BatchNotFound);
        }
        env.storage()
            .persistent()
            .extend_ttl(&DataKey::Batch(batch_id), 4096, 6312000);
        Ok()
    }

    pub fn prune_batches(env: Env, before_ledger: u32) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();

        let mut pruned_up_to: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::PrunedUpTo)
            .unwrap_or(1);
        let batch_count: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::BatchCount)
            .unwrap_or(0);

        let start_pruned = pruned_up_to;
        let mut pruned_any = false;

        while pruned_up_to <= batch_count {
            if let Some(record) = env
                .storage()
                .persistent()
                .get::<_, BatchRecord>(&DataKey::Batch(pruned_up_to))
            {
                if record.anchored_ledger < before_ledger {
                    env.storage().persistent().remove(&DataKey::Batch(pruned_up_to));
                    pruned_up_to += 1;
                    pruned_any = true;
                } else {
                    break;
                }
            } else {
                pruned_up_to += 1;
            }
        }

        if pruned_any {
            env.storage()
                .persistent()
                .set(&DataKey::PrunedUpTo, &pruned_up_to);
            env.events().publish(
                (
                    soroban_sdk::Symbol::new(&env, "prune_event"),
                    start_pruned,
                ),
                PruneEvent {
                    start_batch_id: start_pruned,
                    end_batch_id: pruned_up_to,
                },
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod test;
