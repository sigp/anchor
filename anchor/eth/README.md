## Execution Layer
This crate implements the execution layer component of the SSV node, responsible for monitoring and processing SSV network events on Ethereum L1 networks (Mainnet and Holesky).

## Overview
The execution layer client maintains synchronization with the SSV network contract by:
* Processing historical events from contract deployment
* Monitoring live contract events
* Managing validator and operator state changes
* Handling cluster lifecycle events

## Components
### SSV Event Syncer
This is the core synchronization engine that:
* Manages connections to an Ethereum execution client
* Handles historical and live event processing
* Maintains event ordering and state consistency
* Processes events in configurable batch sizes

### Event Processor
This processes network events and interacts with the database to validate event logs and persist them into the database.

## Event Types
```rust
event OperatorAdded(uint64 indexed operatorId, address indexed owner, bytes publicKey, uint256 fee)
event OperatorRemoved(uint64 indexed operatorId)
event ValidatorAdded(address indexed owner, uint64[] operatorIds, bytes publicKey, bytes shares, Cluster cluster)
event ValidatorRemoved(address indexed owner, uint64[] operatorIds, bytes publicKey, Cluster cluster)
event ClusterLiquidated(address indexed owner, uint64[] operatorIds, Cluster cluster)
event ClusterReactivated(address indexed owner, uint64[] operatorIds, Cluster cluster)
event FeeRecipientAddressUpdated(address indexed owner, address recipientAddress)
event ValidatorExited(address indexed owner, uint64[] operatorIds, bytes publicKey)
```

## Contract Addresses
* Mainnet: `0xDD9BC35aE942eF0cFa76930954a156B3fF30a4E1`
* Holesky: `0x38A4794cCEd47d3baf7370CcC43B560D3a1beEFA`
