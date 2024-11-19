use alloy::sol;

// Generate bindings around the SSV Network contract
sol! {
    #[derive(Debug)]
    #[sol(rpc)]
    SSVContract,
    "src/abi/ssv_contract.json"
}
