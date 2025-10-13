use alloy::sol;

// Generate bindings around the SSV Network contract
sol! {
    #[derive(Debug)]
    #[sol(rpc)]
    contract SSVContract {
        struct Cluster {
            uint32 validatorCount;
            uint64 networkFeeIndex;
            uint64 index;
            bool active;
            uint256 balance;
        }
        event OperatorAdded(uint64 indexed operatorId, address indexed owner, bytes publicKey, uint256 fee);
        event OperatorRemoved(uint64 indexed operatorId);
        event ValidatorAdded(address indexed owner, uint64[] operatorIds, bytes publicKey, bytes shares, Cluster cluster);
        event ValidatorExited(address indexed owner, uint64[] operatorIds, bytes publicKey);
        event ValidatorRemoved(address indexed owner, uint64[] operatorIds, bytes publicKey, Cluster cluster);
        event FeeRecipientAddressUpdated(address indexed owner, address recipientAddress);
        event ClusterLiquidated(address indexed owner, uint64[] operatorIds, Cluster cluster);
        event ClusterReactivated(address indexed owner, uint64[] operatorIds, Cluster cluster);
    }
}
