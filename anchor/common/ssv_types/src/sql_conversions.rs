use std::{
    collections::HashMap,
    io::{Error, ErrorKind},
    str::FromStr,
};

use base64::prelude::*;
use indexmap::IndexSet;
use openssl::rsa::Rsa;
use rusqlite::{Error as SqlError, Row, types::Type};
use types::{Address, GRAFFITI_BYTES_LEN, Graffiti, PublicKeyBytes};

use crate::{
    Cluster, ClusterId, ClusterMember, ENCRYPTED_KEY_LENGTH, Operator, OperatorId, Share,
    ValidatorIndex, ValidatorMetadata,
};

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
        let cluster_id = ClusterId(row.get("cluster_id")?);

        let owner_str = row.get::<_, String>("owner")?;
        let owner = Address::from_str(&owner_str).map_err(|e| from_sql_error(1, Type::Text, e))?;

        let fee_recipient_str: Option<String> = row.get("fee_recipient")?;
        let fee_recipient = if let Some(fee_recipient) = fee_recipient_str {
            Address::from_str(&fee_recipient).map_err(|e| from_sql_error(2, Type::Text, e))?
        } else {
            owner
        };

        let liquidated: bool = row.get("liquidated")?;

        let operator_ids: IndexSet<OperatorId> = cluster_members
            .into_iter()
            .map(|member| member.operator_id)
            .collect();

        Ok(Cluster::new(
            cluster_id,
            owner,
            fee_recipient,
            liquidated,
            operator_ids,
        ))
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
// Note: This creates the basic ValidatorMetadata. Computed fields (owner, committee_id)
// should be resolved using ComputedFieldResolver after loading cluster data.
impl ValidatorMetadata {
    pub fn try_from(
        row: &Row,
        cluster_map: &HashMap<ClusterId, Cluster>,
    ) -> Result<Self, SqlError> {
        // Get public key from column 0
        let validator_pubkey_str = row.get::<_, String>(0)?;
        let public_key = PublicKeyBytes::from_str(&validator_pubkey_str)
            .map_err(|e| from_sql_error(1, Type::Text, Error::new(ErrorKind::InvalidInput, e)))?;

        // Get ClusterId from column 1
        let cluster_id: ClusterId = ClusterId(row.get(1)?);
        let cluster = cluster_map.get(&cluster_id).ok_or_else(|| {
            SqlError::FromSqlConversionFailure(
                1,
                Type::Text,
                Box::new(Error::new(ErrorKind::NotFound, "Cluster not found")),
            )
        })?;

        // Get ValidatorIndex from column 2
        let index = row.get::<_, Option<usize>>(2)?.map(ValidatorIndex);

        // Get Graffiti from column 3
        let graffiti = Graffiti(row.get::<_, [u8; GRAFFITI_BYTES_LEN]>(3)?);

        Ok(ValidatorMetadata {
            public_key,
            cluster_id,
            index,
            graffiti,
            owner: cluster.owner, // Use the cluster's owner as the validator's owner
            committee_id: cluster.committee_id,
        })
    }
}

// Conversion from SQL row into a Share
// Note: This creates the basic Share. Computed fields (owner, committee_id)
// should be resolved using ComputedFieldResolver after loading cluster data.
impl Share {
    pub fn try_from(
        row: &Row,
        cluster_map: &HashMap<ClusterId, Cluster>,
    ) -> Result<Self, rusqlite::Error> {
        // Get Share PublicKey from column 0
        let share_pubkey_str = row.get::<_, String>(0)?;
        let share_pubkey = PublicKeyBytes::from_str(&share_pubkey_str)
            .map_err(|e| from_sql_error(0, Type::Text, Error::new(ErrorKind::InvalidInput, e)))?;

        // Get the encrypted private key from column 1
        let encrypted_private_key: [u8; ENCRYPTED_KEY_LENGTH] = row.get(1)?;

        // Get the OperatorId from column 2 and ClusterId from column 3
        let operator_id = OperatorId(row.get(2)?);
        let cluster_id = ClusterId(row.get(3)?);

        let cluster = cluster_map.get(&cluster_id).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                Type::Text,
                Box::new(Error::new(
                    ErrorKind::NotFound,
                    "Cluster not found in the cluster map",
                )),
            )
        })?;

        // Get the Validator PublicKey from column 4
        let validator_pubkey_str = row.get::<_, String>(4)?;
        let validator_pubkey = PublicKeyBytes::from_str(&validator_pubkey_str)
            .map_err(|e| from_sql_error(4, Type::Text, Error::new(ErrorKind::InvalidInput, e)))?;

        // Create Share with basic fields
        // Computed fields (owner, committee_id) will be resolved using ComputedFieldResolver
        Ok(Share::new(
            validator_pubkey,
            operator_id,
            cluster_id,
            share_pubkey,
            encrypted_private_key,
            cluster.owner,        // Use the cluster's owner as the share's owner
            cluster.committee_id, // Use the cluster's committee_id
        ))
    }
}
