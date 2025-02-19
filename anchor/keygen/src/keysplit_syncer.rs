use crate::keysplit_db::KeysplitDatabase;
use std::sync::Arc;

pub struct KeysplitSyncer {
}


impl KeysplitSyncer {
    pub fn new(rpc: String, db: Arc<KeysplitDatabase>) -> Self {
        todo!()
    }

    pub async fn sync_data(&self) {
        todo!()
    }
}

