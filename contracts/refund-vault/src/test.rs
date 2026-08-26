#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Address, Env, BytesN};

    #[test]
    fn test_refund_error_precedence_paused_vs_invalid_amount() {
        let env = Env::default();
        let vault = RefundVaultClient::new(&env, &env.register_contract(None, RefundVault));
        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        vault.initialize(&admin, &token, &100);
        vault.pause();
        
        // Paused error should trigger before InvalidAmount
        let res = vault.try_refund(&BytesN::from_array(&env, &[0; 32]), &Address::generate(&env), &-1, &0);
        assert_eq!(res.unwrap_err().unwrap(), Symbol::new(&env, "Paused"));
    }
}
