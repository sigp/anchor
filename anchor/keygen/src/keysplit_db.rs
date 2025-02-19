use openssl::rsa::Rsa;
use types::Address;
use openssl::pkey::Public;


pub struct KeysplitDatabase {
}

impl KeysplitDatabase {
    pub fn new() -> Self {
        todo!()
    }

    pub fn get_keys_for_operators(&self, operators: Vec<u64>) -> Result<Vec<Rsa<Public>>, String> {
        todo!()
    }

    pub fn get_nonce_for_owner(&self, owner: Address) -> u64 {
        0
    }
}
