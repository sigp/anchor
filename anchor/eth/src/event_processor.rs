use super::event_parser::NetworkAction;
use alloy::rpc::types::Log;

// todo!()
// Given a set of logs, the event processor should transform it into some network action
// process it by performing all validation/write to db/anything else, and then send a message
// to some executor to perform a task if we are live

// Process a new event by persisting it into the database and notifying
// the central processor this event has occured
pub struct EventProcessor {
    // reference to the database
    // communication w/ central processor
    // keymanager (from spec/impl)
}

impl EventProcessor {
    pub fn process_logs(&self, logs: Vec<Log>, live: bool) {
        // Go through all of the logs and parse/process them based on the log types
        // Reflect the change in the database and send event to central processor if we are live
        for log in logs {
            let action: NetworkAction = log.into();

            if live {
                // send off to the central processor
            }
        }
    }
}
