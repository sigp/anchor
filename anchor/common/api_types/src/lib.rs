use serde::Serialize;

#[derive(Serialize)]
pub struct VersionData {
    pub version: String,
}

#[derive(Serialize)]
pub struct ValidatorData {
    pub public_key: String,
    pub cluster_id: String,
    pub index: Option<usize>,
    pub graffiti: String,
}

#[derive(Serialize)]
pub struct GenericResponse<T> {
    pub data: T,
}

impl<T> From<T> for GenericResponse<T> {
    fn from(data: T) -> Self {
        Self { data }
    }
}
