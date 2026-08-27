#![no_std]

use soroban_sdk::{
    contract, contractclient, contracterror, contractevent, contractimpl, contractmeta,
    contracttype, token, Address, BytesN, Env, Symbol,
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
    StrategyNotSet = 11,
    InvalidRatio = 12,
    InsufficientReserve = 13,
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

#[contracttype]
pub enum DataKey {
    Admin,
    Token,
    RefundWindow,
    Paused,
    Refund(BytesN<32>),
    PendingAdmin,
    YieldStrategy,
    ReserveRatio,
    MaxDeployRatio,
    DeployedPrincipal,
    HarvestedYield,
}

pub const MIN_REFUND_WINDOW: u32 = 1;
pub const BASIS_POINTS_DENOMINATOR: u32 = 10_000;

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
        let token_client = token::Client::new(&env, &token_address);
        token_client.transfer(&admin, &env.current_contract_address(), &amount);

        env.events().publish(
            (Symbol::new(&env, "deposit_event"), from),
            amount,
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
        let token_client = token::Client::new(&env, &token_address);

        let mut contract_balance = token_client.balance(&env.current_contract_address());

        if contract_balance < amount {
            let deployed_principal: i128 = env.storage().instance().get(&DataKey::DeployedPrincipal).unwrap_or(0);
            if deployed_principal > 0 {
                if let Some(strategy_addr) = env.storage().instance().get::<_, Address>(&DataKey::YieldStrategy) {
                    let needed = amount - contract_balance;
                    let withdraw_amount = core::cmp::min(needed, deployed_principal);
                    if withdraw_amount > 0 {
                        let strategy_client = YieldStrategyClient::new(&env, &strategy_addr);
                        if let Ok((_p, _y)) = strategy_client.withdraw(&withdraw_amount) {
                            env.storage().instance().set(&DataKey::DeployedPrincipal, &(deployed_principal - withdraw_amount));
                            contract_balance = token_client.balance(&env.current_contract_address());
                        }
                    }
                }
            }
        }

        if contract_balance < amount {
            return Err(Error::InsufficientFloat);
        }

        token_client.transfer(&env.current_contract_address(), &recipient, &amount);

        let record = RefundRecord {
            amount,
            recipient: recipient.clone(),
            ledger: current_ledger,
        };
        env.storage().persistent().set(&refund_key, &record);

        env.events().publish(
            (Symbol::new(&env, "refund_event"), payment_ref.clone()),
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
        let token_client = token::Client::new(&env, &token_address);

        let mut contract_balance = token_client.balance(&env.current_contract_address());
        if contract_balance < amount {
            let deployed_principal: i128 = env.storage().instance().get(&DataKey::DeployedPrincipal).unwrap_or(0);
            if deployed_principal > 0 {
                if let Some(strategy_addr) = env.storage().instance().get::<_, Address>(&DataKey::YieldStrategy) {
                    let needed = amount - contract_balance;
                    let withdraw_amount = core::cmp::min(needed, deployed_principal);
                    if withdraw_amount > 0 {
                        let strategy_client = YieldStrategyClient::new(&env, &strategy_addr);
                        if let Ok((_p, _y)) = strategy_client.withdraw(&withdraw_amount) {
                            env.storage().instance().set(&DataKey::DeployedPrincipal, &(deployed_principal - withdraw_amount));
                            contract_balance = token_client.balance(&env.current_contract_address());
                        }
                    }
                }
            }
        }

        if contract_balance < amount {
            return Err(Error::InsufficientFloat);
        }

        token_client.transfer(&env.current_contract_address(), &to, &amount);

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

    pub fn get_refund(env: Env, payment_ref: BytesN<32>) -> Option<RefundRecord> {
        let key = DataKey::Refund(payment_ref);
        env.storage().persistent().get(&key)
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

    pub fn extend_refund_ttl(env: Env, payment_ref: BytesN<32>) -> Result<(), Error> {
        let key = DataKey::Refund(payment_ref);
        if !env.storage().persistent().has(&key) {
            return Err(Error::NotInitialized);
        }
        let ttl = env.storage().persistent().get_ttl(&key);
        let min_ttl = env.storage().persistent().max_ttl();
        env.storage().persistent().extend_ttl(&key, ttl, min_ttl);
        Ok(())
    }

    pub fn transfer_admin(env: Env, new_admin: Address) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();

        env.storage().instance().set(&DataKey::PendingAdmin, &new_admin);
        env.events().publish(
            (Symbol::new(&env, "admin_transfer_initiated"), admin, new_admin.clone()),
            (),
        );
        Ok(())
    }

    pub fn accept_admin(env: Env) -> Result<(), Error> {
        let pending: Address = env.storage().instance().get(&DataKey::PendingAdmin).ok_or(Error::Unauthorized)?;
        pending.require_auth();

        let old_admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;

        env.storage().instance().set(&DataKey::Admin, &pending);
        env.storage().instance().remove(&DataKey::PendingAdmin);

        env.events().publish(
            (Symbol::new(&env, "admin_transfer_accepted"), old_admin, pending),
            (),
        );
        Ok(())
    }

    pub fn cancel_admin_transfer(env: Env) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();

        if !env.storage().instance().has(&DataKey::PendingAdmin) {
            return Err(Error::Unauthorized);
        }
        env.storage().instance().remove(&DataKey::PendingAdmin);
        Ok(())
    }

    pub fn set_yield_strategy(env: Env, strategy: Address) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::YieldStrategy, &strategy);
        Ok(())
    }

    pub fn set_reserve_ratio(env: Env, ratio_bp: u32) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();
        if ratio_bp > BASIS_POINTS_DENOMINATOR {
            return Err(Error::InvalidRatio);
        }
        env.storage().instance().set(&DataKey::ReserveRatio, &ratio_bp);
        Ok(())
    }

    pub fn set_max_deploy_ratio(env: Env, ratio_bp: u32) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();
        if ratio_bp > BASIS_POINTS_DENOMINATOR {
            return Err(Error::InvalidRatio);
        }
        env.storage().instance().set(&DataKey::MaxDeployRatio, &ratio_bp);
        Ok(())
    }

    pub fn deploy_yield(env: Env, amount: i128) -> Result<(), Error> {
        Self::check_paused(&env)?;
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();

        let strategy_addr: Address = env.storage().instance().get(&DataKey::YieldStrategy).ok_or(Error::StrategyNotSet)?;
        let token_address: Address = env.storage().instance().get(&DataKey::Token).ok_or(Error::NotInitialized)?;
        let token_client = token::Client::new(&env, &token_address);

        let contract_balance = token_client.balance(&env.current_contract_address());
        let reserve_ratio: u32 = env.storage().instance().get(&DataKey::ReserveRatio).unwrap_or(2_000);
        let required_reserve = contract_balance * (reserve_ratio as i128) / (BASIS_POINTS_DENOMINATOR as i128);

        if contract_balance - amount < required_reserve {
            return Err(Error::InsufficientReserve);
        }

        token_client.transfer(&env.current_contract_address(), &strategy_addr, &amount);
        let strategy_client = YieldStrategyClient::new(&env, &strategy_addr);
        strategy_client.deposit(&amount)?;

        let deployed: i128 = env.storage().instance().get(&DataKey::DeployedPrincipal).unwrap_or(0);
        env.storage().instance().set(&DataKey::DeployedPrincipal, &(deployed + amount));

        env.events().publish(
            (Symbol::new(&env, "yield_deployed"), strategy_addr),
            amount,
        );

        Ok(())
    }

    pub fn harvest_yield(env: Env) -> Result<i128, Error> {
        Self::check_paused(&env)?;
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();

        let strategy_addr: Address = env.storage().instance().get(&DataKey::YieldStrategy).ok_or(Error::StrategyNotSet)?;
        let strategy_client = YieldStrategyClient::new(&env, &strategy_addr);

        let harvested = strategy_client.harvest()?;
        if harvested > 0 {
            let total_harvested: i128 = env.storage().instance().get(&DataKey::HarvestedYield).unwrap_or(0);
            env.storage().instance().set(&DataKey::HarvestedYield, &(total_harvested + harvested));

            env.events().publish(
                Symbol::new(&env, "yield_harvested"),
                harvested,
            );
        }

        Ok(harvested)
    }

    pub fn get_yield_info(env: Env) -> YieldInfo {
        let deployed_principal: i128 = env.storage().instance().get(&DataKey::DeployedPrincipal).unwrap_or(0);
        let harvested_yield: i128 = env.storage().instance().get(&DataKey::HarvestedYield).unwrap_or(0);
        let strategy = env.storage().instance().get(&DataKey::YieldStrategy);
        let reserve_ratio: u32 = env.storage().instance().get(&DataKey::ReserveRatio).unwrap_or(2_000);
        let max_deploy_ratio: u32 = env.storage().instance().get(&DataKey::MaxDeployRatio).unwrap_or(8_000);

        YieldInfo {
            deployed_principal,
            harvested_yield,
            strategy,
            reserve_ratio,
            max_deploy_ratio,
        }
    }

    fn check_paused(env: &Env) -> Result<(), Error> {
        let paused: bool = env.storage().instance().get(&DataKey::Paused).unwrap_or(false);
        if paused {
            return Err(Error::Paused);
        }
        Ok(())
    }
}

#[cfg(test)]
mod test;
#[cfg(test)]
mod yield_tests;
