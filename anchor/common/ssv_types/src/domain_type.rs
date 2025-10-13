use std::str::FromStr;

use rusqlite::{
    ToSql,
    types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, Value, ValueRef},
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DomainType(pub [u8; 4]);

impl FromStr for DomainType {
    type Err = String;

    fn from_str(hex_str: &str) -> Result<Self, Self::Err> {
        let bytes = hex::decode(hex_str).map_err(|e| format!("Invalid domain type hex: {e}"))?;
        if bytes.len() != 4 {
            return Err("Domain type must be 4 bytes".into());
        }
        let mut domain_type = [0; 4];
        domain_type.copy_from_slice(&bytes);
        Ok(Self(domain_type))
    }
}

impl From<DomainType> for String {
    fn from(domain_type: DomainType) -> Self {
        hex::encode(domain_type.0)
    }
}

impl From<[u8; 4]> for DomainType {
    fn from(bytes: [u8; 4]) -> Self {
        Self(bytes)
    }
}

impl FromSql for DomainType {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let value = value.as_i64()?;
        let value = u32::try_from(value).map_err(|_| FromSqlError::InvalidType)?;
        Ok(value.to_le_bytes().into())
    }
}

impl ToSql for DomainType {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(Value::Integer(
            u32::from_le_bytes(self.0).into(),
        )))
    }
}
