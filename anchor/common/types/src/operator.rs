// Unique idetifier for an Operator
pub type OperatorID = u64;

// Operator RSA public key
pub type OperatorPublicKey = [u8; 459];

// Client responsible for maintaining the overall health of the network.
#[derive(Debug, Clone)]
pub struct Operator {
    // ID to uniquely identify this operator
    pub id: OperatorID,
    // Base-64 encoded PEM RSA public key
    pub public_key: OperatorPublicKey,
}
