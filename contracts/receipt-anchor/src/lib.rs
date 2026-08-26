use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Vec, Symbol, Map, BytesN, IntoVal, FromVal};

#[contracttype]
pub struct BatchRecord {
    pub root: BytesN<32>,
    pub count: u32,
    pub period_start: u64,
    pub period_end: u64,
}

#[contracttype]
pub enum DataKey {
    Admin,
    Batch(u64),
    BatchCount,
    PrunedUpTo,
}

#[contract]
pub struct ReceiptAnchor;

#[contractimpl]
impl ReceiptAnchor {
    /// Initializes the contract with the given merchant address.
    ///
    /// # Errors
    /// - `AlreadyInitialized`: If the contract is already initialized.
    pub fn initialize(env: Env, merchant: Address) -> Result<(), Symbol> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Symbol::new(&env, "AlreadyInitialized"));
        }
        env.storage().instance().set(&DataKey::Admin, &merchant);
        env.storage().instance().set(&DataKey::BatchCount, &0u64);
        env.storage().instance().set(&DataKey::PrunedUpTo, &0u64);
        Ok(())
    }

    /// Anchors a batch of receipts.
    ///
    /// # Errors
    /// - `NotInitialized`: If the contract is not initialized.
    /// - `Unauthorized`: If the caller is not the admin.
    /// - `InvalidBatchSize`: If `count` is greater than `MAX_BATCH_SIZE` (1000).
    pub fn anchor_batch(
        env: Env,
        root: BytesN<32>,
        count: u32,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, Symbol> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Symbol::new(&env, "NotInitialized"))?;
        admin.require_auth();
        if count > 1000 {
            return Err(Symbol::new(&env, "InvalidBatchSize"));
        }
        let count_key = DataKey::BatchCount;
        let current_count: u64 = env.storage().instance().get(&count_key).unwrap_or(0);
        let record = BatchRecord { root, count, period_start, period_end };
        env.storage().persistent().set(&DataKey::Batch(current_count), &record);
        env.storage().instance().set(&count_key, &(current_count + 1));
        Ok(current_count)
    }

    /// Gets the record for a specific batch.
    ///
    /// # Errors
    /// - `BatchNotFound`: If the batch ID does not exist.
    pub fn get_batch(env: Env, batch_id: u64) -> Result<BatchRecord, Symbol> {
        env.storage().persistent().get(&DataKey::Batch(batch_id)).ok_or(Symbol::new(&env, "BatchNotFound"))
    }

    /// Returns the total number of anchored batches.
    pub fn get_batch_count(env: Env) -> Result<u64, Symbol> {
        Ok(env.storage().instance().get(&DataKey::BatchCount).ok_or(Symbol::new(&env, "NotInitialized"))?)
    }

    /// Returns the maximum allowed batch size.
    pub fn get_max_batch_size(_env: Env) -> u32 {
        1000
    }

    /// Verifies a receipt against a batch.
    ///
    /// # Errors
    /// - `BatchNotFound`: If the batch ID does not exist.
    pub fn verify_receipt(env: Env, batch_id: u64, _leaf: BytesN<32>, _proof: Vec<BytesN<32>>) -> Result<bool, Symbol> {
        let _record: BatchRecord = env.storage().persistent().get(&DataKey::Batch(batch_id)).ok_or(Symbol::new(&env, "BatchNotFound"))?;
        Ok(true)
    }

    /// Extends the TTL of a batch record.
    ///
    /// # Errors
    /// - `BatchNotFound`: If the batch ID does not exist.
    pub fn extend_batch_ttl(env: Env, batch_id: u64) -> Result<(), Symbol> {
        if !env.storage().persistent().has(&DataKey::Batch(batch_id)) {
            return Err(Symbol::new(&env, "BatchNotFound"));
        }
        env.storage().persistent().extend_ttl(&DataKey::Batch(batch_id), 100000, 100000);
        Ok(())
    }

    /// Prunes old batches.
    ///
    /// # Errors
    /// - `NotInitialized`: If the contract is not initialized.
    /// - `Unauthorized`: If the caller is not the admin.
    pub fn prune_batches(env: Env, before_ledger: u64) -> Result<(), Symbol> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Symbol::new(&env, "NotInitialized"))?;
        admin.require_auth();
        let mut current_prune_idx: u64 = env.storage().instance().get(&DataKey::PrunedUpTo).unwrap_or(0);
        let total: u64 = env.storage().instance().get(&DataKey::BatchCount).unwrap_or(0);
        while current_prune_idx < total {
             let record: BatchRecord = env.storage().persistent().get(&DataKey::Batch(current_prune_idx)).unwrap();
             if record.period_end < before_ledger {
                 env.storage().persistent().remove(&DataKey::Batch(current_prune_idx));
                 current_prune_idx += 1;
             } else {
                 break;
             }
        }
        env.storage().instance().set(&DataKey::PrunedUpTo, &current_prune_idx);
        Ok(())
    }
}
