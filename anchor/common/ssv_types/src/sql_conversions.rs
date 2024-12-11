use crate::{
    Cluster, ClusterId, ClusterMember, Operator, OperatorId, Share, ValidatorIndex,
    ValidatorMetadata,
};
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
    // Change the error type to rusqlite::Error
    type Error = SqlError;

    fn try_from(row: &Row) -> Result<Self, Self::Error> {
        let id: OperatorId = OperatorId(row.get(0)?);

        // For each operation that could fail, we convert its error to a rusqlite::Error
        let pem_string = row.get::<_, String>(1)?;
        let decoded_pem = BASE64_STANDARD
            .decode(pem_string)
            .map_err(|e| from_sql_error(1, Type::Text, e))?;

        let rsa_pubkey =
            Rsa::public_key_from_pem(&decoded_pem).map_err(|e| from_sql_error(1, Type::Text, e))?;

        let owner_str = row.get::<_, String>(2)?;
        let owner = Address::from_str(&owner_str).map_err(|e| from_sql_error(2, Type::Text, e))?;

        Ok(Operator {
            id,
            rsa_pubkey,
            owner,
        })
    }
}

// Conversion from SQL row into a Share
impl TryFrom<&Row<'_>> for Share {
    type Error = rusqlite::Error;
    fn try_from(row: &Row) -> Result<Self, Self::Error> {
        // We get the share_pubkey string from column 2
        let share_pubkey_str = row.get::<_, String>(2)?;

        // Convert the string to PublicKey, wrapping any parsing errors
        let share_pubkey = PublicKey::from_str(&share_pubkey_str)
            .map_err(|e| from_sql_error(2, Type::Text, Error::new(ErrorKind::InvalidInput, e)))?;

        Ok(Share { share_pubkey })
    }
}

// Conversion from SQL row and cluster members into a Cluster
impl TryFrom<(&Row<'_>, Vec<ClusterMember>)> for Cluster {
    type Error = rusqlite::Error;
    fn try_from((row, cluster_members): (&Row, Vec<ClusterMember>)) -> Result<Self, Self::Error> {
        // These are simple numeric/boolean conversions that use rusqlite's built-in error handling
        let cluster_id: ClusterId = ClusterId(row.get(0)?);
        let faulty: u64 = row.get(1)?;
        let liquidated: bool = row.get(2)?;

        // Convert the row to ValidatorMetadata - this will use the ValidatorMetadata impl
        // defined below
        let validator_metadata: ValidatorMetadata = row.try_into()?;

        Ok(Cluster {
            cluster_id,
            cluster_members,
            faulty,
            liquidated,
            validator_metadata,
        })
    }
}

// Conversion from SQL row to ValidatorMetadata
impl TryFrom<&Row<'_>> for ValidatorMetadata {
    type Error = SqlError;
    fn try_from(row: &Row) -> Result<Self, Self::Error> {
        // Get and parse validator_pubkey from column 3
        let validator_pubkey_str = row.get::<_, String>(3)?;
        let validator_pubkey = PublicKey::from_str(&validator_pubkey_str)
            .map_err(|e| from_sql_error(2, Type::Text, Error::new(ErrorKind::InvalidInput, e)))?;

        // Get the owner from column 7
        let owner_str = row.get::<_, String>(4)?;
        let owner = Address::from_str(&owner_str).map_err(|e| from_sql_error(7, Type::Text, e))?;

        // The rest of the field may not be populated upon first insert. So the values may be
        // default

        // Get and parse fee_recipient from column 4
        let fee_recipient_str = row.get::<_, String>(4)?;
        let fee_recipient =
            Address::from_str(&fee_recipient_str).map_err(|e| from_sql_error(4, Type::Text, e))?;

        // Get the Graffifi from column 5
        let graffiti = Graffiti(row.get::<_, [u8; GRAFFITI_BYTES_LEN]>(5)?);

        // Get validator_index from column 6
        let validator_index: ValidatorIndex = ValidatorIndex(row.get(6)?);


        Ok(ValidatorMetadata {
            validator_index,
            validator_pubkey,
            fee_recipient,
            graffiti,
            owner,
        })
    }
}
