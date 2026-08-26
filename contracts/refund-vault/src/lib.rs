#![no_std]

use soroban_sdk::{
    contract, contractclient, contracterror, contractevent, contractimpl, contractmeta,
    contracttype, token, Address, BytesN, Env,
};

contractmeta!(key = "name", val = "RefundVault");
contractmeta!(key = "version", val = env!("CARGO_PKG_VERSION"));
contractmeta!(
    key = "repo",
    val = "https://github.com/accensa/accensa-contracts"
);
contractmeta!(key = "commit", val = env!("GIT_SHA"));

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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct YieldInfo {
    pub deployed_principal: i128,
    pub harvested_yield: i128,
    pub strategy: Option<Address>,
    pub reserve_ratio: u32,
    pub max_deploy_ratio: u32,
}

/// Emitted when a payment is refunded from the vault float.
///
/// Topics: `("refund_event", payment_ref)`. The data map mirrors [`RefundRecord`],
/// so indexers can decode it with the same shape stored under the payment ref.
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
pub struct DepositEvent {
    #[topic]
    pub from: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WithdrawEvent {
    #[topic]
    pub to: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminTransferInitiatedEvent {
    #[topic]
    pub from: Address,
    #[topic]
    pub to: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminTransferAcceptedEvent {
    #[topic]
    pub from: Address,
    #[topic]
    pub to: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct YieldDeployedEvent {
    #[topic]
    pub strategy: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct YieldWithdrawnEvent {
    #[topic]
    pub strategy: Address,
    pub principal: i128,
    pub yield_amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct YieldHarvestedEvent {
    pub amount: i128,
}

/// Interface for external yield-generating strategies (e.g., Soroban lending protocols).
///
/// Any contract that implements these methods can be registered as the vault's yield
/// strategy. The vault calls these to deploy idle funds and harvest accrued yield.
#[contractclient(name = "YieldStrategyClient")]
pub trait YieldStrategy {
    /// Deploy `amount` tokens into the strategy. The vault transfers tokens to the
    /// strategy contract before calling this.
    fn deposit(env: Env, amount: i128) -> Result<(), Error>;

    /// Withdraw `principal` worth of tokens plus any proportional accrued yield.
    /// Returns `(principal_returned, yield_returned)`. The strategy transfers tokens
    /// back to the vault before returning.
    fn withdraw(env: Env, principal: i128) -> Result<(i128, i128), Error>;

    /// Harvest all accrued yield without touching deployed principal.
    /// Returns the yield amount. The strategy transfers yield tokens to the vault.
    fn harvest(env: Env) -> Result<i128, Error>;

    /// Read-only: total tokens held by this strategy (principal + accrued yield).
    fn total_balance(env: Env) -> i128;

    /// Read-only: accrued yield only (total_balance - total principal deployed).
    fn accrued_yield(env: Env) -> i128;
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

        let deployed: i128 = env
            .storage()
            .instance()
            .get(&DataKey::DeployedPrincipal)
            .unwrap_or(0);
        let harvested: i128 = env
            .storage()
            .instance()
            .get(&DataKey::HarvestedYield)
            .unwrap_or(0);

        // total_value = liquid tokens + deployed principal
        // (harvested yield has already been transferred to the vault and is part of token_balance,
        //  but it belongs to the operator, not the principal pool — subtract it)
        let total_value = token_balance + deployed - harvested;

        // Reserve check: after deployment, liquid tokens must cover the reserve.
        let reserve_ratio: u32 = env
            .storage()
            .instance()
            .get(&DataKey::ReserveRatio)
            .unwrap_or(0);
        let post_deploy_balance = token_balance - amount;
        let reserve_required = total_value * reserve_ratio as i128 / 10_000;
        if post_deploy_balance < reserve_required {
            return Err(Error::InsufficientReserve);
        }

        // Max deployment check.
        let max_deploy_ratio: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxDeployRatio)
            .unwrap_or(10_000);
        let post_deploy_total = deployed + amount;
        let max_deploy = total_value * max_deploy_ratio as i128 / 10_000;
        if post_deploy_total > max_deploy {
            return Err(Error::DeploymentExceedsMax);
        }

        // Transfer tokens to strategy, then notify the strategy of the deposit
        // (it needs to record the principal so it can return it on withdrawal).
        token_client.transfer(&env.current_contract_address(), &strategy, &amount);
        let strategy_client = YieldStrategyClient::new(&env, &strategy);
        strategy_client.deposit(&amount);

        env.storage()
            .instance()
            .set(&DataKey::DeployedPrincipal, &(deployed + amount));

        env.events().publish(
            (Symbol::new(&env, "withdraw_event"), to),
            amount,
        }
        .publish(&env);

        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
        Ok(())
    }

    /// Withdraw principal from the yield strategy. The strategy returns the requested
    /// principal plus any proportional accrued yield.
    ///
    /// `principal` is the amount of originally-deployed principal to reclaim.
    pub fn withdraw_from_yield(env: Env, principal: i128) -> Result<(), Error> {
        if env
            .storage()
            .instance()
            .get(&DataKey::IsPaused)
            .unwrap_or(false)
        {
            return Err(Error::Paused);
        }

        if principal <= 0 {
            return Err(Error::InvalidAmount);
        }

        let merchant: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        merchant.require_auth();

        let strategy: Address = env
            .storage()
            .instance()
            .get(&DataKey::YieldStrategy)
            .ok_or(Error::StrategyNotSet)?;

        let deployed: i128 = env
            .storage()
            .instance()
            .get(&DataKey::DeployedPrincipal)
            .unwrap_or(0);
        if principal > deployed {
            return Err(Error::NothingToWithdraw);
        }

        let strategy_client = YieldStrategyClient::new(&env, &strategy);
        let (principal_returned, yield_returned) = strategy_client.withdraw(&principal);

        let harvested: i128 = env
            .storage()
            .instance()
            .get(&DataKey::HarvestedYield)
            .unwrap_or(0);

        env.storage().instance().set(
            &DataKey::DeployedPrincipal,
            &(deployed - principal_returned),
        );
        env.storage()
            .instance()
            .set(&DataKey::HarvestedYield, &(harvested + yield_returned));

        YieldWithdrawnEvent {
            strategy,
            principal: principal_returned,
            yield_amount: yield_returned,
        }
        .publish(&env);

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
