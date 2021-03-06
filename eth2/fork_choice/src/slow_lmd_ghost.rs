// Copyright 2019 Sigma Prime Pty Ltd.
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

extern crate db;

use crate::{ForkChoice, ForkChoiceError};
use db::{
    stores::{BeaconBlockStore, BeaconStateStore},
    ClientDB,
};
use std::collections::HashMap;
use std::sync::Arc;
use types::{
    readers::{BeaconBlockReader, BeaconStateReader},
    slot_epoch_height::Slot,
    validator_registry::get_active_validator_indices,
    BeaconBlock, Hash256,
};

//TODO: Pruning and syncing

//TODO: Sort out global constants
const GENESIS_SLOT: u64 = 0;
const FORK_CHOICE_BALANCE_INCREMENT: u64 = 1e9 as u64;
const MAX_DEPOSIT_AMOUNT: u64 = 32e9 as u64;
const EPOCH_LENGTH: u64 = 64;

pub struct SlowLMDGhost<T: ClientDB + Sized> {
    /// The latest attestation targets as a map of validator index to block hash.
    //TODO: Could this be a fixed size vec
    latest_attestation_targets: HashMap<u64, Hash256>,
    /// Stores the children for any given parent.
    children: HashMap<Hash256, Vec<Hash256>>,
    /// Block storage access.
    block_store: Arc<BeaconBlockStore<T>>,
    /// State storage access.
    state_store: Arc<BeaconStateStore<T>>,
}

impl<T> SlowLMDGhost<T>
where
    T: ClientDB + Sized,
{
    pub fn new(block_store: BeaconBlockStore<T>, state_store: BeaconStateStore<T>) -> Self {
        SlowLMDGhost {
            latest_attestation_targets: HashMap::new(),
            children: HashMap::new(),
            block_store: Arc::new(block_store),
            state_store: Arc::new(state_store),
        }
    }

    /// Finds the latest votes weighted by validator balance. Returns a hashmap of block_hash to
    /// weighted votes.
    pub fn get_latest_votes(
        &self,
        state_root: &Hash256,
        block_slot: Slot,
    ) -> Result<HashMap<Hash256, u64>, ForkChoiceError> {
        // get latest votes
        // Note: Votes are weighted by min(balance, MAX_DEPOSIT_AMOUNT) //
        // FORK_CHOICE_BALANCE_INCREMENT
        // build a hashmap of block_hash to weighted votes
        let mut latest_votes: HashMap<Hash256, u64> = HashMap::new();
        // gets the current weighted votes
        let current_state = self
            .state_store
            .get_deserialized(&state_root)?
            .ok_or_else(|| ForkChoiceError::MissingBeaconState(*state_root))?;

        let active_validator_indices = get_active_validator_indices(
            &current_state.validator_registry,
            block_slot.epoch(EPOCH_LENGTH),
        );

        for index in active_validator_indices {
            let balance =
                std::cmp::min(current_state.validator_balances[index], MAX_DEPOSIT_AMOUNT)
                    / FORK_CHOICE_BALANCE_INCREMENT;
            if balance > 0 {
                if let Some(target) = self.latest_attestation_targets.get(&(index as u64)) {
                    *latest_votes.entry(*target).or_insert_with(|| 0) += balance;
                }
            }
        }

        Ok(latest_votes)
    }

    /// Get the total number of votes for some given block root.
    ///
    /// The vote count is incremented each time an attestation target votes for a block root.
    fn get_vote_count(
        &self,
        latest_votes: &HashMap<Hash256, u64>,
        block_root: &Hash256,
    ) -> Result<u64, ForkChoiceError> {
        let mut count = 0;
        let block_slot = self
            .block_store
            .get_deserialized(&block_root)?
            .ok_or_else(|| ForkChoiceError::MissingBeaconBlock(*block_root))?
            .slot();

        for (target_hash, votes) in latest_votes.iter() {
            let (root_at_slot, _) = self
                .block_store
                .block_at_slot(&block_root, block_slot)?
                .ok_or(ForkChoiceError::MissingBeaconBlock(*block_root))?;
            if root_at_slot == *target_hash {
                count += votes;
            }
        }
        Ok(count)
    }
}

impl<T: ClientDB + Sized> ForkChoice for SlowLMDGhost<T> {
    /// Process when a block is added
    fn add_block(
        &mut self,
        block: &BeaconBlock,
        block_hash: &Hash256,
    ) -> Result<(), ForkChoiceError> {
        // build the children hashmap
        // add the new block to the children of parent
        (*self
            .children
            .entry(block.parent_root)
            .or_insert_with(|| vec![]))
        .push(block_hash.clone());

        // complete
        Ok(())
    }

    fn add_attestation(
        &mut self,
        validator_index: u64,
        target_block_root: &Hash256,
    ) -> Result<(), ForkChoiceError> {
        // simply add the attestation to the latest_attestation_target if the block_height is
        // larger
        let attestation_target = self
            .latest_attestation_targets
            .entry(validator_index)
            .or_insert_with(|| *target_block_root);
        // if we already have a value
        if attestation_target != target_block_root {
            // get the height of the target block
            let block_height = self
                .block_store
                .get_deserialized(&target_block_root)?
                .ok_or_else(|| ForkChoiceError::MissingBeaconBlock(*target_block_root))?
                .slot()
                .height(Slot::from(GENESIS_SLOT));

            // get the height of the past target block
            let past_block_height = self
                .block_store
                .get_deserialized(&attestation_target)?
                .ok_or_else(|| ForkChoiceError::MissingBeaconBlock(*attestation_target))?
                .slot()
                .height(Slot::from(GENESIS_SLOT));
            // update the attestation only if the new target is higher
            if past_block_height < block_height {
                *attestation_target = *target_block_root;
            }
        }
        Ok(())
    }

    /// A very inefficient implementation of LMD ghost.
    fn find_head(&mut self, justified_block_start: &Hash256) -> Result<Hash256, ForkChoiceError> {
        let start = self
            .block_store
            .get_deserialized(&justified_block_start)?
            .ok_or_else(|| ForkChoiceError::MissingBeaconBlock(*justified_block_start))?;

        let start_state_root = start.state_root();

        let latest_votes = self.get_latest_votes(&start_state_root, start.slot())?;

        let mut head_hash = Hash256::zero();

        loop {
            let mut head_vote_count = 0;

            let children = match self.children.get(&head_hash) {
                Some(children) => children,
                // we have found the head, exit
                None => break,
            };

            for child_hash in children {
                let vote_count = self.get_vote_count(&latest_votes, &child_hash)?;

                if vote_count > head_vote_count {
                    head_hash = *child_hash;
                    head_vote_count = vote_count;
                }
            }
        }
        Ok(head_hash)
    }
}
