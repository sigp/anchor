use std::str::FromStr;

#[derive(Clone, Debug, Default)]
pub struct DomainType(pub [u8; 4]);

impl FromStr for DomainType {
    type Err = String;

    fn from_str(hex_str: &str) -> Result<Self, Self::Err> {
        let bytes = hex::decode(hex_str).map_err(|e| format!("Invalid domain type hex: {}", e))?;
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
