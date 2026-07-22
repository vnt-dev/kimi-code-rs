use std::collections::BTreeMap;

use crate::minidb::BatchInputOp;

use super::{router::Router, types::CrossShardMode, utils::InvalidShardCount};

pub struct Coordinator {
    mode: CrossShardMode,
}

impl Coordinator {
    pub fn new(mode: CrossShardMode) -> Self {
        Self { mode }
    }

    pub fn check_mode(&self, shard_count: usize) -> Result<(), String> {
        if shard_count <= 1 {
            return Ok(());
        }
        match self.mode {
            CrossShardMode::BestEffort => Ok(()),
            CrossShardMode::None => Err(format!(
                "operation spans {shard_count} shards but cross_shard mode is 'none'"
            )),
            CrossShardMode::TwoPhaseCommit => {
                Err("cross_shard mode '2pc' is reserved but not implemented yet".into())
            }
        }
    }

    pub fn group_entries<V>(
        &self,
        router: &Router,
        entries: Vec<(String, V)>,
    ) -> Result<BTreeMap<usize, Vec<(String, V)>>, InvalidShardCount> {
        let mut groups = BTreeMap::new();
        for entry in entries {
            groups
                .entry(router.shard_for(&entry.0)?)
                .or_insert_with(Vec::new)
                .push(entry);
        }
        Ok(groups)
    }

    pub fn group_keys(
        &self,
        router: &Router,
        keys: Vec<String>,
    ) -> Result<BTreeMap<usize, Vec<String>>, InvalidShardCount> {
        let mut groups = BTreeMap::new();
        for key in keys {
            groups
                .entry(router.shard_for(&key)?)
                .or_insert_with(Vec::new)
                .push(key);
        }
        Ok(groups)
    }

    pub fn group_operations<V>(
        &self,
        router: &Router,
        operations: Vec<BatchInputOp<V>>,
    ) -> Result<BTreeMap<usize, Vec<BatchInputOp<V>>>, InvalidShardCount> {
        let mut groups = BTreeMap::new();
        for operation in operations {
            let key = match &operation {
                BatchInputOp::Set { key, .. } | BatchInputOp::Del { key } => key,
            };
            groups
                .entry(router.shard_for(key)?)
                .or_insert_with(Vec::new)
                .push(operation);
        }
        Ok(groups)
    }
}
