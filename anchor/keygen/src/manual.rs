use crate::{base_processing, KeygenError, Manual};

pub fn manual_split(manual: Manual) -> Result<(), KeygenError> {
    let validator_keys = base_processing(&manual.shared)?;
    Ok(())
}
