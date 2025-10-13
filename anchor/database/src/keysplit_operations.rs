use base64::prelude::*;
use openssl::{pkey::Public, rsa::Rsa};
use rusqlite::params;
use types::Address;

use super::{DatabaseError, NetworkDatabase, sql_operations};

impl NetworkDatabase {
    // Get the public key for each operator id
    pub fn get_keys_for_operators(
        &self,
        operators: Vec<u64>,
    ) -> Result<Vec<Rsa<Public>>, DatabaseError> {
        let conn = self.connection()?;
        let mut stmt = conn.prepare(sql_operations::GET_OPERATOR_KEY)?;

        let mut pubkeys = vec![];

        // Fetch one at a time to maintain order
        for id in operators {
            let pem_string = stmt
                .query_row(params![id], |row| row.get::<_, String>(0))
                .map_err(DatabaseError::from)?;
            let decoded_pem = BASE64_STANDARD
                .decode(pem_string)
                .expect("Key was validated on insertion");
            let rsa_pubkey =
                Rsa::public_key_from_pem(&decoded_pem).expect("Key was validator on insertion");
            pubkeys.push(rsa_pubkey);
        }

        Ok(pubkeys)
    }

    // Fetch the nonce for the owner
    pub fn get_nonce_for_owner(&self, owner: Address) -> Result<Option<u64>, DatabaseError> {
        let conn = self.connection()?;
        let mut stmt = conn.prepare(sql_operations::GET_NONCE)?;
        let mut rows = stmt.query(params![owner.to_string()])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }
}
