use block_producer::{
    BeaconNode, BlockProducer, DutiesReader, PollOutcome as BlockProducerPollOutcome,
};
use slog::{error, info, warn, Logger};
use slot_clock::SlotClock;
use std::time::Duration;

pub struct BlockProducerService<T: SlotClock, U: BeaconNode, V: DutiesReader> {
    pub block_producer: BlockProducer<T, U, V>,
    pub poll_interval_millis: u64,
    pub log: Logger,
}

impl<T: SlotClock, U: BeaconNode, V: DutiesReader> BlockProducerService<T, U, V> {
    /// Run a loop which polls the block producer each `poll_interval_millis` millseconds.
    ///
    /// Logs the results of the polls.
    pub fn run(&mut self) {
        loop {
            match self.block_producer.poll() {
                Err(error) => {
                    error!(self.log, "Block producer poll error"; "error" => format!("{:?}", error))
                }
                Ok(BlockProducerPollOutcome::BlockProduced(slot)) => {
                    info!(self.log, "Produced block"; "slot" => slot)
                }
                Ok(BlockProducerPollOutcome::SlashableBlockNotProduced(slot)) => {
                    warn!(self.log, "Slashable block was not signed"; "slot" => slot)
                }
                Ok(BlockProducerPollOutcome::BlockProductionNotRequired(slot)) => {
                    info!(self.log, "Block production not required"; "slot" => slot)
                }
                Ok(BlockProducerPollOutcome::ProducerDutiesUnknown(slot)) => {
                    error!(self.log, "Block production duties unknown"; "slot" => slot)
                }
                Ok(BlockProducerPollOutcome::SlotAlreadyProcessed(slot)) => {
                    warn!(self.log, "Attempted to re-process slot"; "slot" => slot)
                }
                Ok(BlockProducerPollOutcome::BeaconNodeUnableToProduceBlock(slot)) => {
                    error!(self.log, "Beacon node unable to produce block"; "slot" => slot)
                }
            };

            std::thread::sleep(Duration::from_millis(self.poll_interval_millis));
        }
    }
}