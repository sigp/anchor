use crate::{Cluster, ClusterId, ClusterMember};
use crate::{Operator, OperatorId};
use crate::{Share, ValidatorIndex, ValidatorMetadata};
use base64::prelude::*;
use openssl::rsa::Rsa;
use rusqlite::{types::Type, Error as SqlError, Row};
use std::io::{Error, ErrorKind};
use std::str::FromStr;
use types::{Address, Graffiti, PublicKey, GRAFFITI_BYTES_LEN};

// Helper for converting to Rustqlite Error
fn from_sql_error<E: std::error::Error + Send + Sync + 'static>(
    col: usize,
    t: Type,
    e: E,
) -> SqlError {
    SqlError::FromSqlConversionFailure(col, t, Box::new(e))
}

// Conversion from SQL row to an Operator
impl TryFrom<&Row<'_>> for Operator {
    type Error = rusqlite::Error;
    fn try_from(row: &Row) -> Result<Self, Self::Error> {
        // Get the OperatorId from column 0
        let id: OperatorId = OperatorId(row.get(0)?);

        // Get the public key from column 1
        let pem_string = row.get::<_, String>(1)?;
        let decoded_pem = BASE64_STANDARD
            .decode(pem_string)
            .map_err(|e| from_sql_error(1, Type::Text, e))?;
        let rsa_pubkey =
            Rsa::public_key_from_pem(&decoded_pem).map_err(|e| from_sql_error(1, Type::Text, e))?;

        // Get the owner from column 2
        let owner_str = row.get::<_, String>(2)?;
        let owner = Address::from_str(&owner_str).map_err(|e| from_sql_error(2, Type::Text, e))?;

        Ok(Operator {
            id,
            rsa_pubkey,
            owner,
        })
    }
}

// Conversion from SQL row and cluster members into a Cluster
impl TryFrom<(&Row<'_>, Vec<ClusterMember>)> for Cluster {
    type Error = rusqlite::Error;

    fn try_from(
        (row, cluster_members): (&Row<'_>, Vec<ClusterMember>),
    ) -> Result<Self, Self::Error> {
        // Get ClusterId from column 0
        let cluster_id = ClusterId(row.get(0)?);

        // Get the owner from column 1
        let owner_str = row.get::<_, String>(1)?;
        let owner = Address::from_str(&owner_str).map_err(|e| from_sql_error(1, Type::Text, e))?;

        // Get the fee_recipient from column 2
        let fee_recipient_str = row.get::<_, String>(2)?;
        let fee_recipient =
            Address::from_str(&fee_recipient_str).map_err(|e| from_sql_error(2, Type::Text, e))?;

        // Get faulty count from column 3
        let faulty: u64 = row.get(3)?;

        // Get liquidated status from column 4
        let liquidated: bool = row.get(4)?;

        Ok(Cluster {
            cluster_id,
            owner,
            fee_recipient,
            faulty,
            liquidated,
            cluster_members: cluster_members
                .into_iter()
                .map(|member| member.operator_id)
                .collect(),
        })
    }
}

// Conversion from SQL row to a ClusterMember
impl TryFrom<&Row<'_>> for ClusterMember {
    type Error = rusqlite::Error;

    fn try_from(row: &Row) -> Result<Self, Self::Error> {
        // Get ClusterId from column 0
        let cluster_id = ClusterId(row.get(0)?);

        // Get OperatorId from column 1
        let operator_id = OperatorId(row.get(1)?);

        Ok(ClusterMember {
            operator_id,
            cluster_id,
        })
    }
}

// Conversion from SQL row to ValidatorMetadata
impl TryFrom<&Row<'_>> for ValidatorMetadata {
    type Error = SqlError;
    fn try_from(row: &Row) -> Result<Self, Self::Error> {
        // Get public key from column 0
        let validator_pubkey_str = row.get::<_, String>(0)?;
        let public_key = PublicKey::from_str(&validator_pubkey_str)
            .map_err(|e| from_sql_error(1, Type::Text, Error::new(ErrorKind::InvalidInput, e)))?;

        // Get ClusterId from column 1
        let cluster_id: ClusterId = ClusterId(row.get(1)?);

        // Get ValidatorIndex from column 2
        let index: ValidatorIndex = ValidatorIndex(row.get(2)?);

        // Get Graffiti from column 3
        let graffiti = Graffiti(row.get::<_, [u8; GRAFFITI_BYTES_LEN]>(3)?);

        Ok(ValidatorMetadata {
            public_key,
            cluster_id,
            index,
            graffiti,
        })
    }
}

// Conversion from SQL row into a Share
impl TryFrom<&Row<'_>> for Share {
    type Error = rusqlite::Error;
    fn try_from(row: &Row) -> Result<Self, Self::Error> {
        // Get Share PublicKey from column 0
        let share_pubkey_str = row.get::<_, String>(0)?;
        let share_pubkey = PublicKey::from_str(&share_pubkey_str)
            .map_err(|e| from_sql_error(0, Type::Text, Error::new(ErrorKind::InvalidInput, e)))?;

        // Get the encrypted private key from column 1
        let encrypted_private_key: [u8; 256] = row.get(1)?;

        // Get the OperatorId from column 2 and ClusterId from column 3
        let operator_id = OperatorId(row.get(2)?);
        let cluster_id = ClusterId(row.get(3)?);

        // Get the Validator PublicKey from column 4
        let validator_pubkey_str = row.get::<_, String>(4)?;
        let validator_pubkey = PublicKey::from_str(&validator_pubkey_str)
            .map_err(|e| from_sql_error(4, Type::Text, Error::new(ErrorKind::InvalidInput, e)))?;

        Ok(Share {
            validator_pubkey,
            operator_id,
            cluster_id,
            share_pubkey,
            encrypted_private_key,
        })
    }
}
