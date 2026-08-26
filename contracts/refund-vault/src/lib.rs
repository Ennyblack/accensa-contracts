#![no_std]

use soroban_sdk::{contract, contractimpl, contractevent, contracterror, Address, BytesN, Env, token};

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitializeEvent {
    #[topic]
    pub admin: Address,
    pub token: Address,
    pub refund_window_ledgers: u32,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DepositEvent {
    #[topic]
    pub from: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundEvent {
    #[topic]
    pub payment_ref: BytesN<32>,
    pub amount: i128,
    pub recipient: Address,
    pub ledger: u32,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WithdrawEvent {
    #[topic]
    pub to: Address,
    pub amount: i128,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    InvalidAmount = 4,
    AlreadyRefunded = 5,
    WindowExpired = 6,
    Paused = 7,
    NoPendingAdmin = 8,
}

#[derive(Clone)]
#[soroban_sdk::contracttype]
pub struct RefundRecord {
    pub amount: i128,
    pub recipient: Address,
    pub ledger: u32,
}

#[derive(Clone)]
#[soroban_sdk::contracttype]
pub enum DataKey {
    Admin,
    Token,
    RefundWindow,
    Paused,
    Refund(BytesN<32>),
    PendingAdmin,
}

#[contract]
pub struct RefundVault;

#[contractimpl]
impl RefundVault {
    pub fn initialize(
        env: Env,
        admin: Address,
        token: Address,
        refund_window_ledgers: u32,
    ) -> Result<(), Error> {
        if env.storage().persistent().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage().persistent().set(&DataKey::Token, &token);
        env.storage()
            .persistent()
            .set(&DataKey::RefundWindow, &refund_window_ledgers);
        env.storage().persistent().set(&DataKey::Paused, &false);

        env.events().publish(
            (
                soroban_sdk::Symbol::new(&env, "initialize_event"),
                admin.clone(),
            ),
            InitializeEvent {
                admin,
                token,
                refund_window_ledgers,
            },
        );

        Ok(())
    }

    pub fn deposit(env: Env, from: Address, amount: i128) -> Result<(), Error> {
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let token_addr: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Token)
            .ok_or(Error::NotInitialized)?;
        from.require_auth();

        let token_client = token::Client::new(&env, &token_addr);
        token_client.transfer(&from, &env.current_contract_address(), &amount);

        env.events().publish(
            (
                soroban_sdk::Symbol::new(&env, "deposit_event"),
                from.clone(),
            ),
            DepositEvent { from, amount },
        );

        Ok(())
    }

    pub fn refund(
        env: Env,
        payment_ref: BytesN<32>,
        recipient: Address,
        amount: i128,
        paid_at_ledger: u32,
    ) -> Result<(), Error> {
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let is_paused: bool = env
            .storage()
            .persistent()
            .get(&DataKey::Paused)
            .unwrap_or(false);
        if is_paused {
            return Err(Error::Paused);
        }

        if env.storage().persistent().has(&DataKey::Refund(payment_ref.clone())) {
            return Err(Error::AlreadyRefunded);
        }

        let refund_window: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::RefundWindow)
            .ok_or(Error::NotInitialized)?;

        if refund_window > 0 {
            let current_ledger = env.ledger().sequence();
            if current_ledger > paid_at_ledger + refund_window {
                return Err(Error::WindowExpired);
            }
        }

        let token_addr: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Token)
            .ok_or(Error::NotInitialized)?;

        let record = RefundRecord {
            amount,
            recipient: recipient.clone(),
            ledger: paid_at_ledger,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Refund(payment_ref.clone()), &record);

        let token_client = token::Client::new(&env, &token_addr);
        token_client.transfer(&env.current_contract_address(), &recipient, &amount);

        env.events().publish(
            (
                soroban_sdk::Symbol::new(&env, "refund_event"),
                payment_ref.clone(),
            ),
            RefundEvent {
                payment_ref,
                amount,
                recipient,
                ledger: paid_at_ledger,
            },
        );

        Ok(())
    }

    pub fn withdraw(env: Env, amount: i128, to: Address) -> Result<(), Error> {
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();

        let token_addr: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Token)
            .unwrap_or_else(|| panic!("not initialized"));

        let token_client = token::Client::new(&env, &token_addr);
        token_client.transfer(&env.current_contract_address(), &to, &amount);

        env.events().publish(
            (
                soroban_sdk::Symbol::new(&env, "withdraw_event"),
                to.clone(),
            ),
            WithdrawEvent { to, amount },
        );

        Ok(())
    }

    pub fn set_refund_window(env: Env, ledgers: u32) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();

        env.storage()
            .persistent()
            .set(&DataKey::RefundWindow, &ledgers);
        Ok(())
    }

    pub fn get_refund(env: Env, payment_ref: BytesN<32>) -> Option<RefundRecord> {
        env.storage().persistent().get(&DataKey::Refund(payment_ref))
    }

    pub fn pause(env: Env) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();

        env.storage().persistent().set(&DataKey::Paused, &true);
        Ok(())
    }

    pub fn unpause(env: Env) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();

        env.storage().persistent().set(&DataKey::Paused, &false);
        Ok(())
    }

    pub fn extend_refund_ttl(env: Env, payment_ref: BytesN<32>) -> Result<(), Error> {
        if !env.storage().persistent().has(&DataKey::Refund(payment_ref.clone())) {
            return Err(Error::AlreadyRefunded);
        }
        env.storage()
            .persistent()
            .extend_ttl(&DataKey::Refund(payment_ref), 4096, 6312000);
        Ok()
    }
}

#[cfg(test)]
mod test;
