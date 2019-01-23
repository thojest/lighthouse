use super::{BeaconChain, SlotClock};
use db::{
    stores::{BeaconBlockAtSlotError, BeaconBlockStore},
    ClientDB, DBError,
};
use slot_clock::TestingSlotClockError;
use ssz::{ssz_encode, Encodable};
use std::collections::HashSet;
use std::sync::Arc;
use types::{
    readers::{BeaconBlockReader, BeaconStateReader},
    validator_registry::get_active_validator_indices,
    BeaconBlock, Hash256,
};

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Something,
}

#[derive(Debug, PartialEq)]
pub enum Error {
    DBError(String),
    MissingBeaconState(Hash256),
    InvalidBeaconState(Hash256),
    MissingBeaconBlock(Hash256),
    InvalidBeaconBlock(Hash256),
}

impl<T, U> BeaconChain<T, U>
where
    T: ClientDB,
    U: SlotClock,
    Error: From<<U as SlotClock>::Error>,
{
    pub fn slow_lmd_ghost(&mut self, start_hash: &Hash256) -> Result<Hash256, Error> {
        let start = self
            .block_store
            .get_reader(&start_hash)?
            .ok_or(Error::MissingBeaconBlock(*start_hash))?;

        let start_state_root = start.state_root();

        let state = self
            .state_store
            .get_reader(&start_state_root)?
            .ok_or(Error::MissingBeaconState(start_state_root))?
            .into_beacon_state()
            .ok_or(Error::InvalidBeaconState(start_state_root))?;

        let active_validator_indices =
            get_active_validator_indices(&state.validator_registry, start.slot());

        let mut attestation_targets = Vec::with_capacity(active_validator_indices.len());
        for i in active_validator_indices {
            if let Some(target) = self.latest_attestation_targets.get(&i) {
                attestation_targets.push(target);
            }
        }

        let mut head_hash = Hash256::zero();
        let mut head_vote_count = 0;

        loop {
            let child_hashes_and_slots =
                get_child_hashes_and_slots(&self.block_store, &head_hash, &self.leaf_blocks)?;

            if child_hashes_and_slots.len() == 0 {
                break;
            }

            for (child_hash, child_slot) in child_hashes_and_slots {
                let vote_count = get_vote_count(
                    &self.block_store,
                    &attestation_targets[..],
                    &child_hash,
                    child_slot,
                )?;

                if vote_count > head_vote_count {
                    head_hash = child_hash;
                    head_vote_count = vote_count;
                }
            }
        }

        Ok(head_hash)
    }
}

fn get_vote_count<T: ClientDB>(
    block_store: &Arc<BeaconBlockStore<T>>,
    attestation_targets: &[&Hash256],
    block_root: &Hash256,
    slot: u64,
) -> Result<u64, Error> {
    let mut count = 0;
    for target in attestation_targets {
        let (root_at_slot, _) = block_store
            .block_at_slot(&block_root, slot)?
            .ok_or(Error::MissingBeaconBlock(*block_root))?;
        if root_at_slot == *block_root {
            count += 1;
        }
    }
    Ok(count)
}

/// Starting from some `leaf_hashes`, recurse back down each branch until the `root_hash`, adding
/// each `block_root` and `slot` to a HashSet.
fn get_child_hashes_and_slots<T: ClientDB>(
    block_store: &Arc<BeaconBlockStore<T>>,
    root_hash: &Hash256,
    leaf_hashes: &HashSet<Hash256>,
) -> Result<HashSet<(Hash256, u64)>, Error> {
    let mut hash_set = HashSet::new();

    for leaf_hash in leaf_hashes {
        let mut current_hash = *leaf_hash;

        loop {
            if let Some(block_reader) = block_store.get_reader(&current_hash)? {
                let parent_root = block_reader.parent_root();

                let new_hash = hash_set.insert((current_hash, block_reader.slot()));

                // If the hash just added was already in the set, break the loop.
                //
                // In such a case, the present branch has merged with a branch that is already in
                // the set.
                if !new_hash {
                    break;
                }

                // The branch is exhausted if the parent of this block is the root_hash.
                if parent_root == *root_hash {
                    break;
                }

                current_hash = parent_root.clone();
            } else {
                return Err(Error::MissingBeaconBlock(current_hash));
            }
        }
    }

    Ok(hash_set)
}

impl From<DBError> for Error {
    fn from(e: DBError) -> Error {
        Error::DBError(e.message)
    }
}

impl From<BeaconBlockAtSlotError> for Error {
    fn from(e: BeaconBlockAtSlotError) -> Error {
        match e {
            BeaconBlockAtSlotError::UnknownBeaconBlock(h) => Error::MissingBeaconBlock(h),
            BeaconBlockAtSlotError::InvalidBeaconBlock(h) => Error::InvalidBeaconBlock(h),
            BeaconBlockAtSlotError::DBError(msg) => Error::DBError(msg),
        }
    }
}

impl From<TestingSlotClockError> for Error {
    fn from(_: TestingSlotClockError) -> Error {
        unreachable!(); // Testing clock never throws an error.
    }
}