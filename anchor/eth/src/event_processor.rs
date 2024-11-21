use super::event_parser::NetworkAction;
use alloy::rpc::types::Log;

// Given a set of logs, the event processor will persist the information into the underlying
// database, parse the logs into some network action, and then send the action to be executed
pub struct EventProcessor {
    // reference to the database
    // communication w/ central processor
    // keymanager (from spec/impl)
}

impl EventProcessor {
    /// Construct a new EventProcessor
    pub fn new() -> Self {
        Self {}
    }

    /// Process a new set of logs
    pub fn process_logs(&self, logs: Vec<Log>, live: bool) -> Result<(), String> {
        for log in logs {
            // perform all DB updated needed with the log
            // todo!()

            // If we have a valid action and are live, then send off to the controller to execute
            let action: NetworkAction = log.try_into()?;
            if action != NetworkAction::NoOp && live {
                // todo!() send off somewhere
            }
        }

        Ok(())
    }
}
