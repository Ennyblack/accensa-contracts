#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, IntoVal, Symbol, Val};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    Paused = 4,
    AlreadyRefunded = 5,
    WindowExpired = 6,
    InsufficientFloat = 7,
    InvalidAmount = 8,
    InvalidProof = 9,
    InvalidWindow = 10,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundRecord {
    pub amount: i128,
    pub recipient: Address,
    pub ledger: u32,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    PendingAdmin,
    Token,
    RefundWindow,
    Paused,
    Refund(soroban_sdk::Bytes),
}

pub const MIN_REFUND_WINDOW: u32 = 1;

#[contract]
pub struct RefundVault;

#[contractimpl]
impl RefundVault {
    pub fn initialize(
        env: Env,
        merchant: Address,
        token: Address,
        refund_window_ledgers: u32,
    ) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        if refund_window_ledgers < MIN_REFUND_WINDOW {
            return Err(Error::InvalidWindow);
        }

        merchant.require_auth();

        env.storage().instance().set(&DataKey::Admin, &merchant);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::RefundWindow, &refund_window_ledgers);
        env.storage().instance().set(&DataKey::Paused, &false);

        Ok(()) 
    }

    pub fn deposit(env: Env, from: Address, amount: i128) -> Result<(), Error> {
        Self::check_paused(&env)?;
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        if from != admin {
            return Err(Error::Unauthorized);
        }
        admin.require_auth();

        let token_address: Address = env.storage().instance().get(&DataKey::Token).ok_or(Error::NotInitialized)?;
        let token = soroban_sdk::token::Client::new(&env, &token_address);
        token.transfer(&admin, &env.current_contract_address(), &amount);

        env.events().publish(
            (Symbol::new(&env, "deposit_event"), from),
            amount,
        );

        Ok(())
    }

    pub fn refund(
        env: Env,
        payment_ref: soroban_sdk::Bytes,
        recipient: Address,
        amount: i128,
        paid_at_ledger: u32,
    ) -> Result<(), Error> {
        Self::check_paused(&env)?;
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();

        let refund_key = DataKey::Refund(payment_ref.clone());
        if env.storage().persistent().has(&refund_key) {
            return Err(Error::AlreadyRefunded);
        }

        let window: u32 = env.storage().instance().get(&DataKey::RefundWindow).ok_or(Error::NotInitialized)?;
        let current_ledger = env.ledger().sequence();
        if current_ledger > paid_at_ledger + window {
            return Err(Error::WindowExpired);
        }

        let token_address: Address = env.storage().instance().get(&DataKey::Token).ok_or(Error::NotInitialized)?;
        let token = soroban_sdk::token::Client::new(&env, &token_address);

        let contract_balance = token.balance(&env.current_contract_address());
        if contract_balance < amount {
            return Err(Error::InsufficientFloat);
        }

        token.transfer(&env.current_contract_address(), &recipient, &amount);

        let record = RefundRecord {
            amount,
            recipient: recipient.clone(),
            ledger: current_ledger,
        };
        env.storage().persistent().set(&refund_key, &record);

        env.events().publish(
            (Symbol::new(&env, "refund_event"), payment_ref),
            record,
        );

        Ok(())
    }

    pub fn withdraw(env: Env, amount: i128, to: Address) -> Result<(), Error> {
        Self::check_paused(&env)?;
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();

        let token_address: Address = env.storage().instance().get(&DataKey::Token).ok_or(Error::NotInitialized)?;
        let token = soroban_sdk::token::Client::new(&env, &token_address);

        let contract_balance = token.balance(&env.current_contract_address());
        if contract_balance < amount {
            return Err(Error::InsufficientFloat);
        }

        token.transfer(&env.current_contract_address(), &to, &amount);

        env.events().publish(
            (Symbol::new(&env, "withdraw_event"), to),
            amount,
        );

        Ok(())
    }

    pub fn set_refund_window(env: Env, ledgers: u32) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();

        if ledgers < MIN_REFUND_WINDOW {
            return Err(Error::InvalidWindow);
        }

        env.storage().instance().set(&DataKey::RefundWindow, &ledgers);

        Ok(())
    }

    pub fn get_refund(env: Env, payment_ref: soroban_sdk::Bytes) -> Option<RefundRecord> {
        env.storage().persistent().get(&DataKey::Refund(payment_ref))
    }

    pub fn pause(env: Env) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        Ok(())
    }

    pub fn unpause(env: Env) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);
        Ok(())
    }

    pub fn extend_refund_ttl(env: Env, payment_ref: soroban_sdk::Bytes) -> Result<(), Error> {
        let refund_key = DataKey::Refund(payment_ref);
        if !env.storage().persistent().has(&refund_key) {
            return Err(Error::InvalidProof);
        }
        env.storage().persistent().extend_ttl(&refund_key, 4096, 4096);
        Ok(())
    }

    pub fn transfer_admin(env: Env, new_admin: Address) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::PendingAdmin, &new_admin);
        Ok(())
    }

    pub fn accept_admin(env: Env) -> Result<(), Error> {
        let new_admin: Address = env.storage().instance().get(&DataKey::PendingAdmin).ok_or(Error::Unauthorized)?;
        new_admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.storage().instance().remove(&DataKey::PendingAdmin);
        Ok(())
    }

    fn check_paused(env: &Env) -> Result<(), Error> {
        let paused: bool = env.storage().instance().get(&DataKey::Paused).unwrap_or(false);
        if paused {
            return Err(Error::InvalidAmount); // Or Paused error, maintaining existing compatibility
        }
        Ok(())
    }
}

#[cfg(test)]mod test;
