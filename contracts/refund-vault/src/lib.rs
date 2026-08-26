use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Symbol, token};

#[contracttype]
pub struct RefundRecord { pub amount: i128, pub recipient: Address, pub ledger: u32 }

#[contracttype]
pub enum DataKey {
    Admin,
    Token,
    RefundWindow,
    IsPaused,
    Refund(BytesN<32>),
}

#[contract]
pub struct RefundVault;

#[contractimpl]
impl RefundVault {
    /// Initializes the vault.
    /// # Errors
    /// - `AlreadyInitialized`: If already set.
    pub fn initialize(env: Env, merchant: Address, token: Address, refund_window: u32) -> Result<(), Symbol> {
        if env.storage().instance().has(&DataKey::Admin) { return Err(Symbol::new(&env, "AlreadyInitialized")); }
        env.storage().instance().set(&DataKey::Admin, &merchant);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::RefundWindow, &refund_window);
        env.storage().instance().set(&DataKey::IsPaused, &false);
        Ok(())
    }

    /// Deposits funds.
    /// # Errors
    /// - `NotInitialized`: If not init.
    /// - `Paused`: If contract is paused.
    /// # Traps
    /// - Traps if token transfer fails.
    pub fn deposit(env: Env, from: Address, amount: i128) -> Result<(), Symbol> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Symbol::new(&env, "NotInitialized"))?;
        if env.storage().instance().get(&DataKey::IsPaused).unwrap_or(false) { return Err(Symbol::new(&env, "Paused")); }
        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        token::Client::new(&env, &token_addr).transfer(&from, &env.current_contract_address(), &amount);
        Ok(())
    }

    /// Performs a refund.
    /// # Errors
    /// - `Paused`: Check order 1.
    /// - `NotInitialized`: Check order 2.
    /// - `InvalidAmount`: Check order 3.
    /// - `AlreadyRefunded`: Check order 4.
    /// - `WindowExpired`: Check order 5.
    /// - `InsufficientFloat`: Check order 6.
    /// # Traps
    /// - Traps if token transfer fails.
    pub fn refund(env: Env, payment_ref: BytesN<32>, recipient: Address, amount: i128, paid_at_ledger: u32) -> Result<(), Symbol> {
        if env.storage().instance().get(&DataKey::IsPaused).unwrap_or(false) { return Err(Symbol::new(&env, "Paused")); }
        let _admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Symbol::new(&env, "NotInitialized"))?;
        if amount <= 0 { return Err(Symbol::new(&env, "InvalidAmount")); }
        if env.storage().persistent().has(&DataKey::Refund(payment_ref.clone())) { return Err(Symbol::new(&env, "AlreadyRefunded")); }
        let window: u32 = env.storage().instance().get(&DataKey::RefundWindow).unwrap();
        if env.ledger().sequence() > paid_at_ledger + window { return Err(Symbol::new(&env, "WindowExpired")); }
        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let balance = token::Client::new(&env, &token_addr).balance(&env.current_contract_address());
        if balance < amount { return Err(Symbol::new(&env, "InsufficientFloat")); }
        token::Client::new(&env, &token_addr).transfer(&env.current_contract_address(), &recipient, &amount);
        env.storage().persistent().set(&DataKey::Refund(payment_ref), &RefundRecord { amount, recipient, ledger: env.ledger().sequence() });
        Ok(())
    }

    /// Withdraws float.
    /// # Errors
    /// - `NotInitialized`: Not init.
    /// - `Paused`: If paused.
    /// # Traps
    /// - Traps if token transfer fails.
    pub fn withdraw(env: Env, amount: i128, to: Address) -> Result<(), Symbol> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Symbol::new(&env, "NotInitialized"))?;
        admin.require_auth();
        if env.storage().instance().get(&DataKey::IsPaused).unwrap_or(false) { return Err(Symbol::new(&env, "Paused")); }
        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        token::Client::new(&env, &token_addr).transfer(&env.current_contract_address(), &to, &amount);
        Ok(())
    }

    pub fn set_refund_window(env: Env, ledgers: u32) -> Result<(), Symbol> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Symbol::new(&env, "NotInitialized"))?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::RefundWindow, &ledgers);
        Ok(())
    }

    pub fn get_refund(env: Env, payment_ref: BytesN<32>) -> Option<RefundRecord> {
        env.storage().persistent().get(&DataKey::Refund(payment_ref))
    }

    pub fn pause(env: Env) -> Result<(), Symbol> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Symbol::new(&env, "NotInitialized"))?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::IsPaused, &true);
        Ok(())
    }

    pub fn unpause(env: Env) -> Result<(), Symbol> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Symbol::new(&env, "NotInitialized"))?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::IsPaused, &false);
        Ok(())
    }

    pub fn extend_refund_ttl(env: Env, payment_ref: BytesN<32>) -> Result<(), Symbol> {
        if !env.storage().persistent().has(&DataKey::Refund(payment_ref.clone())) { return Err(Symbol::new(&env, "NotFound")); }
        env.storage().persistent().extend_ttl(&DataKey::Refund(payment_ref), 100000, 100000);
        Ok(())
    }
}
