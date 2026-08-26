#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Map, Bytes};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    Unauthorized = 3,
    InvalidBatchSize = 4,
    BatchNotFound = 5,
    InvalidProof = 6,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchRecord {
    pub root: Bytes,
    pub count: u32,
    pub period_start: u64,
    pub period_end: u64,
    pub anchored_ledger: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Admin,
    BatchCount,
    PrunedUpTo,
    Batch(u64),
}

const MAX_BATCH_SIZE: u32 = 1000;

#[contract]
pub struct ReceiptAnchor;

#[contractimpl]
impl ReceiptAnchor {
    /// Initializes the receipt anchor contract with an admin (merchant) address.
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::BatchCount, &0u64);
        // PrunedUpTo invariant:
        // "PrunedUpTo" represents the water-mark up to which batches have been deliberately
        // pruned and deleted. Every batch ID strictly less than PrunedUpTo is guaranteed to have
        // been deliberately removed by prune_batches, or if missing due to TTL archival, it is
        // never allowed to sit below PrunedUpTo in a way that violates the contiguous prefix
        // guarantee. Specifically, the contract stops pruning or advancing the watermark upon
        // encountering any gap, ensuring restored batches can never land below PrunedUpTo.
        env.storage().instance().set(&DataKey::PrunedUpTo, &1u64);
        Ok(()) 
    }

    /// Returns the maximum allowed batch size.
    pub fn get_max_batch_size(_env: Env) -> u32 {
        MAX_BATCH_SIZE
    }

    /// Returns the total number of anchored batches.
    pub fn get_batch_count(env: Env) -> Result<u64, Error> {
        Self::check_initialized(&env)?;
        Ok(env.storage().instance().get(&DataKey::BatchCount).unwrap_or(0))
    }

    /// Anchors a new batch root.
    pub fn anchor_batch(
        env: Env,
        root: Bytes,
        count: u32,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();

        if count == 0 || count > MAX_BATCH_SIZE {
            return Err(Error::InvalidBatchSize);
        }

        let mut batch_count: u64 = env.storage().instance().get(&DataKey::BatchCount).unwrap_or(0);
        batch_count += 1;

        let anchored_ledger = env.ledger().sequence();
        let record = BatchRecord {
            root,
            count,
            period_start,
            period_end,
            anchored_ledger,
        };

        env.storage().persistent().set(&DataKey::Batch(batch_count), &record);
        env.storage().instance().set(&DataKey::BatchCount, &batch_count);

        let mut event_data = Map::new(&env);
        event_data.set(Bytes::from_slice(&env, b"root"), record.root.clone());
        event_data.set(Bytes::from_slice(&env, b"count"), record.count);
        event_data.set(Bytes::from_slice(&env, b"period_start"), record.period_start);
        event_data.set(Bytes::from_slice(&env, b"period_end"), record.period_end);
        event_data.set(Bytes::from_slice(&env, b"anchored_ledger"), record.anchored_ledger);

        env.events().publish(
            (Bytes::from_slice(&env, b"anchor_event"), batch_count),
            event_data,
        );

        Ok(batch_count)
    }

    /// Retrieves an anchored batch record by its ID.
    pub fn get_batch(env: Env, batch_id: u64) -> Result<BatchRecord, Error> {
        Self::check_initialized(&env)?;
        let pruned_up_to: u64 = env.storage().instance().get(&DataKey::PrunedUpTo).unwrap_or(1);
        if batch_id < pruned_up_to {
            return Err(Error::BatchNotFound);
        }
        env.storage()
            .persistent()
            .get(&DataKey::Batch(batch_id))
            .ok_or(Error::BatchNotFound)
    }

    /// Extends the TTL of a batch to prevent archival.
    pub fn extend_batch_ttl(env: Env, batch_id: u64) -> Result<(), Error> {
        Self::check_initialized(&env)?;
        let pruned_up_to: u64 = env.storage().instance().get(&DataKey::PrunedUpTo).unwrap_or(1);
        if batch_id < pruned_up_to {
            return Err(Error::BatchNotFound);
        }
        let key = DataKey::Batch(batch_id);
        if !env.storage().persistent().has(&key) {
            return Err(Error::BatchNotFound);
        }
        env.storage().persistent().extend_ttl(&key, 4096, 6312000);
        Ok(())
    }

    /// Prunes anchored batches older than `before_ledger`.
    ///
    /// Invariant: `PrunedUpTo` guarantees that all batches strictly below `PrunedUpTo` have been
    /// deliberately pruned. If a batch entry is missing due to TTL archival or manual removal rather
    /// than deliberate pruning, the loop halts immediately rather than advancing past the gap silently,
    /// preventing restored batches from landing below `PrunedUpTo`.
    pub fn prune_batches(env: Env, before_ledger: u32) -> Result<u64, Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();

        let batch_count: u64 = env.storage().instance().get(&DataKey::BatchCount).unwrap_or(0);
        let mut pruned_up_to: u64 = env.storage().instance().get(&DataKey::PrunedUpTo).unwrap_or(1);

        let initial_pruned_up_to = pruned_up_to;

        while pruned_up_to <= batch_count {
            let key = DataKey::Batch(pruned_up_to);
            if !env.storage().persistent().has(&key) {
                // Emit an observable event/log or handle gap to prevent silent incorrect advancement.
                let mut gap_data = Map::new(&env);
                gap_data.set(Bytes::from_slice(&env, b"batch_id"), pruned_up_to);
                env.events().publish(
                    (Bytes::from_slice(&env, b"prune_gap_event"), pruned_up_to),
                    gap_data,
                );
                break;
            }

            let record: BatchRecord = env.storage().persistent().get(&key).unwrap();
            if record.anchored_ledger < before_ledger {
                env.storage().persistent().remove(&key);
                pruned_up_to += 1;
            } else {
                break;
            }
        }

        env.storage().instance().set(&DataKey::PrunedUpTo, &pruned_up_to);

        if pruned_up_to > initial_pruned_up_to {
            let mut event_data = Map::new(&env);
            event_data.set(Bytes::from_slice(&env, b"end_batch_id"), pruned_up_to);
            env.events().publish(
                (
                    Bytes::from_slice(&env, b"prune_event"),
                    initial_pruned_up_to,
                ),
                event_data,
            );
        }

        Ok(pruned_up_to)
    }

    /// Verifies a receipt against an anchored batch root using sorted-pair SHA-256.
    pub fn verify_receipt(
        env: Env,
        batch_id: u64,
        leaf: Bytes,
        proof: soroban_sdk::Vec<Bytes>,
    ) -> Result<bool, Error> {
        let record = Self::get_batch(env.clone(), batch_id)?;
        let mut current = leaf;

        for sibling in proof.iter() {
            current = hash_sorted_pair(&env, &current, &sibling);
        }

        Ok(current == record.root)
    }

    fn check_initialized(env: &Env) -> Result<(), Error> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }
        Ok(())
    }
}

fn hash_sorted_pair(env: &Env, a: &Bytes, b: &Bytes) -> Bytes {
    let combined = if a < b {
        let mut buf = Bytes::new(env);
        buf.append(a);
        buf.append(b);
        buf
    } else {
        let mut buf = Bytes::new(env);
        buf.append(b);
        buf.append(a);
        buf
    };
    let digest = env.crypto().sha256(&combined);
    Bytes::from_array(env, &digest.to_array())
}

#[cfg(test)]
mod test;
