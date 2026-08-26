#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Address, Env, BytesN, Symbol};

    #[test]
    fn test_anchor_error_precedence_not_initialized_vs_unauthorized() {
        let env = Env::default();
        let anchor = ReceiptAnchorClient::new(&env, &env.register_contract(None, ReceiptAnchor));
        // Not initialized should throw NotInitialized before checking auth
        let res = anchor.try_anchor_batch(&BytesN::from_array(&env, &[0; 32]), &10, &0, &0);
        assert_eq!(res.unwrap_err().unwrap(), Symbol::new(&env, "NotInitialized"));
    }
}
